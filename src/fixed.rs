//! Module with fixed buffers.
//!
//! See [`BufPool`].

use std::alloc::{self, alloc, alloc_zeroed, dealloc};
use std::mem::{size_of, MaybeUninit};
use std::ops::{Deref, DerefMut};
use std::os::fd::AsRawFd;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::{fmt, io, slice};

use crate::io::BufMut;
use crate::{libc, SubmissionQueue};

/// Id for a [`BufPool`].
#[doc(hidden)] // Public because it's used in [`BufMut`].
#[derive(Copy, Clone, Debug)]
pub struct BufGroupId(pub(crate) u16);

/// Index for a [`BufPool`].
#[doc(hidden)] // Public because it's used in [`BufMut`].
#[derive(Copy, Clone, Debug)]
pub struct BufIdx(pub(crate) u16);

/// Size of a single page, often 4096.
static PAGE_SIZE: LazyLock<usize> =
    LazyLock::new(|| unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize });

/// Buffer group ID generator.
static ID: AtomicU16 = AtomicU16::new(0);

/// A read buffer pool.
#[derive(Clone, Debug)]
pub struct ReadBufPool {
    shared: Arc<Shared>,
}

/// Shared between one or more [`ReadBufPool`]s and one or more [`ReadBuf`]s.
#[derive(Debug)]
struct Shared {
    /// Identifier used by the kernel (aka `bgid`, `buf_group`).
    id: BufGroupId,
    /// Submission queue used to unregister the pool on drop.
    sq: SubmissionQueue,
    /// Number of buffers.
    pool_size: u16,
    /// Size of the buffers.
    buf_size: u32,
    /// Address of the allocation the buffers, see `alloc_layout_buffers`.
    bufs_addr: *mut u8,
    /// Address of the ring registration, see `alloc_layout_ring`.
    ring_addr: *mut libc::io_uring_buf_ring,
    /// Mask used to determin the tail in the ring.
    tail_mask: u16,
    /// Lock used reregister [`ReadBuf`]s after usage, see the `Drop` implementation
    /// of `ReadBuf`.
    reregister_lock: Mutex<()>,
}

impl ReadBufPool {
    /// Create a new buffer pool.
    ///
    /// `pool_size` must be a power of 2, with a maximum of 2^15 (32768).
    pub fn new(sq: SubmissionQueue, pool_size: u16, buf_size: u32) -> io::Result<ReadBufPool> {
        debug_assert!(pool_size <= 2 ^ 15);
        debug_assert!(pool_size.is_power_of_two());

        let ring_fd = sq.shared.ring_fd.as_raw_fd();
        let id = ID.fetch_add(1, Ordering::SeqCst);

        // This allocation must be page aligned.
        let page_size = *PAGE_SIZE;
        // NOTE: do the layout calculations first in case of an error.
        let ring_layout = alloc_layout_ring(pool_size, page_size)?;
        let bufs_layout = alloc_layout_buffers(pool_size, buf_size, page_size)?;

        // Allocation for the buffer ring, shared with the kernel.
        let ring_addr = match unsafe { alloc_zeroed(ring_layout) } {
            ring_addr if ring_addr.is_null() => return Err(io::ErrorKind::OutOfMemory.into()),
            ring_addr => ring_addr.cast::<libc::io_uring_buf_ring>(),
        };

        // Register the buffer ring with the kernel.
        let buf_register = libc::io_uring_buf_reg {
            ring_addr: ring_addr as u64,
            ring_entries: pool_size as u32,
            bgid: id,
            // Padding and reserved for future use.
            pad: 0,
            resv: [0; 3],
        };
        log::trace!(ring_fd = ring_fd, bgid = id, size = pool_size; "registering buffer pool");
        let result = libc::syscall!(io_uring_register(
            ring_fd,
            libc::IORING_REGISTER_PBUF_RING,
            ptr::addr_of!(buf_register).cast(),
            1,
        ));
        if let Err(err) = result {
            // SAFETY: we just allocated this above.
            unsafe { dealloc(ring_addr.cast(), ring_layout) };
            return Err(err);
        }

        // Create a `Shared` type to manage the allocations and registration.
        let shared = Shared {
            id: BufGroupId(id),
            sq,
            pool_size,
            buf_size,
            // Allocate the buffer space, checked below.
            bufs_addr: unsafe { alloc(bufs_layout) },
            ring_addr,
            // NOTE: this works because `pool_size` must be a power of two.
            tail_mask: pool_size - 1,
            reregister_lock: Mutex::new(()),
        };

        if shared.bufs_addr.is_null() {
            // NOTE: dealloc and unregister happen in the `Drop` impl of
            // `Shared.
            return Err(io::ErrorKind::OutOfMemory.into());
        }

        // Fill the buffer ring to let the kernel know what buffers are
        // available.
        let ring_tail = shared.ring_tail();
        let ring_addr = unsafe { &mut *ring_addr };
        let bufs = unsafe {
            slice::from_raw_parts_mut(
                ptr::addr_of_mut!(ring_addr.__bindgen_anon_1.bufs)
                    .cast::<MaybeUninit<libc::io_uring_buf>>(),
                pool_size as usize,
            )
        };
        for (i, ring_buf) in bufs.iter_mut().enumerate() {
            let addr = unsafe { shared.bufs_addr.add(i * buf_size as usize) };
            log::trace!(bid = i, addr = log::as_debug!(addr), len = buf_size; "registering buffer");
            ring_buf.write(libc::io_uring_buf {
                addr: addr as u64,
                len: buf_size,
                bid: i as u16,
                resv: 0,
            });
        }
        ring_tail.store(pool_size, Ordering::Release);

        Ok(ReadBufPool {
            shared: Arc::new(shared),
        })
    }

