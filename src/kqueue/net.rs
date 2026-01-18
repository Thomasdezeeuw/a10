use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::{io, ptr, slice};

use crate::io::{Buf, BufMut, BufMutSlice, BufSlice};
use crate::kqueue::fd::OpKind;
use crate::kqueue::op::{DirectFdOp, DirectOp, FdOp, FdOpExtract, impl_fd_op};
use crate::net::{
    AcceptFlag, AddressStorage, Domain, Level, Name, NoAddress, Opt, OptionStorage, Protocol,
    RecvFlag, SendCall, SendFlag, SocketAddress, Type, option,
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

        let r#type = r#type.0.cast_signed();
        #[cfg(any(
            target_os = "dragonfly",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
        ))]
        let r#type = r#type.0.cast_signed() | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC;

        let socket = syscall!(socket(domain.0, r#type, protocol.0.cast_signed()))?;
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
            ptr::from_ref(&1).cast(),
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
        syscall!(bind(fd.fd(), ptr, length))?;
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
        syscall!(listen(fd.fd(), backlog))?;
        Ok(())
    }
}

impl_fd_op!(ListenOp);

// Sources of documentation on how to make a non-blocking connect call:
// * https://cr.yp.to/docs/connect.html
// * https://stackoverflow.com/questions/17769964/linux-sockets-non-blocking-connect
// * https://github.com/Thomasdezeeuw/heph/blob/12cb4dc3e828c63c18c1ba7a21bcfc39db3bf468/src/net/tcp/stream.rs#L560-L622
pub(crate) struct ConnectOp<A>(PhantomData<*const A>);

#[allow(unreachable_patterns)] // EAGAIN and EWOULDBLOCK can be the same value.
fn is_in_progress(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EINPROGRESS | libc::EAGAIN | libc::EWOULDBLOCK)
    )
}

impl<A: SocketAddress> FdOp for ConnectOp<A> {
    type Output = ();
    type Resources = AddressStorage<A::Storage>;
    type Args = ();
    type OperationOutput = ();

    fn setup(fd: &AsyncFd, address: &mut Self::Resources, (): &mut Self::Args) -> io::Result<()> {
        let (ptr, len) = unsafe { A::as_ptr(&address.0) };
        match syscall!(connect(fd.fd(), ptr, len)) {
            Ok(_) => Ok(()),
            Err(ref err) if is_in_progress(err) => Ok(()),
            Err(err) => Err(err),
        }

        // Now we wait for a read able event and continue in `try_run`.
    }

    const OP_KIND: OpKind = OpKind::Write;

    fn try_run(
        fd: &AsyncFd,
        _: &mut Self::Resources,
        (): &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        // If we hit an error while connecting return that error.
        let mut errno: libc::c_int = 0;
        let mut len = size_of::<libc::c_int>() as libc::socklen_t;
        syscall!(getsockopt(
            fd.fd(),
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            ptr::from_mut(&mut errno).cast(),
            &raw mut len,
        ))?;
        debug_assert!(len == (size_of::<libc::c_int>() as libc::socklen_t));
        if errno != 0 {
            return Err(io::Error::from_raw_os_error(errno));
        }

        let mut address: MaybeUninit<A::Storage> = MaybeUninit::uninit();
        let (ptr, mut length) = unsafe { A::as_mut_ptr(&mut address) };
        match syscall!(getpeername(fd.fd(), ptr, &raw mut length)) {
            Ok(_) => Ok(()),
            Err(err)
                if err.kind() == io::ErrorKind::NotConnected
                // It seems that macOS sometimes returns `EINVAL` when
                // the socket is not (yet) connected. Since we ensure
                // all arguments are valid we can safely ignore it.
                || err.kind() == io::ErrorKind::InvalidInput =>
            {
                // Socket is not (yet) connected but haven't hit an
                // error either. So we return `Pending` and wait for
                // another event.
                Err(io::ErrorKind::WouldBlock.into())
            }
            Err(err) => Err(err),
        }
    }

    fn map_ok(_: &AsyncFd, _: Self::Resources, (): Self::OperationOutput) -> Self::Output {}
}

pub(crate) struct SocketNameOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> DirectFdOp for SocketNameOp<A> {
    type Output = A;
    type Resources = AddressStorage<(MaybeUninit<A::Storage>, libc::socklen_t)>;
    type Args = Name;

