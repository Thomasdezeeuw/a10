use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{array, ptr};

use crate::fd::{AsyncFd, Descriptor};
use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice, Buffer, ReadBuf, ReadBufPool};
use crate::net::{AddressStorage, NoAddress, SendCall, SocketAddress};
use crate::op::{FdIter, FdOpExtract};
use crate::sys::{self, cq, libc, sq};
use crate::SubmissionQueue;

pub(crate) struct SocketOp<D>(PhantomData<*const D>);

impl<D: Descriptor> sys::Op for SocketOp<D> {
    type Output = AsyncFd<D>;
    type Resources = ();
    type Args = (libc::c_int, libc::c_int, libc::c_int, libc::c_int); // domain, type, protocol, flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        (): &mut Self::Resources,
        (domain, r#type, protocol, flags): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_SOCKET as u8;
        submission.0.fd = *domain;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: *r#type as _ };
        submission.0.len = *protocol as _;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { rw_flags: *flags };
        D::create_flags(submission);
    }

    fn map_ok(sq: &SubmissionQueue, (): Self::Resources, (_, fd): cq::OpReturn) -> Self::Output {
        // SAFETY: kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as _, sq.clone()) }
    }
}

pub(crate) struct ConnectOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> sys::FdOp for ConnectOp<A> {
    type Output = ();
    type Resources = AddressStorage<Box<A::Storage>>;
    type Args = ();

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        address: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_CONNECT as u8;
        submission.0.fd = fd.fd();
        let (ptr, length) = unsafe { A::as_ptr(&address.0) };
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: u64::from(length),
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        _: Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct RecvOp<B>(PhantomData<*const B>);

impl<B: BufMut> sys::FdOp for RecvOp<B> {
    type Output = B;
    type Resources = Buffer<B>;
    type Args = libc::c_int; // flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        buf: &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_RECV as u8;
        submission.0.fd = fd.fd();
        let (ptr, length) = unsafe { buf.buf.parts_mut() };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: *flags as _,
        };
        submission.0.len = length;
        if let Some(buf_group) = buf.buf.buffer_group() {
            submission.0.__bindgen_anon_4.buf_group = buf_group.0;
            submission.0.flags |= libc::IOSQE_BUFFER_SELECT;
        }
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        mut buf: Self::Resources,
        (buf_id, n): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: kernel just initialised the bytes for us.
        unsafe {
            buf.buf.buffer_init(BufId(buf_id), n);
        };
        buf.buf
    }
}

pub(crate) struct MultishotRecvOp;

impl sys::FdOp for MultishotRecvOp {
    type Output = ReadBuf;
    type Resources = ReadBufPool;
    type Args = libc::c_int; // flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        buf_pool: &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_RECV as u8;
        submission.0.flags = libc::IOSQE_BUFFER_SELECT;
        submission.0.ioprio = libc::IORING_RECV_MULTISHOT as _;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: *flags as _,
        };
        submission.0.__bindgen_anon_4.buf_group = buf_pool.group_id().0;
    }

    fn map_ok<D: Descriptor>(
        fd: &AsyncFd<D>,
        mut buf_pool: Self::Resources,
        (buf_id, n): cq::OpReturn,
    ) -> Self::Output {
        MultishotRecvOp::map_next(fd, &mut buf_pool, (buf_id, n))
    }
}

impl FdIter for MultishotRecvOp {
    fn map_next<D: Descriptor>(
        _: &AsyncFd<D>,
        buf_pool: &mut Self::Resources,
        (buf_id, n): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the buffers for us as part of the read
        // call.
        unsafe { buf_pool.new_buffer(BufId(buf_id), n) }
    }
}

pub(crate) struct RecvVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufMutSlice<N>, const N: usize> sys::FdOp for RecvVectoredOp<B, N> {
    type Output = (B, libc::c_int);
    type Resources = (B, Box<(libc::msghdr, [crate::io::IoMutSlice; N])>);
    type Args = libc::c_int; // flags

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (_, resources): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (msg, iovecs) = &mut **resources;
        let address = &mut MaybeUninit::new(NoAddress);
        fill_recvmsg_submission::<NoAddress, N>(fd.fd(), msg, iovecs, address, *flags, submission)
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        (mut bufs, resources): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { bufs.set_init(n as usize) };
        (bufs, resources.0.msg_flags)
    }
}

