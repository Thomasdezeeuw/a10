use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{io, ptr, slice};

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice};
use crate::kqueue::op::{impl_fd_op, DirectFdOp, DirectOp};
use crate::kqueue::{self, cq, sq};
use crate::net::{AddressStorage, Domain, NoAddress, OptionStorage, Protocol, SocketAddress, Type};
use crate::{fd, syscall, AsyncFd, SubmissionQueue};

pub(crate) use crate::unix::MsgHeader;

pub(crate) struct SocketOp;

impl DirectOp for SocketOp {
    type Output = AsyncFd;
    type Resources = fd::Kind;
    type Args = (Domain, Type, Protocol);

    fn run(
        sq: &SubmissionQueue,
        kind: Self::Resources,
        (domain, r#type, protocol): Self::Args,
    ) -> io::Result<Self::Output> {
        let fd::Kind::File = kind;

        let r#type = r#type.0 as libc::c_int;
        #[cfg(any(
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        let r#type = r#type | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;

        let socket = syscall!(socket(domain.0, r#type, protocol.0 as _))?;
        // SAFETY: just created the socket above.
        let fd = unsafe { AsyncFd::from_raw_fd(socket, sq.clone()) };

        // Mimic std lib and set SO_NOSIGPIPE on apple systems.
        #[cfg(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "tvos",
            target_os = "visionos",
            target_os = "watchos",
        ))]
        syscall!(setsockopt(
            socket,
            libc::SOL_SOCKET,
            libc::SO_NOSIGPIPE,
            &1 as *const libc::c_int as *const libc::c_void,
            size_of::<libc::c_int>() as libc::socklen_t
        ))?;

        // Apple systems don't have SOCK_NONBLOCK or SOCK_CLOEXEC.
        #[cfg(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "tvos",
            target_os = "visionos",
            target_os = "watchos",
        ))]
        {
            syscall!(fcntl(socket, libc::F_SETFL, libc::O_NONBLOCK))?;
            syscall!(fcntl(socket, libc::F_SETFD, libc::FD_CLOEXEC))?;
        }

        Ok(fd)
    }
}

pub(crate) struct BindOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> DirectFdOp for BindOp<A> {
    type Output = ();
    type Resources = AddressStorage<Box<A::Storage>>;
    type Args = ();

    fn run(fd: &AsyncFd, address: Self::Resources, (): Self::Args) -> io::Result<Self::Output> {
        let (ptr, length) = unsafe { A::as_ptr(&address.0) };
        let socket = syscall!(bind(fd.fd(), ptr, length))?;
        Ok(())
    }
}

impl_fd_op!(BindOp<A>);

pub(crate) struct ListenOp;

impl DirectFdOp for ListenOp {
    type Output = ();
    type Resources = ();
    type Args = libc::c_int; // backlog.

    fn run(fd: &AsyncFd, (): Self::Resources, backlog: Self::Args) -> io::Result<Self::Output> {
        let socket = syscall!(listen(fd.fd(), backlog))?;
        Ok(())
    }
}

impl_fd_op!(ListenOp);