    /// Get a buffer reference to this pool.
    ///
    /// This can be used in read I/O operation, such as [`AsyncFd::read`], but
    /// it won't yet allocate a buffer to use. This is done by the kernel once
    /// it actually has data to write into the buffer.
    ///
    /// [`AsyncFd::read`]: crate::AsyncFd::read
    pub fn get(&self) -> ReadBuf {
        ReadBuf {
            shared: self.shared.clone(),
            owned: None,
        }
    }
}

impl Shared {
    /// Returns the tail of buffer ring.
    fn ring_tail(&self) -> &AtomicU16 {
        unsafe {
            &*(ptr::addr_of!(((*self.ring_addr).__bindgen_anon_1.__bindgen_anon_1.tail))
                .cast::<AtomicU16>())
        }
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        let page_size = *PAGE_SIZE;

        // Unregister the buffer pool with the ring.
        let buf_register = libc::io_uring_buf_reg {
            bgid: self.id.0,
            // Unused in this call.
            ring_addr: 0,
            ring_entries: 0,
            // Padding and reserved for future use.
            pad: 0,
            resv: [0; 3],
        };
        let result = libc::syscall!(io_uring_register(
            self.sq.shared.ring_fd.as_raw_fd(),
            libc::IORING_UNREGISTER_PBUF_RING,
            ptr::addr_of!(buf_register).cast(),
            1,
        ));
        if let Err(err) = result {
            log::warn!("failed to unregister a10::ReadBufPool: {err}");
        }

        // Next deallocate the ring.
        unsafe {
            // SAFETY: created this layout in `new` and didn't fail, so it's
            // still valid here.
            let ring_layout = alloc_layout_ring(self.pool_size, page_size).unwrap();
            // SAFETY: we allocated this in `new`, so it's safe to deallocate
            // for us.
            dealloc(self.ring_addr.cast(), ring_layout);
        };

        // And finally deallocate the buffers themselves.
        if !self.bufs_addr.is_null() {
            unsafe {
                // SAFETY: created this layout in `new` and didn't fail, so it's
                // still valid here.
                let layout =
                    alloc_layout_buffers(self.pool_size, self.buf_size, page_size).unwrap();
                // SAFETY: we allocated this in `new`, so it's safe to
                // deallocate for us.
                dealloc(self.bufs_addr, layout);
            }
        }
    }
}

fn alloc_layout_buffers(
    pool_size: u16,
    buf_size: u32,
    page_size: usize,
) -> io::Result<alloc::Layout> {
    match alloc::Layout::from_size_align(pool_size as usize * buf_size as usize, page_size) {
        Ok(layout) => Ok(layout),
        // This will only fail if the size is larger then roughly
        // `isize::MAX - PAGE_SIZE`, which is a huge allocation.
        Err(_) => Err(io::ErrorKind::OutOfMemory.into()),
    }
}

