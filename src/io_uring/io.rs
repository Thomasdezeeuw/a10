use std::alloc::{self, alloc, alloc_zeroed, dealloc};
use std::marker::{PhantomData, PhantomPinned};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};
use std::{io, slice};

use crate::fd::{AsyncFd, Descriptor};
use crate::io::{Buf, BufGroupId, BufId, BufMut, BufMutSlice};
use crate::sys::{self, cq, libc, sq};
use crate::SubmissionQueue;

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

pub(crate) use crate::unix::{IoMutSlice, IoSlice};

#[derive(Debug)]
pub(crate) struct ReadBufPool {
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

/// Buffer group ID generator.
static ID: AtomicU16 = AtomicU16::new(0);

impl ReadBufPool {
    pub(crate) fn new(
        sq: SubmissionQueue,
        pool_size: u16,
        buf_size: u32,
    ) -> io::Result<ReadBufPool> {
        debug_assert!(pool_size <= 1 << 15);
        debug_assert!(pool_size.is_power_of_two());

        let ring_fd = sq.inner.shared_data().rfd.as_raw_fd();
        let id = ID.fetch_add(1, Ordering::SeqCst);

        // These allocations must be page aligned.
        let page_size = page_size();
        // NOTE: do the layout calculations first in case of an error.
        let ring_layout = alloc_layout_ring(pool_size, page_size)?;
        let bufs_layout = alloc_layout_buffers(pool_size, buf_size, page_size)?;

        // Allocation for the buffer ring, shared with the kernel.
        let ring_addr = match unsafe { alloc_zeroed(ring_layout) } {
            ring_addr if ring_addr.is_null() => return Err(io::ErrorKind::OutOfMemory.into()),
            #[allow(clippy::cast_ptr_alignment)] // Did proper alignment in `alloc_layout_ring`.
            ring_addr => ring_addr.cast::<libc::io_uring_buf_ring>(),
        };

        // Register the buffer ring with the kernel.
        let buf_register = libc::io_uring_buf_reg {
            ring_addr: ring_addr as u64,
            ring_entries: u32::from(pool_size),
            bgid: id,
            flags: 0,
            // Reserved for future use.
            resv: [0; 3],
        };
        log::trace!(ring_fd = ring_fd, buffer_group = id, size = pool_size; "registering buffer pool");

        let result = sq.inner.shared_data().register(
            libc::IORING_REGISTER_PBUF_RING,
            ptr::addr_of!(buf_register).cast(),
            1,
        );
        if let Err(err) = result {
            // SAFETY: we just allocated this above.
            unsafe { dealloc(ring_addr.cast(), ring_layout) };
            return Err(err);
        }

        // Create a `ReadBufPool` type early to manage the allocations and registration.
        let pool = ReadBufPool {
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

        if pool.bufs_addr.is_null() {
            // NOTE: dealloc and unregister happen in the `Drop` impl of
            // `ReadBufPool`.
            return Err(io::ErrorKind::OutOfMemory.into());
        }

        // Fill the buffer ring to let the kernel know what buffers are
        // available.
        let ring_tail = pool.ring_tail();
        let ring_addr = unsafe { &mut *ring_addr };
        let bufs = unsafe {
            slice::from_raw_parts_mut(
                ptr::addr_of_mut!(ring_addr.__bindgen_anon_1.bufs)
                    .cast::<MaybeUninit<libc::io_uring_buf>>(),
                pool_size as usize,
            )
        };
        for (i, ring_buf) in bufs.iter_mut().enumerate() {
            let addr = unsafe { pool.bufs_addr.add(i * buf_size as usize) };
            log::trace!(buffer_group = id, buffer = i, addr:? = addr, len = buf_size; "registering buffer");
            ring_buf.write(libc::io_uring_buf {
                addr: addr as u64,
                len: buf_size,
                bid: i as u16,
                resv: 0,
            });
        }
        ring_tail.store(pool_size, Ordering::Release);

        Ok(pool)
    }

    pub(crate) const fn buf_size(&self) -> usize {
        self.buf_size as usize
    }

    /// Returns the group id for this pool.
    pub(crate) const fn group_id(&self) -> BufGroupId {
        self.id
    }

    pub(crate) unsafe fn init_buffer(&self, id: BufId, n: u32) -> NonNull<[u8]> {
        let addr = self.bufs_addr.add(id.0 as usize * self.buf_size());
        log::trace!(buffer_group = self.id.0, buffer = id.0, addr:? = addr, len = n; "initialised buffer");
        // SAFETY: `bufs_addr` is not NULL.
        let addr = unsafe { NonNull::new_unchecked(addr) };
        NonNull::slice_from_raw_parts(addr, n as usize)
    }

    pub(crate) unsafe fn release(&self, ptr: NonNull<[u8]>) {
        let ring_tail = self.ring_tail();

        // Calculate the buffer id based on the `ptr`, which points to the start
        // of our buffer, and `bufs_addr`, which points to the start of the
        // pool, by calculating the difference and dividing it by the buffer
        // size.
        // TODO: use `sub_ptr` once stable:
        // `ptr_sub_ptr` <https://github.com/rust-lang/rust/issues/95892>.
        let buf_id = unsafe {
            usize::try_from(ptr.as_ptr().cast::<u8>().offset_from(self.bufs_addr))
                .unwrap_unchecked()
                / self.buf_size as usize
        } as u16;

        // Because we need to fill the `ring_buf` and then atomatically update
        // the `ring_tail` we do it while holding a lock.
        let guard = self.reregister_lock.lock().unwrap();
        // Get a ring_buf we write into.
        // NOTE: that we allocated at least as many `io_uring_buf`s as we
        // did buffer, so there is always a slot available for us.
        let tail = ring_tail.load(Ordering::Acquire);
        let ring_idx = tail & self.tail_mask;
        let ring_buf = unsafe {
            &mut *(ptr::addr_of_mut!((*self.ring_addr).__bindgen_anon_1.bufs)
                .cast::<MaybeUninit<libc::io_uring_buf>>()
                .add(ring_idx as usize))
        };
        log::trace!(buffer_group = self.id.0, buffer = buf_id, addr:? = ptr; "reregistering buffer");
        ring_buf.write(libc::io_uring_buf {
            addr: ptr.as_ptr().cast::<u8>() as u64,
            len: self.buf_size,
            bid: buf_id,
            resv: 0,
        });
        ring_tail.store(tail.wrapping_add(1), Ordering::SeqCst);
        drop(guard);
    }

    /// Returns the tail of buffer ring.
    fn ring_tail(&self) -> &AtomicU16 {
        unsafe {
            &*(ptr::addr_of!(((*self.ring_addr).__bindgen_anon_1.__bindgen_anon_1.tail))
                .cast::<AtomicU16>())
        }
    }
}

unsafe impl Sync for ReadBufPool {}
unsafe impl Send for ReadBufPool {}

impl Drop for ReadBufPool {
    fn drop(&mut self) {
        let page_size = page_size();

        // Unregister the buffer pool with the ring.
        let buf_register = libc::io_uring_buf_reg {
            bgid: self.id.0,
            // Unused in this call.
            ring_addr: 0,
            ring_entries: 0,
            flags: 0,
            // Reserved for future use.
            resv: [0; 3],
        };
        let result = self.sq.inner.shared_data().register(
            libc::IORING_UNREGISTER_PBUF_RING,
            ptr::addr_of!(buf_register).cast(),
            1,
        );
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

pub(crate) struct ReadOp<B>(PhantomData<*const B>);

impl<B: BufMut> sys::FdOp for ReadOp<B> {
    type Output = B;
    type Resources = B;
    type Args = u64; // Offset.

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (ptr, len) = unsafe { buf.parts_mut() };
        submission.0.opcode = libc::IORING_OP_READ as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
        submission.0.len = len;
        if let Some(buf_group) = buf.buffer_group() {
            submission.set_buffer_select(buf_group.0);
        }
    }

    fn map_ok(mut buf: Self::Resources, (buf_id, n): cq::OpReturn) -> Self::Output {
        // SAFETY: kernel just initialised the bytes for us.
        unsafe {
            buf.buffer_init(BufId(buf_id), n);
        };
        buf
    }
}

/// PhantomPinned is needed to unimplement `Unpin` (`!Unpin`), as the iovecs
/// must not be moved while the kernel is reading the submission.
pub(crate) struct ReadVectoredOp<B, const N: usize>(PhantomData<*const B>, PhantomPinned);

impl<B: BufMutSlice<N>, const N: usize> sys::FdOp for ReadVectoredOp<B, N> {
    type Output = B;
    /// `IoMutSlice` holds the buffer references used by the kernel.
    /// NOTE: we only need these in the submission, we don't have to keep around
    /// during the operation. Because of this we don't heap allocate it like we
    /// for other operations. This leaves a small duration between the
    /// submission of the entry and the submission being read by the kernel in
    /// which this future could be dropped and the kernel will read memory we
    /// don't own. However because we wake the kernel after submitting the
    /// timeout entry it's not really worth to heap allocation.
    type Resources = (B, [crate::io::IoMutSlice; N]);
    type Args = u64; // Offset.

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (_, iovecs): &mut Self::Resources,
        offset: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_READV as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: iovecs.as_ptr() as _,
        };
        submission.0.len = iovecs.len() as u32;
    }

    fn map_ok((mut bufs, _): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        // SAFETY: kernel just initialised the buffers for us.
        unsafe { bufs.set_init(n as usize) };
        bufs
    }
}

pub(crate) struct WriteOp<B>(PhantomData<*const B>);

impl<B: Buf> sys::FdOp for WriteOp<B> {
    type Output = usize;
    type Resources = B;
    type Args = u64; // Offset.

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (ptr, length) = unsafe { buf.parts() };
        submission.0.opcode = libc::IORING_OP_WRITE as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *offset };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as u64 };
        submission.0.len = length;
    }

    fn map_ok(_: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        n as usize
    }
}

/// Size of a single page, often 4096.
#[allow(clippy::cast_sign_loss)] // Page size shouldn't be negative.
fn page_size() -> usize {
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();
    *PAGE_SIZE.get_or_init(|| unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize })
}