pub(crate) struct RecvFromOp<B, A>(PhantomData<*const (B, A)>);

impl<B: BufMut, A: SocketAddress> sys::FdOp for RecvFromOp<B, A> {
    type Output = (B, A, libc::c_int);
    type Resources = (
        B,
        // These types need a stable address for the duration of the operation.
        Box<(libc::msghdr, crate::io::IoMutSlice, MaybeUninit<A::Storage>)>,
    );
    type Args = libc::c_int; // flags

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (_, resources): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (msg, iovec, address) = &mut **resources;
        let iovecs = array::from_mut(iovec);
        fill_recvmsg_submission::<A, 1>(fd.fd(), msg, iovecs, address, *flags, submission)
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        (mut buf, resources): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { buf.set_init(n as usize) };
        // SAFETY: kernel initialised the address for us.
        let address = unsafe { A::init(resources.2, resources.0.msg_namelen) };
        (buf, address, resources.0.msg_flags)
    }
}

pub(crate) struct RecvFromVectoredOp<B, A, const N: usize>(PhantomData<*const (B, A)>);

impl<B: BufMutSlice<N>, A: SocketAddress, const N: usize> sys::FdOp
    for RecvFromVectoredOp<B, A, N>
{
    type Output = (B, A, libc::c_int);
    type Resources = (
        B,
        // These types need a stable address for the duration of the operation.
        Box<(
            libc::msghdr,
            [crate::io::IoMutSlice; N],
            MaybeUninit<A::Storage>,
        )>,
    );
    type Args = libc::c_int; // flags

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (_, resources): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (msg, iovecs, address) = &mut **resources;
        fill_recvmsg_submission::<A, N>(fd.fd(), msg, iovecs, address, *flags, submission)
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        (mut bufs, resources): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { bufs.set_init(n as usize) };
        // SAFETY: kernel initialised the address for us.
        let address = unsafe { A::init(resources.2, resources.0.msg_namelen) };
        (bufs, address, resources.0.msg_flags)
    }
}

fn fill_recvmsg_submission<A: SocketAddress, const N: usize>(
    fd: RawFd,
    msg: &mut libc::msghdr,
    iovecs: &mut [crate::io::IoMutSlice; N],
    address: &mut MaybeUninit<A::Storage>,
    flags: libc::c_int,
    submission: &mut sq::Submission,
) {
    let (ptr, length) = unsafe { A::as_mut_ptr(address) };
    msg.msg_name = ptr.cast();
    msg.msg_namelen = length;
    // SAFETY: this cast is safe because `IoMutSlice` is `repr(transparent)`.
    msg.msg_iov = ptr::from_mut(&mut *iovecs).cast();
    msg.msg_iovlen = N;

    submission.0.opcode = libc::IORING_OP_RECVMSG as u8;
    submission.0.fd = fd;
    submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
        addr: ptr::from_mut(&mut *msg).addr() as _,
    };
    submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
        msg_flags: flags as _,
    };
    submission.0.len = 1;
}

pub(crate) struct SendOp<B>(PhantomData<*const B>);

impl<B: Buf> sys::FdOp for SendOp<B> {
    type Output = usize;
    type Resources = Buffer<B>;
    type Args = (SendCall, libc::c_int); // send_op, flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        buf: &mut Self::Resources,
        (send_op, flags): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = match *send_op {
            SendCall::Normal => libc::IORING_OP_SEND as u8,
            SendCall::ZeroCopy => libc::IORING_OP_SEND_ZC as u8,
        };
        submission.0.fd = fd.fd();
        let (ptr, length) = unsafe { buf.buf.parts() };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as u64 };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: *flags as _,
        };
        submission.0.len = length;
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        _: Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        n as usize
    }
}

