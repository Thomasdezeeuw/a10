use std::alloc::{self, alloc, dealloc};
use std::io;
use std::marker::PhantomData;
use std::os::fd::RawFd;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::io::{Buf, BufMut, BufMutParts, BufMutSlice, BufSlice, NO_OFFSET, ReadBuf};
use crate::kqueue::fd::OpKind;
use crate::kqueue::op::{DirectOp, FdOp, FdOpExtract};
use crate::{AsyncFd, SubmissionQueue, fd, syscall};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use crate::unix::{IoMutSlice, IoSlice};
pub(crate) use std::io::*;

#[derive(Debug)]
pub(crate) struct ReadBufPool {
    /// Number of buffers.
    pool_size: u16,
    /// Size of the buffers.
    buf_size: u32,
    /// Address of the allocation the buffers, see `alloc_layout_buffers`.
    bufs_addr: NonNull<u8>,
    /// Buffer availablility bitset.
    available: Box<[AtomicUsize]>,
}

impl ReadBufPool {
    pub(crate) fn new(
        _: SubmissionQueue,
        pool_size: u16,
        buf_size: u32,
    ) -> io::Result<ReadBufPool> {
        // NOTE: do the layout calculations first in case of an error.
        let bufs_layout = alloc_layout_buffers(pool_size, buf_size, page_size())?;
        let Some(bufs_addr) = NonNull::new(unsafe { alloc(bufs_layout) }) else {
            return Err(io::ErrorKind::OutOfMemory.into());
        };

        let mut available_size = pool_size / usize::BITS as u16;
        if !pool_size.is_multiple_of(usize::BITS as u16) {
            available_size += 1;
        }
        // SAFETY: AtomicUsize has the same layout as usize and all zero is a
        // valid value for both.
        let mut available: Box<[AtomicUsize]> =
            unsafe { Box::new_zeroed_slice(available_size as usize).assume_init() };
        if available_size % usize::BITS as u16 != 0 {
            // Mark the addition bits as unavailable.
            *available[available_size as usize - 1].get_mut() =
                ((1 << (available_size * usize::BITS as u16) - pool_size) - 1)
                    << (pool_size % usize::BITS as u16);
        }

        Ok(ReadBufPool {
            pool_size,
            buf_size,
            bufs_addr,
            available,
        })
    }

    pub(crate) const fn buf_size(&self) -> usize {
        self.buf_size as usize
    }

    pub(crate) fn get_buf(&self) -> Option<(*mut u8, u32)> {
        for (idx, available) in self.available.iter().enumerate() {
            // SAFETY: Relaxed ordering is acceptable here because when we
            // actually attempt to set the bit below (the `fetch_or`) we use the
            // correct Acquire ordering.
            let mut value = available.load(Ordering::Relaxed);
            let mut i = value.trailing_ones();
            while i < usize::BITS {
                // Attempt to set the bit, claiming the slot.
                value = available.fetch_or(1 << i, Ordering::AcqRel);
                // Another thread could have attempted to set the same bit we're
                // setting, so we need to make sure we actually set the bit
                // (i.e. check if was unset in the previous state).
                if is_unset(value, i as usize) {
                    let buf_id = (idx * usize::BITS as usize) + i as usize;
                    let len = self.buf_size();
                    let ptr = unsafe { self.bufs_addr.byte_offset((len * buf_id).cast_signed()) };
                    return Some((ptr.as_ptr(), len as u32));
                }
                i += (value >> i).trailing_ones();
            }
        }
        None
    }

    pub(crate) unsafe fn release(&self, ptr: NonNull<[u8]>) {
        // Calculate the buffer id based on the `ptr`, which points to the start
        // of our buffer, and `bufs_addr`, which points to the start of the
        // pool, by calculating the difference and dividing it by the buffer
        // size.
        let buf_id = unsafe {
            ptr.cast::<u8>().offset_from(self.bufs_addr).cast_unsigned() / (self.buf_size as usize)
        };

        let idx = buf_id / usize::BITS as usize;
        let n = buf_id % usize::BITS as usize;
        let old_value = self.available[idx].fetch_and(!(1 << n), Ordering::AcqRel);
        debug_assert!(!is_unset(old_value, n));
    }
}

impl ReadBuf {
    pub(crate) fn parts_sys(&mut self) -> BufMutParts {
        let (ptr, len) = if let Some((ptr, len)) = self.shared.get_buf() {
            self.owned = unsafe {
                let ptr = NonNull::new_unchecked(ptr);
                Some(NonNull::slice_from_raw_parts(ptr, 0))
            };
            (ptr, len)
        } else {
            (ptr::null_mut(), 0)
        };
        BufMutParts::Pool(PoolBufParts { ptr, len })
    }
}

pub(crate) struct PoolBufParts {
    ptr: *mut u8,
    len: u32,
}

impl BufMutParts {
    pub(crate) fn pool_ptr(self) -> io::Result<(*mut u8, u32, bool)> {
        match self {
            BufMutParts::Buf { ptr, len } if len == 0 => {
                Err(io::Error::from_raw_os_error(libc::ENOBUFS))
            }
            BufMutParts::Buf { ptr, len } => Ok((ptr, len, false)),
            BufMutParts::Pool(PoolBufParts { ptr, len }) => Ok((ptr, len, true)),
        }
    }
}

unsafe impl Sync for ReadBufPool {}
unsafe impl Send for ReadBufPool {}

