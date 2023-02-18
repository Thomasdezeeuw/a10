//! Module with read buffer pool.
//!
//! See [`ReadBufPool`].

use std::alloc::{self, alloc, alloc_zeroed, dealloc};
use std::borrow::{Borrow, BorrowMut};
use std::mem::{size_of, MaybeUninit};
use std::ops::{Deref, DerefMut};
use std::os::fd::AsRawFd;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::{fmt, io, slice};

use crate::io::{Buf, BufMut};
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
///
/// This is a special buffer pool that shares its buffers with the kernel. The
/// buffer pool is used by the kernel in `read(2)` and `recv(2)` like calls.
/// Instead of user space having to select a buffer before issueing the read
/// call, the kernel will select a buffer from the pool when it's ready for
/// reading. This avoids the need to have as many buffers as concurrent read
/// calls.
///
/// As a result of this the returned buffer, [`ReadBuf`], is somewhat limited.
/// For example it can't grow beyond the pool's buffer size. However it can be
/// used in write calls like any other buffer.
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
    /// `buf_size` is the maximum capacity of the buffer. Note that buffer can't
    /// grow beyond this capacity.
    #[doc(alias = "IORING_REGISTER_PBUF_RING")]
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
    /// This can only be used in read I/O operation, such as [`AsyncFd::read`],
    /// but it won't yet select a buffer to use. This is done by the kernel once
    /// it actually has data to write into the buffer. Before it's used in a
    /// read call the returned buffer will be empty and can't be resized, it's
    /// effecitvely useless before a read call.
    ///
    /// [`AsyncFd::read`]: crate::AsyncFd::read
    pub fn get(&self) -> ReadBuf {
        ReadBuf {
            shared: self.shared.clone(),
            owned: None,
        }
    }

    /// Returns the group id for this pool.
    pub(crate) fn group_id(&self) -> BufGroupId {
        self.shared.id
    }

    /// Initialise a new buffer with `index` with `len` size.
    ///
    /// # Safety
    ///
    /// The provided index must come from the kernel, reusing the same index
    /// will cause data races.
    pub(crate) unsafe fn new_buffer(&self, index: BufIdx, len: u32) -> ReadBuf {
        let owned = if len == 0 && index.0 == 0 {
            // If we read 0 bytes it means the kernel didn't actually allocate a
            // buffer.
            None
        } else {
            let data = self
                .shared
                .bufs_addr
                .add(index.0 as usize * self.shared.buf_size as usize);
            log::trace!(bid = index.0, addr = log::as_debug!(data), len = len; "kernel initialised buffer");
            // SAFETY: `bufs_addr` is not NULL.
            let data = unsafe { NonNull::new_unchecked(data) };
            Some(NonNull::slice_from_raw_parts(data, len as usize))
        };
        ReadBuf {
            shared: self.shared.clone(),
            owned,
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
///
/// Before a read system call, this will be empty and can't be resize. This is
/// really only useful in a call to a `read(2)` like system call.
///
/// # Notes
///
/// Do **not** use the [`BufMut`] implementation of this buffer to write into
/// it, it's a specialised implementation that is invalid use to outside of the
/// A10 crate.
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

    /// Returns true if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.owned.map_or(true, |ptr| ptr.len() == 0)
    }

    /// Returns itself as slice.
    pub fn as_slice(&self) -> &[u8] {
        self
    }

    /// Returns itself as mutable slice.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self
    }

    /// Truncate the buffer to `len` bytes.
    ///
    /// If the buffer is shorter then `len` bytes this does nothing.
    pub fn truncate(&mut self, len: usize) {
        if let Some(ptr) = self.owned {
            if len > ptr.len() {
                return;
            }
            self.owned = Some(NonNull::slice_from_raw_parts(ptr.as_non_null_ptr(), len));
        }
    }

    /// Clear the buffer.
    ///
    /// # Notes
    ///
    /// This is not the same as returning the buffer to the buffer pool, for
    /// that use [`ReadBuf::release`].
    pub fn clear(&mut self) {
        if let Some(ptr) = self.owned {
            self.owned = Some(NonNull::slice_from_raw_parts(ptr.as_non_null_ptr(), 0));
        }
    }

    /// Set the length of the buffer to `new_len`.
    ///
    /// # Safety
    ///
    /// The caller must ensure `new_len` bytes are initialised and that
    /// `new_len` is not larger than the buffer's capacity.
    pub unsafe fn set_len(&mut self, new_len: usize) {
        debug_assert!(new_len <= self.shared.buf_size as usize);
        if let Some(ptr) = self.owned {
            self.owned = Some(NonNull::slice_from_raw_parts(
                ptr.as_non_null_ptr(),
                new_len,
            ));
        }
    }

    /// Appends `other` to `self`.
    ///
    /// If `self` doesn't have sufficient capacity it will return `Err(())` and
    /// will not append anything.
    pub fn extend_from_slice(&mut self, other: &[u8]) -> Result<(), ()> {
        if let Some(ptr) = self.owned {
            let new_len = ptr.len() + other.len();
            if new_len > self.shared.buf_size as usize {
                return Err(());
            }

            // SAFETY: the source, destination and len are all valid.
            // NOTE: we can't use `copy_from_nonoverlapping` as we can't
            // guarantee that `self` and `other` are not overlapping.
            unsafe {
                ptr.as_mut_ptr()
                    .add(ptr.len())
                    .copy_from(other.as_ptr(), other.len());
            }
            self.owned = Some(NonNull::slice_from_raw_parts(
                ptr.as_non_null_ptr(),
                new_len,
            ));
            Ok(())
        } else {
            Err(())
        }
    }

    /// Returns the remaining spare capacity of the buffer.
    pub fn spare_capacity_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        if let Some(ptr) = self.owned {
            let unused_len = self.shared.buf_size as usize - ptr.len();
            // SAFETY: this won't overflow `isize`.
            let data = unsafe { ptr.as_mut_ptr().add(ptr.len()) };
            // SAFETY: the pointer and length are correct.
            unsafe { slice::from_raw_parts_mut(data.cast(), unused_len) }
        } else {
            &mut []
        }
    }

    /// Release the buffer back to the buffer pool.
    ///
    /// If `self` isn't an allocated buffer this does nothing.
    ///
    /// The buffer can still be used in a `read(2)` system call, it's reset to
    /// the state as if it was just created by calling [`ReadBufPool::get`].
    ///
    /// # Notes
    ///
    /// This is automatically called in the `Drop` implementation.
    pub fn release(&mut self) {
        if let Some(ptr) = self.owned.take() {
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

/// The implementation for `ReadBuf` is a special one as we don't actually pass
/// a "real" buffer. Instead we pass special flags to the kernel that allows it
/// to select a buffer from the connected [`ReadBufPool`] once the actual read
/// operation starts.
///
/// If the `ReadBuf` is used a second time in a read call this changes as at
/// that point it owns an actual buffer. At that point it will behave more like
/// the `Vec<u8>` implementation is that it only uses the unused capacity, so
/// any bytes already in the buffer will be untouched.
///
/// To revert to the original behaviour of allowing the kernel to select a
/// buffer call [`ReadBuf::release`] first.
///
/// Note that this can **not** be used in vectored I/O as a part of the
/// [`ButMutSlice`] trait.
///
/// [`ButMutSlice`]: crate::io::BufMutSlice
unsafe impl BufMut for ReadBuf {
    unsafe fn parts(&mut self) -> (*mut u8, u32) {
        if let Some(ptr) = self.owned {
            let len = self.shared.buf_size - ptr.len() as u32;
            (ptr.as_mut_ptr().add(ptr.len()), len)
        } else {
            (ptr::null_mut(), self.shared.buf_size)
        }
    }

    unsafe fn set_init(&mut self, _: usize) {
        panic!("Don't call a10::Buf::set_init");
    }

    fn buffer_group(&self) -> Option<BufGroupId> {
        if self.owned.is_none() {
            Some(self.shared.id)
        } else {
            // Already have an allocated buffer, don't need another one.
            None
        }
    }

    unsafe fn buffer_init(&mut self, idx: BufIdx, n: u32) {
        if let Some(ptr) = self.owned {
            // We shouldn't be assigned another buffer, we should be resizing
            // the current one.
            debug_assert!(idx.0 == 0);
            let len = ptr.len() + n as usize;
            self.owned = Some(NonNull::slice_from_raw_parts(ptr.as_non_null_ptr(), len));
        } else {
            let data = self
                .shared
                .bufs_addr
                .add(idx.0 as usize * self.shared.buf_size as usize);
            log::trace!(bid = idx.0, addr = log::as_debug!(data), len = n; "kernel initialised buffer");
            // SAFETY: `bufs_addr` is not NULL.
            let data = unsafe { NonNull::new_unchecked(data) };
            self.owned = Some(NonNull::slice_from_raw_parts(data, n as usize));
        }
    }
}

// SAFETY: `ReadBuf` manages the allocation of the bytes once it's assigned a
// buffer, so as long as it's alive, so is the slice of bytes.
unsafe impl Buf for ReadBuf {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let slice = self.as_slice();
        (slice.as_ptr().cast(), slice.len() as u32)
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

impl AsMut<[u8]> for ReadBuf {
    fn as_mut(&mut self) -> &mut [u8] {
        self
    }
}

impl Borrow<[u8]> for ReadBuf {
    fn borrow(&self) -> &[u8] {
        self
    }
}

impl BorrowMut<[u8]> for ReadBuf {
    fn borrow_mut(&mut self) -> &mut [u8] {
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
        self.release();
    }
}

#[test]
fn size_assertion() {
    assert_eq!(std::mem::size_of::<ReadBufPool>(), 8);
    assert_eq!(std::mem::size_of::<ReadBuf>(), 24);
}
