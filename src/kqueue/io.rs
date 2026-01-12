use std::io;
use std::marker::PhantomData;
use std::os::fd::RawFd;

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice, NO_OFFSET};
use crate::kqueue::fd::OpKind;
use crate::kqueue::op::{DirectOp, FdOp, FdOpExtract};
use crate::kqueue::{self, Event, cq, sq};
use crate::{AsyncFd, SubmissionQueue, asan, fd, msan, syscall};

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
        fd: &AsyncFd,
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

    const OP_KIND: OpKind = OpKind::Read;

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
        (buf, n as usize)
    }
}

pub(crate) struct WriteVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufSlice<N>, const N: usize> FdOp for WriteVectoredOp<B, N> {
    type Output = usize;
    type Resources = (B, [crate::io::IoSlice; N]);
    type Args = u64; // Offset.
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Read;

    fn try_run(
        fd: &AsyncFd,
        (bufs, iovecs): &mut Self::Resources,
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
        (bufs, n as usize)
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
