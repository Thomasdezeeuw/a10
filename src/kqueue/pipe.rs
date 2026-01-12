use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{io, ptr, slice};

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice};
use crate::kqueue::op::{DirectFdOp, DirectOp, impl_fd_op};
use crate::kqueue::{self, cq, sq};
use crate::net::{AddressStorage, Domain, NoAddress, OptionStorage, Protocol, SocketAddress, Type};
use crate::pipe::PipeFlag;
use crate::{AsyncFd, SubmissionQueue, fd, syscall};

pub(crate) struct PipeOp;

impl DirectOp for PipeOp {
    type Output = [AsyncFd; 2];
    type Resources = ([RawFd; 2], fd::Kind);
    type Args = PipeFlag;

    fn run(
        sq: &SubmissionQueue,
        (mut fds, kind): Self::Resources,
        _flags: Self::Args,
    ) -> io::Result<Self::Output> {
        let fd::Kind::File = kind;

        #[cfg(any(
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        syscall!(pipe2(
            ptr::from_mut(&mut fds).cast(),
            libc::O_NONBLOCK | libc::O_CLOEXEC
        ))?;
        #[cfg(not(any(
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        )))]
        syscall!(pipe(ptr::from_mut(&mut fds).cast()))?;

        // SAFETY: create the pipe above.
        let fds = unsafe {
            [
                AsyncFd::from_raw(fds[0], kind, sq.clone()),
                AsyncFd::from_raw(fds[1], kind, sq.clone()),
            ]
        };

        #[cfg(not(any(
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        )))]
        {
            syscall!(fcntl(fds[0].fd(), libc::F_SETFL, libc::O_NONBLOCK))?;
            syscall!(fcntl(fds[0].fd(), libc::F_SETFD, libc::FD_CLOEXEC))?;
            syscall!(fcntl(fds[1].fd(), libc::F_SETFL, libc::O_NONBLOCK))?;
            syscall!(fcntl(fds[1].fd(), libc::F_SETFD, libc::FD_CLOEXEC))?;
        }

        Ok(fds)
    }
}