impl<B: Buf> FdOpExtract for SendOp<B> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract<D: Descriptor>(
        _: &AsyncFd<D>,
        buf: Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (buf.buf, n as usize)
    }
}

pub(crate) struct SendMsgOp<B, A, const N: usize>(PhantomData<*const (B, A)>);

impl<B: BufSlice<N>, A: SocketAddress, const N: usize> sys::FdOp for SendMsgOp<B, A, N> {
    type Output = usize;
    type Resources = (
        B,
        // These types need a stable address for the duration of the operation.
        Box<(libc::msghdr, [crate::io::IoSlice; N], A::Storage)>,
    );
    type Args = (SendCall, libc::c_int); // send_op, flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (_, resources): &mut Self::Resources,
        (send_op, flags): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (msg, iovecs, address) = &mut **resources;
        let (ptr, length) = unsafe { A::as_ptr(address) };
        msg.msg_name = ptr.cast_mut().cast();
        msg.msg_namelen = length;
        // SAFETY: this cast is safe because `IoMutSlice` is `repr(transparent)`.
        msg.msg_iov = ptr::from_mut(&mut *iovecs).cast();
        msg.msg_iovlen = N;

        submission.0.opcode = match *send_op {
            SendCall::Normal => libc::IORING_OP_SENDMSG as u8,
            SendCall::ZeroCopy => libc::IORING_OP_SENDMSG_ZC as u8,
        };
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr::from_mut(&mut *msg).addr() as _,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: *flags as _,
        };
        submission.0.len = 1;
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        _: Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        n as usize
    }
}

impl<B: BufSlice<N>, A: SocketAddress, const N: usize> FdOpExtract for SendMsgOp<B, A, N> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract<D: Descriptor>(
        _: &AsyncFd<D>,
        (buf, _): Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (buf, n as usize)
    }
}

pub(crate) struct AcceptOp<A, D>(PhantomData<*const (A, D)>);

impl<A: SocketAddress, D: Descriptor> sys::FdOp for AcceptOp<A, D> {
    type Output = (AsyncFd<D>, A);
    type Resources = AddressStorage<Box<(MaybeUninit<A::Storage>, libc::socklen_t)>>;
    type Args = libc::c_int; // flags

    fn fill_submission<LD: Descriptor>(
        fd: &AsyncFd<LD>,
        resources: &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (ptr, length) = unsafe { A::as_mut_ptr(&mut (resources.0).0) };
        let address_length = &mut (resources.0).1;
        *address_length = length;
        submission.0.opcode = libc::IORING_OP_ACCEPT as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: ptr::from_mut(address_length).addr() as _,
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr.addr() as _,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            accept_flags: *flags as _,
        };
        submission.0.flags |= libc::IOSQE_ASYNC;
        D::create_flags(submission);
    }

    fn map_ok<LD: Descriptor>(
        lfd: &AsyncFd<LD>,
        resources: Self::Resources,
        (_, fd): cq::OpReturn,
    ) -> Self::Output {
        let sq = lfd.sq.clone();
        // SAFETY: the accept operation ensures that `fd` is valid.
        let socket = unsafe { AsyncFd::from_raw(fd as _, sq) };
        // SAFETY: the kernel has written the address for us.
        let address = unsafe { A::init((resources.0).0, (resources.0).1) };
        (socket, address)
    }
}

pub(crate) struct MultishotAcceptOp<D>(PhantomData<*const D>);

impl<D: Descriptor> sys::FdOp for MultishotAcceptOp<D> {
    type Output = AsyncFd<D>;
    type Resources = ();
    type Args = libc::c_int; // flags

    fn fill_submission<LD: Descriptor>(
        fd: &AsyncFd<LD>,
        (): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_ACCEPT as u8;
        submission.0.ioprio = libc::IORING_ACCEPT_MULTISHOT as _;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            accept_flags: *flags as _,
        };
        submission.0.flags = libc::IOSQE_ASYNC;
        D::create_flags(submission);
    }

    fn map_ok<LD: Descriptor>(
        lfd: &AsyncFd<LD>,
        (): Self::Resources,
        ok: cq::OpReturn,
    ) -> Self::Output {
        MultishotAcceptOp::map_next(lfd, &mut (), ok)
    }
}

