use std::io;
use std::marker::PhantomData;
use std::os::fd::RawFd;
use std::ptr::NonNull;

use crate::fd::{AsyncFd, Descriptor};
use crate::io::{
    Buf, BufGroupId, BufId, BufMut, BufMutSlice, BufSlice, Buffer, SpliceDirection, NO_OFFSET,
};
use crate::op::{FdOpExtract, OpResult};
use crate::{kqueue, syscall, SubmissionQueue};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

pub(crate) use crate::unix::{IoMutSlice, IoSlice};

#[derive(Debug)]
pub(crate) struct ReadBufPool {
    // TODO(port).
}

impl ReadBufPool {
    pub(crate) fn new(
        sq: SubmissionQueue,
        pool_size: u16,
        buf_size: u32,
    ) -> io::Result<ReadBufPool> {
        // TODO(port).
        todo!("ReadBufPool::new")
    }

    pub(crate) const fn buf_size(&self) -> usize {
        // TODO(port).
        todo!("ReadBufPool::buf_size")
    }

    pub(crate) const fn group_id(&self) -> BufGroupId {
        // TODO(port).
        todo!("ReadBufPool::group_id")
    }

    pub(crate) unsafe fn init_buffer(&self, id: BufId, n: u32) -> NonNull<[u8]> {
        // TODO(port).
        todo!("ReadBufPool::init_buffer")
    }

    pub(crate) unsafe fn release(&self, ptr: NonNull<[u8]>) {
        // TODO(port).
        todo!("ReadBufPool::release")
    }
}

pub(crate) struct ReadOp<B>(PhantomData<*const B>);

impl<B: BufMut> kqueue::FdOp for ReadOp<B> {
    type Output = B;
    type Resources = Buffer<B>;
    type Args = u64; // Offset.
    type OperationOutput = usize;

    fn fill_submission<D: Descriptor>(fd: &AsyncFd<D>, kevent: &mut kqueue::Event) {
        kevent.0.ident = fd.fd() as _;
        kevent.0.filter = libc::EVFILT_READ;
    }

    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        buf: &mut Self::Resources,
        offset: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        let (ptr, len) = unsafe { buf.buf.parts_mut() };
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
        unsafe { buf.buf.set_init(n) }
        buf.buf
    }
}

pub(crate) struct ReadVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufMutSlice<N>, const N: usize> kqueue::FdOp for ReadVectoredOp<B, N> {
    type Output = B;
    type Resources = (B, Box<[crate::io::IoMutSlice; N]>);
    type Args = u64; // Offset.
    type OperationOutput = usize;

    fn fill_submission<D: Descriptor>(fd: &AsyncFd<D>, kevent: &mut kqueue::Event) {
        kevent.0.ident = fd.fd() as _;
        kevent.0.filter = libc::EVFILT_READ;
    }

    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
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

pub(crate) struct WriteOp<B>(PhantomData<*const B>);

impl<B: Buf> kqueue::FdOp for WriteOp<B> {
    type Output = usize;
    type Resources = Buffer<B>;
    type Args = u64; // Offset.
    type OperationOutput = usize;

    fn fill_submission<D: Descriptor>(fd: &AsyncFd<D>, kevent: &mut kqueue::Event) {
        // TODO(port): implement.
        todo!("WriteOp::fill_submission");
    }

    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        _: &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        // TODO(port): implement.
        todo!("WriteOp::check_result");
    }

    fn map_ok(_: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        // TODO(port): implement.
        todo!("WriteOp::map_ok");
    }
}

impl<B: Buf> FdOpExtract for WriteOp<B> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract<D: Descriptor>(
        _: &AsyncFd<D>,
        _: Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::ExtractOutput {
        // TODO(port): implement.
        todo!("WriteOp::map_ok_extract");
    }
}

pub(crate) struct WriteVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufSlice<N>, const N: usize> kqueue::FdOp for WriteVectoredOp<B, N> {
    type Output = usize;
    type Resources = (B, Box<[crate::io::IoSlice; N]>);
    type Args = u64; // Offset.
    type OperationOutput = usize;

    fn fill_submission<D: Descriptor>(fd: &AsyncFd<D>, kevent: &mut kqueue::Event) {
        // TODO(port): implement.
        todo!("WriteVectoredOp::fill_submission");
    }

    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        _: &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        // TODO(port): implement.
        todo!("WriteVectoredOp::check_result");
    }

    fn map_ok(_: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        // TODO(port): implement.
        todo!("WriteVectoredOp::map_ok");
    }
}

impl<B: BufSlice<N>, const N: usize> FdOpExtract for WriteVectoredOp<B, N> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract<D: Descriptor>(
        _: &AsyncFd<D>,
        _: Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::ExtractOutput {
        // TODO(port): implement.
        todo!("WriteVectoredOp::map_ok_extract");
    }
}

pub(crate) struct SpliceOp;

impl kqueue::FdOp for SpliceOp {
    type Output = usize;
    type Resources = ();
    type Args = (RawFd, SpliceDirection, u64, u64, u32, libc::c_int); // target, direction, off_in, off_out, len, flags
    type OperationOutput = usize;

    fn fill_submission<D: Descriptor>(fd: &AsyncFd<D>, kevent: &mut kqueue::Event) {
        // TODO(port): implement.
        todo!("SpliceOp::fill_submission");
    }

    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        _: &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        // TODO(port): implement.
        todo!("SpliceOp::check_result");
    }

    fn map_ok(_: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        // TODO(port): implement.
        todo!("SpliceOp::map_ok");
    }
}

pub(crate) struct CloseOp<D>(PhantomData<*const D>);

impl<D: Descriptor> kqueue::Op for CloseOp<D> {
    type Output = ();
    type Resources = ();
    type Args = RawFd;
    type OperationOutput = usize;

    fn fill_submission(kevent: &mut kqueue::Event) {
        // TODO(port): implement.
        todo!("CloseOp::fill_submission");
    }

    fn check_result(
        _: &mut Self::Resources,
        _: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput> {
        // TODO(port): implement.
        todo!("CloseOp::check_result");
    }

    fn map_ok(
        sq: &crate::SubmissionQueue,
        _: Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::Output {
        // TODO(port): implement.
        todo!("CloseOp::map_ok");
    }
}

pub(crate) fn close_file_fd(fd: RawFd, kevent: &mut kqueue::Event) {
    // Since we don't need an event to close an fd we trigger the submission
    // using a user event.
    kevent.0.filter = libc::EVFILT_USER;
    kevent.0.flags = libc::EV_ADD | libc::EV_RECEIPT;
    kevent.0.fflags = libc::NOTE_TRIGGER;
}