fn alloc_layout_ring(pool_size: u16, page_size: usize) -> io::Result<alloc::Layout> {
    match alloc::Layout::from_size_align(
        size_of::<libc::io_uring_buf_ring>() * pool_size as usize,
        page_size,
    ) {
        Ok(layout) => Ok(layout),
        // This will only fail if the size is larger then roughly
        // `isize::MAX - PAGE_SIZE`, which is a huge allocation.
        Err(_) => Err(io::ErrorKind::OutOfMemory.into()),
    }
}

/// Buffer reference from a [`ReadBufPool`].
pub struct ReadBuf {
    /// Buffer pool info.
    shared: Arc<Shared>,
    /// This is `Some` if the buffer was assigned.
    owned: Option<NonNull<[u8]>>,
}

impl ReadBuf {
    /// Returns the capacity of the buffer.
    pub fn capacity(&self) -> usize {
        self.shared.buf_size as usize
    }

    /// Returns the length of the buffer.
    pub fn len(&self) -> usize {
        self.owned.map_or(0, NonNull::len)
    }

    /// Returns itself as slice.
    pub fn as_slice(&self) -> &[u8] {
        self
    }

    /// Returns itself as mutable slice.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self
    }
}

unsafe impl BufMut for ReadBuf {
    unsafe fn parts(&mut self) -> (*mut u8, u32) {
        (ptr::null_mut(), self.shared.buf_size)
    }

    unsafe fn set_init(&mut self, _: usize) {
        panic!("Don't call a10::Buf::set_init");
    }

    fn buffer_group(&self) -> Option<BufGroupId> {
        Some(self.shared.id)
    }

    unsafe fn buffer_init(&mut self, idx: BufIdx, n: u32) {
        debug_assert!(self.owned.is_none());
        let data = self
            .shared
            .bufs_addr
            .add(idx.0 as usize * self.shared.buf_size as usize);
        // SAFETY: `bufs_addr` is not NULL.
        let data = unsafe { NonNull::new_unchecked(data) };
        self.owned = Some(NonNull::slice_from_raw_parts(data, n as usize));
    }
}

impl Deref for ReadBuf {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.owned.map_or(&[], |ptr| unsafe { ptr.as_ref() })
    }
}

impl DerefMut for ReadBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.owned
            .map_or(&mut [], |mut ptr| unsafe { ptr.as_mut() })
    }
}

impl AsRef<[u8]> for ReadBuf {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl fmt::Debug for ReadBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_slice().fmt(f)
    }
}

impl Drop for ReadBuf {
    fn drop(&mut self) {
        if let Some(ptr) = self.owned {
            let ring_tail = self.shared.ring_tail();

            // Calculate the buffer index based on the `ptr`, which points to
            // the start of our buffer, and `bufs_addr`, which points to the
            // start of the pool, by calculating the difference and dividing it
            // by the buffer size.
            let buf_idx = unsafe {
                (ptr.as_mut_ptr().sub_ptr(self.shared.bufs_addr)) / self.shared.buf_size as usize
            } as u16;

            // Because we need to fill the `ring_buf` and then atomatically
            // update the `ring_tail` we do it while holding a lock.
            let guard = self.shared.reregister_lock.lock().unwrap();
            // Get a ring_buf we write into.
            // NOTE: that we allocated at least as many `io_uring_buf`s as we
            // did buffer, so there is always a slot available for us.
            let tail = ring_tail.load(Ordering::Acquire);
            let ring_idx = tail & self.shared.tail_mask;
            let ring_buf = unsafe {
                &mut *(ptr::addr_of_mut!((*self.shared.ring_addr).__bindgen_anon_1.bufs)
                    .cast::<MaybeUninit<libc::io_uring_buf>>()
                    .add(ring_idx as usize))
            };
            log::trace!(bid = buf_idx, addr = log::as_debug!(ptr), len = self.shared.buf_size; "reregistering buffer");
            ring_buf.write(libc::io_uring_buf {
                addr: ptr.as_mut_ptr() as u64,
                len: self.shared.buf_size,
                bid: buf_idx,
                resv: 0,
            });
            ring_tail.store(tail + 1, Ordering::SeqCst);
            Mutex::unlock(guard);
        }
    }
}

#[test]
fn size_assertion() {
    assert_eq!(std::mem::size_of::<ReadBufPool>(), 8);
    assert_eq!(std::mem::size_of::<ReadBuf>(), 24);
}
