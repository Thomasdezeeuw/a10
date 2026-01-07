use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{io, ptr, slice};

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice};
use crate::kqueue::{self, cq, sq};
use crate::net::{AddressStorage, Domain, NoAddress, OptionStorage, Protocol, SocketAddress, Type};
use crate::{fd, syscall, AsyncFd, SubmissionQueue};

pub(crate) use crate::unix::MsgHeader;

pub(crate) struct SocketOp;

impl kqueue::op::Op for SocketOp {
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
