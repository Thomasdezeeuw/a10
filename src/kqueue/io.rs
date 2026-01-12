use std::io;
use std::marker::PhantomData;

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice, NO_OFFSET};
use crate::kqueue::fd::OpKind;
use crate::kqueue::op::FdOp;
use crate::kqueue::{self, cq, sq, Event};
use crate::{asan, fd, msan, syscall, AsyncFd, SubmissionQueue};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

pub(crate) use crate::unix::{IoMutSlice, IoSlice};

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
        let (ptr, len) = unsafe { buf.parts_mut() };
        // io_uring uses `NO_OFFSET` to issue a `read` system call, otherwise it
        // uses `pread`. We emulate the same thing.
        if *offset == NO_OFFSET {
            syscall!(read(fd.fd(), ptr.cast(), len as _))
        } else {
            syscall!(pread(fd.fd(), ptr.cast(), len as _, *offset as _))
        }
    }

    fn map_ok(fd: &AsyncFd, mut buf: Self::Resources, n: Self::OperationOutput) -> Self::Output {
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
        (buf, iovecs): &mut Self::Resources,
        offset: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        // io_uring uses `NO_OFFSET` to issue a `read` system call, otherwise it
        // uses `pread`. We emulate the same thing.
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
        fd: &AsyncFd,
        (mut bufs, _): Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::Output {
        // SAFETY: kernel just initialised the buffers for us.
        unsafe { bufs.set_init(n as _) };
        bufs
    }
}
