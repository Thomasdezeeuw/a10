use std::io;
use std::marker::PhantomData;
use std::os::fd::RawFd;

use crate::io::{BufMut, BufMutSlice, NO_OFFSET};
use crate::op::OpResult;
use crate::{kqueue, syscall, AsyncFd, SubmissionQueue};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

pub(crate) use crate::unix::{IoMutSlice, IoSlice};

pub(crate) struct ReadOp<B>(PhantomData<*const B>);

impl<B: BufMut> kqueue::FdOp for ReadOp<B> {
    type Output = B;
    type Resources = B;
    type Args = u64; // Offset.
    type OperationOutput = usize;

    fn fill_submission(fd: &AsyncFd, kevent: &mut kqueue::Event) {
        kevent.0.ident = fd.fd() as _;
        kevent.0.filter = libc::EVFILT_READ;
    }

    fn check_result(
        fd: &AsyncFd,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        let (ptr, len) = unsafe { buf.parts_mut() };
        // io_uring uses `NO_OFFSET` to issue a `read` system call,
        // otherwise it uses `pread`. We emulate the same thing.
        let result = if *offset == NO_OFFSET {
            syscall!(read(fd.fd(), ptr.cast(), len as _))
        } else {
            syscall!(pread(fd.fd(), ptr.cast(), len as _, *offset as _))
        };
        match result {
            // SAFETY: negative result is mapped to an error.
            Ok(n) => OpResult::Ok(n as usize),
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => OpResult::Again(true),
            Err(err) => OpResult::Err(err),
        }
    }

    fn map_ok(mut buf: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        // SAFETY: kernel just initialised the bytes for us.
        unsafe { buf.set_init(n) }
        buf
    }
}

pub(crate) struct ReadVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufMutSlice<N>, const N: usize> kqueue::FdOp for ReadVectoredOp<B, N> {
    type Output = B;
    type Resources = (B, [crate::io::IoMutSlice; N]);
    type Args = u64; // Offset.
    type OperationOutput = usize;

    fn fill_submission(fd: &AsyncFd, kevent: &mut kqueue::Event) {
        kevent.0.ident = fd.fd() as _;
        kevent.0.filter = libc::EVFILT_READ;
    }

    fn check_result(
        fd: &AsyncFd,
        (_, iovecs): &mut Self::Resources,
        offset: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        // io_uring uses `NO_OFFSET` to issue a `readv` system call,
        // otherwise it uses `preadv`. We emulate the same thing.
        let result = if *offset == NO_OFFSET {
            syscall!(readv(fd.fd(), iovecs.as_ptr() as _, iovecs.len() as _))
        } else {
            syscall!(preadv(
                fd.fd(),
                iovecs.as_ptr() as _,
                iovecs.len() as _,
                *offset as _
            ))
        };
        match result {
            // SAFETY: negative result is mapped to an error.
            Ok(n) => OpResult::Ok(n as usize),
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => OpResult::Again(true),
            Err(err) => OpResult::Err(err),
        }
    }

    fn map_ok((mut bufs, _): Self::Resources, n: Self::OperationOutput) -> Self::Output {
        // SAFETY: kernel just initialised the buffers for us.
        unsafe { bufs.set_init(n) };
        bufs
    }
}

pub(crate) fn close_direct_fd(fd: RawFd, sq: &SubmissionQueue) -> io::Result<()> {
    unreachable!("close_direct_fd: kqueue doesn't have direct descriptors")
}