    fn run(
        fd: &AsyncFd,
        mut resources: Self::Resources,
        name: Self::Args,
    ) -> io::Result<Self::Output> {
        let (ptr, length) = unsafe { A::as_mut_ptr(&mut (resources.0).0) };
        let address_length = &mut (resources.0).1;
        *address_length = length;
        match name {
            Name::Local => syscall!(getsockname(fd.fd(), ptr, address_length))?,
            Name::Peer => syscall!(getpeername(fd.fd(), ptr, address_length))?,
        };
        // SAFETY: the kernel has written the address for us.
        Ok(unsafe { A::init((resources.0).0, (resources.0).1) })
    }
}

impl_fd_op!(SocketNameOp<A>);

pub(crate) struct RecvOp<B>(PhantomData<*const B>);

impl<B: BufMut> FdOp for RecvOp<B> {
    type Output = B;
    type Resources = B;
    type Args = RecvFlag;
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Read;

    fn try_run(
        fd: &AsyncFd,
        buf: &mut Self::Resources,
        flags: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let (ptr, len, is_pool) = buf.parts().pool_ptr()?;
        let res = syscall!(recv(fd.fd(), ptr.cast(), len as _, flags.0.cast_signed()));
        if res.is_err() && is_pool {
            buf.release();
        }
        res
    }

    fn map_ok(_: &AsyncFd, mut buf: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recv call.
        unsafe { buf.set_init(n.cast_unsigned()) };
        buf
    }
}

pub(crate) struct RecvVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufMutSlice<N>, const N: usize> FdOp for RecvVectoredOp<B, N> {
    type Output = (B, libc::c_int);
    type Resources = (B, MsgHeader, [crate::io::IoMutSlice; N]);
    type Args = RecvFlag;
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Read;

    fn try_run(
        fd: &AsyncFd,
        (_, msg, iovecs): &mut Self::Resources,
        flags: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let address = &mut MaybeUninit::new(NoAddress);
        let ptr = unsafe { msg.init_recv::<NoAddress>(address, iovecs) };
        syscall!(recvmsg(fd.fd(), ptr, flags.0.cast_signed()))
    }

    fn map_ok(
        _: &AsyncFd,
        (mut bufs, msg, _): Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { bufs.set_init(n.cast_unsigned()) };
        (bufs, msg.flags())
    }
}

pub(crate) struct RecvFromOp<B, A>(PhantomData<*const (B, A)>);

impl<B: BufMut, A: SocketAddress> FdOp for RecvFromOp<B, A> {
    type Output = (B, A, libc::c_int);
    type Resources = (B, MsgHeader, crate::io::IoMutSlice, MaybeUninit<A::Storage>);
    type Args = RecvFlag;
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Read;

    fn try_run(
        fd: &AsyncFd,
        (_, msg, iovec, address): &mut Self::Resources,
        flags: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let ptr = unsafe { msg.init_recv::<A>(address, slice::from_mut(iovec)) };
        syscall!(recvmsg(fd.fd(), ptr, flags.0.cast_signed()))
    }

    fn map_ok(
        _: &AsyncFd,
        (mut buf, msg, _, address): Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { buf.set_init(n.cast_unsigned()) };
        // SAFETY: kernel initialised the address for us.
        let address = unsafe { A::init(address, msg.address_len()) };
        (buf, address, msg.flags())
    }
}

pub(crate) struct RecvFromVectoredOp<B, A, const N: usize>(PhantomData<*const (B, A)>);

impl<B: BufMutSlice<N>, A: SocketAddress, const N: usize> FdOp for RecvFromVectoredOp<B, A, N> {
    type Output = (B, A, libc::c_int);
    type Resources = (
        B,
        MsgHeader,
        [crate::io::IoMutSlice; N],
        MaybeUninit<A::Storage>,
    );
    type Args = RecvFlag;
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Read;

    fn try_run(
        fd: &AsyncFd,
        (_, msg, iovecs, address): &mut Self::Resources,
        flags: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let ptr = unsafe { msg.init_recv::<A>(address, iovecs) };
        syscall!(recvmsg(fd.fd(), ptr, flags.0.cast_signed()))
    }