impl<D: Descriptor> FdIter for MultishotAcceptOp<D> {
    fn map_next<LD: Descriptor>(
        lfd: &AsyncFd<LD>,
        (): &mut Self::Resources,
        (_, fd): cq::OpReturn,
    ) -> Self::Output {
        let sq = lfd.sq.clone();
        // SAFETY: the accept operation ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as _, sq) }
    }
}

pub(crate) struct SocketOptionOp<T>(PhantomData<*const T>);

impl<T> sys::FdOp for SocketOptionOp<T> {
    type Output = T;
    type Resources = Box<MaybeUninit<T>>;
    type Args = (libc::c_int, libc::c_int); // level, optname.

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        value: &mut Self::Resources,
        (level, optname): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_URING_CMD as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            __bindgen_anon_1: libc::io_uring_sqe__bindgen_ty_1__bindgen_ty_1 {
                cmd_op: libc::SOCKET_URING_OP_GETSOCKOPT,
                __pad1: 0,
            },
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            __bindgen_anon_1: libc::io_uring_sqe__bindgen_ty_2__bindgen_ty_1 {
                level: *level as _,
                optname: *optname as _,
            },
        };
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            optlen: size_of::<T>() as u32,
        };
        submission.0.__bindgen_anon_6 = libc::io_uring_sqe__bindgen_ty_6 {
            optval: ManuallyDrop::new(value.as_mut_ptr().addr() as _),
        };
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        value: Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        debug_assert!(n == (size_of::<T>() as _));
        // SAFETY: the kernel initialised the value for us as part of the
        // getsockopt call.
        unsafe { MaybeUninit::assume_init(*value) }
    }
}

pub(crate) struct SetSocketOptionOp<T>(PhantomData<*const T>);

impl<T> sys::FdOp for SetSocketOptionOp<T> {
    type Output = ();
    type Resources = Box<T>;
    type Args = (libc::c_int, libc::c_int); // level, optname.

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        value: &mut Self::Resources,
        (level, optname): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_URING_CMD as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            __bindgen_anon_1: libc::io_uring_sqe__bindgen_ty_1__bindgen_ty_1 {
                cmd_op: libc::SOCKET_URING_OP_SETSOCKOPT,
                __pad1: 0,
            },
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            __bindgen_anon_1: libc::io_uring_sqe__bindgen_ty_2__bindgen_ty_1 {
                level: *level as _,
                optname: *optname as _,
            },
        };
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            optlen: size_of::<T>() as u32,
        };
        submission.0.__bindgen_anon_6 = libc::io_uring_sqe__bindgen_ty_6 {
            optval: ManuallyDrop::new(ptr::from_ref(&**value).addr() as _),
        };
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        _: Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        debug_assert!(n == 0);
    }
}

impl<T> FdOpExtract for SetSocketOptionOp<T> {
    type ExtractOutput = T;

    fn map_ok_extract<D: Descriptor>(
        _: &AsyncFd<D>,
        value: Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        debug_assert!(n == 0);
        *value
    }
}

pub(crate) struct ShutdownOp;

impl sys::FdOp for ShutdownOp {
    type Output = ();
    type Resources = ();
    type Args = std::net::Shutdown;

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        (): &mut Self::Resources,
        how: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_SHUTDOWN as u8;
        submission.0.fd = fd.fd();
        submission.0.len = match how {
            std::net::Shutdown::Read => libc::SHUT_RD,
            std::net::Shutdown::Write => libc::SHUT_WR,
            std::net::Shutdown::Both => libc::SHUT_RDWR,
        } as u32;
    }

    fn map_ok<D: Descriptor>(
        _: &AsyncFd<D>,
        (): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        debug_assert!(n == 0);
    }
}
