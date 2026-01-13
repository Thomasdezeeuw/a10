use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{io, ptr, slice};

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice};
use crate::kqueue::fd::OpKind;
use crate::kqueue::op::{
    DirectFdOp, DirectFdOpExtract, DirectOp, FdOp, impl_fd_op, impl_fd_op_extract,
};
use crate::kqueue::{self, cq, sq};
use crate::net::{
    AcceptFlag, AddressStorage, Domain, Level, NoAddress, Opt, OptionStorage, Protocol,
    SocketAddress, Type, option,
};
use crate::{AsyncFd, SubmissionQueue, fd, syscall};

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
    type Resources = AddressStorage<A::Storage>;
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

pub(crate) struct AcceptOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> FdOp for AcceptOp<A> {
    type Output = (AsyncFd, A);
    type Resources = AddressStorage<(MaybeUninit<A::Storage>, libc::socklen_t)>;
    type Args = AcceptFlag;
    type OperationOutput = AsyncFd;

    const OP_KIND: OpKind = OpKind::Read;

    fn try_run(
        lfd: &AsyncFd,
        resources: &mut Self::Resources,
        flags: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let (ptr, length) = unsafe { A::as_mut_ptr(&mut (resources.0).0) };
        let address_length = &mut (resources.0).1;
        *address_length = length;

        #[cfg(any(
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        let fd = syscall!(accept4(
            lfd.fd(),
            ptr,
            address_length,
            libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC
        ))?;

        #[cfg(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "tvos",
            target_os = "visionos",
            target_os = "watchos",
        ))]
        let fd = syscall!(accept(lfd.fd(), ptr, address_length))?;

        // SAFETY: the accept operation ensures that `fd` is valid.
        let fd = unsafe { AsyncFd::from_raw(fd, lfd.kind(), lfd.sq().clone()) };

        #[cfg(any(
            target_os = "ios",
            target_os = "macos",
            target_os = "tvos",
            target_os = "visionos",
            target_os = "watchos",
        ))]
        {
            syscall!(fcntl(fd.fd(), libc::F_SETFD, libc::FD_CLOEXEC))?;
            syscall!(fcntl(fd.fd(), libc::F_SETFL, libc::O_NONBLOCK))?;
        }

        Ok(fd)
    }

    fn map_ok(_: &AsyncFd, resources: Self::Resources, fd: Self::OperationOutput) -> Self::Output {
        // SAFETY: the kernel has written the address for us.
        let address = unsafe { A::init((resources.0).0, (resources.0).1) };
        (fd, address)
    }
}

pub(crate) struct SocketOptionOp<T>(PhantomData<*const T>);

impl<T> DirectFdOp for SocketOptionOp<T> {
    type Output = T;
    type Resources = MaybeUninit<T>;
    type Args = (Level, Opt);

    fn run(
        fd: &AsyncFd,
        mut value: Self::Resources,
        (level, optname): Self::Args,
    ) -> io::Result<Self::Output> {
        let mut optlen = size_of::<T>() as libc::socklen_t;
        syscall!(getsockopt(
            fd.fd(),
            level.0.cast_signed(),
            optname.0.cast_signed(),
            value.as_mut_ptr().cast(),
            &mut optlen,
        ))?;
        // SAFETY: the kernel initialised the value for us as part of the
        // getsockopt call.
        debug_assert!(optlen == (size_of::<T>() as libc::socklen_t));
        Ok(unsafe { MaybeUninit::assume_init(value) })
    }
}

impl_fd_op!(SocketOptionOp<T>);

pub(crate) struct SocketOption2Op<T>(PhantomData<*const T>);

impl<T: option::Get> DirectFdOp for SocketOption2Op<T> {
    type Output = T::Output;
    type Resources = OptionStorage<MaybeUninit<T::Storage>>;
    type Args = (Level, Opt);

    fn run(
        fd: &AsyncFd,
        mut value: Self::Resources,
        (level, optname): Self::Args,
    ) -> io::Result<Self::Output> {
        let (optval, mut optlen) = unsafe { T::as_mut_ptr(&mut value.0) };
        syscall!(getsockopt(
            fd.fd(),
            level.0.cast_signed(),
            optname.0.cast_signed(),
            optval,
            &mut optlen,
        ))?;
        // SAFETY: the kernel initialised the value for us as part of the
        // getsockopt call.
        Ok(unsafe { T::init(value.0, optlen) })
    }
}

impl_fd_op!(SocketOption2Op<T>);

pub(crate) struct SetSocketOptionOp<T>(PhantomData<*const T>);

impl<T> DirectFdOp for SetSocketOptionOp<T> {
    type Output = ();
    type Resources = T;
    type Args = (Level, Opt);

    fn run(fd: &AsyncFd, resources: Self::Resources, args: Self::Args) -> io::Result<Self::Output> {
        Self::run_extract(fd, resources, args)?;
        Ok(())
    }
}

impl_fd_op!(SetSocketOptionOp<T>);

impl<T> DirectFdOpExtract for SetSocketOptionOp<T> {
    type ExtractOutput = T;

    fn run_extract(
        fd: &AsyncFd,
        value: Self::Resources,
        (level, optname): Self::Args,
    ) -> io::Result<Self::ExtractOutput> {
        syscall!(setsockopt(
            fd.fd(),
            level.0.cast_signed(),
            optname.0.cast_signed(),
            ptr::from_ref(&value).cast(),
            size_of::<T>() as _,
        ))?;
        Ok(value)
    }
}

impl_fd_op_extract!(SetSocketOptionOp<T>);

pub(crate) struct SetSocketOption2Op<T>(PhantomData<*const T>);

impl<T: option::Set> DirectFdOp for SetSocketOption2Op<T> {
    type Output = ();
    type Resources = OptionStorage<T::Storage>;
    type Args = (Level, Opt);

    fn run(
        fd: &AsyncFd,
        value: Self::Resources,
        (level, optname): Self::Args,
    ) -> io::Result<Self::Output> {
        syscall!(setsockopt(
            fd.fd(),
            level.0.cast_signed(),
            optname.0.cast_signed(),
            ptr::from_ref(&value.0).cast(),
            size_of::<T::Storage>() as _,
        ))?;
        Ok(())
    }
}

impl_fd_op!(SetSocketOption2Op<T>);

pub(crate) struct ShutdownOp;

impl DirectFdOp for ShutdownOp {
    type Output = ();
    type Resources = ();
    type Args = std::net::Shutdown;

    fn run(fd: &AsyncFd, (): Self::Resources, how: Self::Args) -> io::Result<Self::Output> {
        let how = match how {
            std::net::Shutdown::Read => libc::SHUT_RD,
            std::net::Shutdown::Write => libc::SHUT_WR,
            std::net::Shutdown::Both => libc::SHUT_RDWR,
        };
        syscall!(shutdown(fd.fd(), how))?;
        Ok(())
    }
}

impl_fd_op!(ShutdownOp);