impl Drop for ReadBufPool {
    fn drop(&mut self) {
        let page_size = page_size();
        // SAFETY: created this layout in ReadBufPool::new and didn't fail,
        // so it's still valid here.
        let layout = alloc_layout_buffers(self.pool_size, self.buf_size, page_size).unwrap();
        // SAFETY: we allocated this in `new`, so it's safe to
        // deallocate for us.
        unsafe { dealloc(self.bufs_addr.as_ptr(), layout) }
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

/// Size of a single page.
#[allow(clippy::cast_sign_loss)] // Page size shouldn't be negative.
fn page_size() -> usize {
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

/// Returns true if bit `n` is not set in `value`. `n` is zero indexed, i.e.
/// must be in the range 0..usize::BITS (64).
const fn is_unset(value: usize, n: usize) -> bool {
    ((value >> n) & 1) == 0
}

pub(crate) struct ReadOp<B>(PhantomData<*const B>);

impl<B: BufMut> FdOp for ReadOp<B> {
    type Output = B;
    type Resources = B;
    type Args = u64; // Offset.
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Read;

    fn try_run(
        fd: &AsyncFd,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let (ptr, len, is_pool) = buf.parts().pool_ptr()?;
        // io_uring uses `NO_OFFSET` to issue a `read` system call, otherwise it
        // uses `pread`. We emulate the same thing.
        let res = if *offset == NO_OFFSET {
            syscall!(read(fd.fd(), ptr.cast(), len as _))
        } else {
            syscall!(pread(fd.fd(), ptr.cast(), len as _, *offset as _))
        };
        if res.is_err() && is_pool {
            buf.release();
        }
        res
    }

    fn map_ok(_: &AsyncFd, mut buf: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        // SAFETY: kernel just initialised the bytes for us.
        unsafe { buf.set_init(n as _) };
        buf
    }
}

pub(crate) struct ReadVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufMutSlice<N>, const N: usize> FdOp for ReadVectoredOp<B, N> {
    type Output = B;
    type Resources = (B, [crate::io::IoMutSlice; N]);
    type Args = u64; // Offset.
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Read;

    fn try_run(
        fd: &AsyncFd,
        (_, iovecs): &mut Self::Resources,
        offset: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        // io_uring uses `NO_OFFSET` to issue a `readv` system call, otherwise
        // it uses `preadv`. We emulate the same thing.
        if *offset == NO_OFFSET {
            syscall!(readv(fd.fd(), iovecs.as_ptr() as _, iovecs.len() as _))
        } else {
            syscall!(preadv(
                fd.fd(),
                iovecs.as_ptr() as _,
                iovecs.len() as _,
                *offset as _
            ))
        }
    }

    fn map_ok(
        _: &AsyncFd,
        (mut bufs, _): Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::Output {
        // SAFETY: kernel just initialised the buffers for us.
        unsafe { bufs.set_init(n as _) };
        bufs
    }
}

pub(crate) struct WriteOp<B>(PhantomData<*const B>);

impl<B: Buf> FdOp for WriteOp<B> {
    type Output = usize;
    type Resources = B;
    type Args = u64; // Offset.
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Write;

    fn try_run(
        fd: &AsyncFd,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let (ptr, len) = unsafe { buf.parts() };
        // io_uring uses `NO_OFFSET` to issue a `write` system call, otherwise
        // it uses `pwrite`. We emulate the same thing.
        if *offset == NO_OFFSET {
            syscall!(write(fd.fd(), ptr.cast(), len as _))
        } else {
            syscall!(pwrite(fd.fd(), ptr.cast(), len as _, *offset as _))
        }
    }

    fn map_ok(fd: &AsyncFd, buf: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        Self::map_ok_extract(fd, buf, n).1
    }
}

impl<B: Buf> FdOpExtract for WriteOp<B> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        buf: Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (buf, n.cast_unsigned())
    }
}

pub(crate) struct WriteVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufSlice<N>, const N: usize> FdOp for WriteVectoredOp<B, N> {
    type Output = usize;
    type Resources = (B, [crate::io::IoSlice; N]);
    type Args = u64; // Offset.
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Write;

    fn try_run(
        fd: &AsyncFd,
        (_, iovecs): &mut Self::Resources,
        offset: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        // io_uring uses `NO_OFFSET` to issue a `writev` system call, otherwise
        // it uses `pwritev`. We emulate the same thing.
        if *offset == NO_OFFSET {
            syscall!(writev(fd.fd(), iovecs.as_ptr() as _, iovecs.len() as _))
        } else {
            syscall!(pwritev(
                fd.fd(),
                iovecs.as_ptr() as _,
                iovecs.len() as _,
                *offset as _
            ))
        }
    }

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        Self::map_ok_extract(fd, resources, n).1
    }
}

impl<B: BufSlice<N>, const N: usize> FdOpExtract for WriteVectoredOp<B, N> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        (bufs, _): Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (bufs, n.cast_unsigned())
    }
}

pub(crate) struct CloseOp;

impl DirectOp for CloseOp {
    type Output = ();
    type Resources = ();
    type Args = (RawFd, fd::Kind);

    fn run(
        _: &SubmissionQueue,
        (): Self::Resources,
        (fd, kind): Self::Args,
    ) -> io::Result<Self::Output> {
        let fd::Kind::File = kind;

        syscall!(close(fd))?;
        Ok(())
    }
}