    fn map_ok(
        _: &AsyncFd,
        (mut bufs, msg, _, address): Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { bufs.set_init(n.cast_unsigned()) };
        // SAFETY: kernel initialised the address for us.
        let address = unsafe { A::init(address, msg.address_len()) };
        (bufs, address, msg.flags())
    }
}

pub(crate) struct SendOp<B>(PhantomData<*const B>);

impl<B: Buf> FdOp for SendOp<B> {
    type Output = usize;
    type Resources = B;
    type Args = (SendCall, SendFlag);
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Write;

    fn try_run(
        fd: &AsyncFd,
        buf: &mut Self::Resources,
        (send_op, flags): &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let (SendCall::Normal | SendCall::ZeroCopy) = send_op;

        let (buf_ptr, buf_len) = unsafe { buf.parts() };
        syscall!(send(
            fd.fd(),
            buf_ptr.cast(),
            buf_len as usize,
            flags.0.cast_signed()
        ))
    }

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        Self::map_ok_extract(fd, resources, n).1
    }
}

impl<B: Buf> FdOpExtract for SendOp<B> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        buf: Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (buf, n.cast_unsigned())
    }
}

pub(crate) struct SendToOp<B, A = NoAddress>(PhantomData<*const (B, A)>);

impl<B: Buf, A: SocketAddress> FdOp for SendToOp<B, A> {
    type Output = usize;
    type Resources = (B, AddressStorage<A::Storage>);
    type Args = (SendCall, SendFlag);
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Write;

    fn try_run(
        fd: &AsyncFd,
        (buf, address): &mut Self::Resources,
        (send_op, flags): &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let (SendCall::Normal | SendCall::ZeroCopy) = send_op;

        let (buf_ptr, buf_len) = unsafe { buf.parts() };
        let (address_ptr, address_len) = unsafe { A::as_ptr(&address.0) };
        syscall!(sendto(
            fd.fd(),
            buf_ptr.cast(),
            buf_len as usize,
            flags.0.cast_signed(),
            address_ptr,
            address_len,
        ))
    }

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        Self::map_ok_extract(fd, resources, n).1
    }
}

impl<B: Buf, A: SocketAddress> FdOpExtract for SendToOp<B, A> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        (bufs, _): Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (bufs, n.cast_unsigned())
    }
}

pub(crate) struct SendMsgOp<B, A, const N: usize>(PhantomData<*const (B, A)>);

impl<B: BufSlice<N>, A: SocketAddress, const N: usize> FdOp for SendMsgOp<B, A, N> {
    type Output = usize;
    type Resources = (
        B,
        MsgHeader,
        [crate::io::IoSlice; N],
        AddressStorage<A::Storage>,
    );
    type Args = (SendCall, SendFlag);
    type OperationOutput = libc::ssize_t;

    const OP_KIND: OpKind = OpKind::Write;

    fn try_run(
        fd: &AsyncFd,
        (_, msg, iovecs, address): &mut Self::Resources,
        (send_op, flags): &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        let ptr = unsafe { msg.init_send::<A>(&mut address.0, iovecs) };
        let (SendCall::Normal | SendCall::ZeroCopy) = send_op;
        syscall!(sendmsg(fd.fd(), ptr, flags.0.cast_signed()))
    }

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, n: Self::OperationOutput) -> Self::Output {
        Self::map_ok_extract(fd, resources, n).1
    }
}

impl<B: BufSlice<N>, A: SocketAddress, const N: usize> FdOpExtract for SendMsgOp<B, A, N> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        (bufs, _, _, _): Self::Resources,
        n: Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (bufs, n.cast_unsigned())
    }
}

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
            flags.0.cast_signed() | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
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
            _ = flags;
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

impl<T: option::Get> DirectFdOp for SocketOptionOp<T> {
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
            &raw mut optlen,
        ))?;
        // SAFETY: the kernel initialised the value for us as part of the
        // getsockopt call.
        Ok(unsafe { T::init(value.0, optlen) })
    }
}

impl_fd_op!(SocketOptionOp<T>);

pub(crate) struct SetSocketOptionOp<T>(PhantomData<*const T>);

impl<T: option::Set> DirectFdOp for SetSocketOptionOp<T> {
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

impl_fd_op!(SetSocketOptionOp<T>);

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
