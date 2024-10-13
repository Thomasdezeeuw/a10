use std::io;
use std::marker::PhantomData;

use crate::fd::{AsyncFd, Descriptor};
use crate::io::{BufMut, NO_OFFSET};
use crate::op::OpResult;
use crate::{sys, syscall};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

pub(crate) struct Read<B>(PhantomData<*const B>);

impl<B: BufMut> sys::Op for Read<B> {
    type Output = B;
    type Resources = B;
    type Args = u64; // Offset.
    type OperationOutput = usize;

    fn fill_submission<D: Descriptor>(fd: &AsyncFd<D>, kevent: &mut sys::Event) {
        kevent.0.ident = fd.fd() as _;
        kevent.0.filter = libc::EVFILT_READ;
    }

    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
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
            Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => OpResult::Again,
            Err(err) => OpResult::Err(err),
        }
    }

    fn map_ok(mut buf: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        // SAFETY: kernel just initialised the bytes for us.
        unsafe { buf.set_init(n) }
        buf
    }
}
