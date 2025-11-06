use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{ptr, slice};

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice, Buffer, ReadBuf, ReadBufPool};
use crate::io_uring::{self, cq, libc, sq};
use crate::net::{AddressStorage, NoAddress, SendCall, SocketAddress};
use crate::op::{FdIter, FdOpExtract};
use crate::{fd, AsyncFd, SubmissionQueue};

pub(crate) use crate::unix::MsgHeader;

pub(crate) struct SocketOp;

impl io_uring::Op for SocketOp {
    type Output = AsyncFd;
    type Resources = fd::Kind;
    type Args = (libc::c_int, libc::c_int, libc::c_int, libc::c_int); // domain, type, protocol, flags.

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd_kind: &mut Self::Resources,
        (domain, r#type, protocol, flags): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_SOCKET as u8;
        submission.0.fd = *domain;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: *r#type as u64,
        };
        submission.0.len = *protocol as u32;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { rw_flags: *flags };
        if let fd::Kind::Direct = *fd_kind {
            io_uring::fd::create_direct_flags(submission);
        }
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok(
        sq: &SubmissionQueue,
        fd_kind: Self::Resources,
        (_, fd): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as RawFd, fd_kind, sq.clone()) }
    }
}

pub(crate) struct ConnectOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> io_uring::FdOp for ConnectOp<A> {
    type Output = ();
    type Resources = AddressStorage<Box<A::Storage>>;
    type Args = ();

    fn fill_submission(
        fd: &AsyncFd,
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
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr.addr() as u64,
        };
    }

    fn map_ok(_: &AsyncFd, _: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct RecvOp<B>(PhantomData<*const B>);

impl<B: BufMut> io_uring::FdOp for RecvOp<B> {
    type Output = B;
    type Resources = Buffer<B>;
    type Args = libc::c_int; // flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
        buf: &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_RECV as u8;
        submission.0.fd = fd.fd();
        let (ptr, length) = unsafe { buf.buf.parts_mut() };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr.addr() as u64,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: *flags as u32,
        };
        submission.0.len = length;
        if let Some(buf_group) = buf.buf.buffer_group() {
            submission.0.__bindgen_anon_4.buf_group = buf_group.0;
            submission.0.flags |= libc::IOSQE_BUFFER_SELECT;
        }
    }

    fn map_ok(_: &AsyncFd, mut buf: Self::Resources, (buf_id, n): cq::OpReturn) -> Self::Output {
        // SAFETY: kernel just initialised the bytes for us.
        unsafe {
            buf.buf.buffer_init(BufId(buf_id), n);
        };
        buf.buf
    }
}

pub(crate) struct MultishotRecvOp;

impl io_uring::FdOp for MultishotRecvOp {
    type Output = ReadBuf;
    type Resources = ReadBufPool;
    type Args = libc::c_int; // flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
        buf_pool: &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_RECV as u8;
        submission.0.flags = libc::IOSQE_BUFFER_SELECT;
        submission.0.ioprio = libc::IORING_RECV_MULTISHOT as u16;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: *flags as u32,
        };
        submission.0.__bindgen_anon_4.buf_group = buf_pool.group_id().0;
    }

    fn map_ok(
        fd: &AsyncFd,
        mut buf_pool: Self::Resources,
        (buf_id, n): cq::OpReturn,
    ) -> Self::Output {
        MultishotRecvOp::map_next(fd, &mut buf_pool, (buf_id, n))
    }
}

impl FdIter for MultishotRecvOp {
    fn map_next(
        _: &AsyncFd,
        buf_pool: &mut Self::Resources,
        (buf_id, n): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the buffers for us as part of the read
        // call.
        unsafe { buf_pool.new_buffer(BufId(buf_id), n) }
    }
}

pub(crate) struct RecvVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufMutSlice<N>, const N: usize> io_uring::FdOp for RecvVectoredOp<B, N> {
    type Output = (B, libc::c_int);
    type Resources = (B, Box<(MsgHeader, [crate::io::IoMutSlice; N])>);
    type Args = libc::c_int; // flags

    fn fill_submission(
        fd: &AsyncFd,
        (_, resources): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (msg, iovecs) = &mut **resources;
        let address = &mut MaybeUninit::new(NoAddress);
        fill_recvmsg_submission::<NoAddress>(fd.fd(), msg, iovecs, address, *flags, submission);
    }

    fn map_ok(
        _: &AsyncFd,
        (mut bufs, resources): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { bufs.set_init(n as usize) };
        (bufs, resources.0.flags())
    }
}

pub(crate) struct RecvFromOp<B, A>(PhantomData<*const (B, A)>);

impl<B: BufMut, A: SocketAddress> io_uring::FdOp for RecvFromOp<B, A> {
    type Output = (B, A, libc::c_int);
    type Resources = (
        B,
        // These types need a stable address for the duration of the operation.
        Box<(MsgHeader, crate::io::IoMutSlice, MaybeUninit<A::Storage>)>,
    );
    type Args = libc::c_int; // flags

    fn fill_submission(
        fd: &AsyncFd,
        (buf, resources): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (msg, iovec, address) = &mut **resources;
        let iovecs = slice::from_mut(&mut *iovec);
        fill_recvmsg_submission::<A>(fd.fd(), msg, iovecs, address, *flags, submission);
        if let Some(buf_group) = buf.buffer_group() {
            submission.0.__bindgen_anon_4.buf_group = buf_group.0;
            submission.0.flags |= libc::IOSQE_BUFFER_SELECT;
        }
    }

    fn map_ok(
        _: &AsyncFd,
        (mut buf, resources): Self::Resources,
        (buf_id, n): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { buf.buffer_init(BufId(buf_id), n) };
        // SAFETY: kernel initialised the address for us.
        let address = unsafe { A::init(resources.2, resources.0.address_len()) };
        (buf, address, resources.0.flags())
    }
}

pub(crate) struct RecvFromVectoredOp<B, A, const N: usize>(PhantomData<*const (B, A)>);

impl<B: BufMutSlice<N>, A: SocketAddress, const N: usize> io_uring::FdOp
    for RecvFromVectoredOp<B, A, N>
{
    type Output = (B, A, libc::c_int);
    type Resources = (
        B,
        // These types need a stable address for the duration of the operation.
        Box<(
            MsgHeader,
            [crate::io::IoMutSlice; N],
            MaybeUninit<A::Storage>,
        )>,
    );
    type Args = libc::c_int; // flags

    fn fill_submission(
        fd: &AsyncFd,
        (_, resources): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (msg, iovecs, address) = &mut **resources;
        fill_recvmsg_submission::<A>(fd.fd(), msg, iovecs, address, *flags, submission);
    }

    fn map_ok(
        _: &AsyncFd,
        (mut bufs, resources): Self::Resources,
        (_, n): cq::OpReturn,
    ) -> Self::Output {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { bufs.set_init(n as usize) };
        // SAFETY: kernel initialised the address for us.
        let address = unsafe { A::init(resources.2, resources.0.address_len()) };
        (bufs, address, resources.0.flags())
    }
}

#[allow(clippy::cast_sign_loss)] // For flags as u32.
fn fill_recvmsg_submission<A: SocketAddress>(
    fd: RawFd,
    msg: &mut MsgHeader,
    iovecs: &mut [crate::io::IoMutSlice],
    address: &mut MaybeUninit<A::Storage>,
    flags: libc::c_int,
    submission: &mut sq::Submission,
) {
    // SAFETY: `address` and `iovecs` outlive `msg`.
    unsafe { msg.init_recv::<A>(address, iovecs) };

    submission.0.opcode = libc::IORING_OP_RECVMSG as u8;
    submission.0.fd = fd;
    submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
        addr: ptr::from_mut(&mut *msg).addr() as u64,
    };
    submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
        msg_flags: flags as u32,
    };
    submission.0.len = 1;
}

pub(crate) struct SendOp<B>(PhantomData<*const B>);

impl<B: Buf> io_uring::FdOp for SendOp<B> {
    type Output = usize;
    type Resources = Buffer<B>;
    type Args = (SendCall, libc::c_int); // send_op, flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
        buf: &mut Self::Resources,
        (send_op, flags): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = match *send_op {
            SendCall::Normal => libc::IORING_OP_SEND as u8,
            SendCall::ZeroCopy => libc::IORING_OP_SEND_ZC as u8,
        };
        let (buf_ptr, buf_length) = unsafe { buf.buf.parts() };
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: buf_ptr.addr() as u64,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: *flags as u32,
        };
        submission.0.len = buf_length;
    }

    fn map_ok(_: &AsyncFd, _: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        n as usize
    }
}

impl<B: Buf> FdOpExtract for SendOp<B> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        buf: Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (buf.buf, n as usize)
    }
}

pub(crate) struct SendToOp<B, A = NoAddress>(PhantomData<*const (B, A)>);

impl<B: Buf, A: SocketAddress> io_uring::FdOp for SendToOp<B, A> {
    type Output = usize;
    type Resources = (B, Box<A::Storage>);
    type Args = (SendCall, libc::c_int); // send_op, flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
        (buf, address): &mut Self::Resources,
        (send_op, flags): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = match *send_op {
            SendCall::Normal => libc::IORING_OP_SEND as u8,
            SendCall::ZeroCopy => libc::IORING_OP_SEND_ZC as u8,
        };
        let (buf_ptr, buf_length) = unsafe { buf.parts() };
        let (address_ptr, address_length) = unsafe { A::as_ptr(address) };
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1.addr2 = address_ptr.addr() as u64;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: buf_ptr.addr() as u64,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: *flags as u32,
        };
        submission.0.__bindgen_anon_5.__bindgen_anon_1.addr_len = address_length as u16;
        submission.0.len = buf_length;
    }

    fn map_ok(_: &AsyncFd, _: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        n as usize
    }
}

impl<B: Buf, A: SocketAddress> FdOpExtract for SendToOp<B, A> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        (buf, _): Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (buf, n as usize)
    }
}

pub(crate) struct SendMsgOp<B, A, const N: usize>(PhantomData<*const (B, A)>);

impl<B: BufSlice<N>, A: SocketAddress, const N: usize> io_uring::FdOp for SendMsgOp<B, A, N> {
    type Output = usize;
    type Resources = (
        B,
        // These types need a stable address for the duration of the operation.
        Box<(MsgHeader, [crate::io::IoSlice; N], A::Storage)>,
    );
    type Args = (SendCall, libc::c_int); // send_op, flags

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
        (_, resources): &mut Self::Resources,
        (send_op, flags): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (msg, iovecs, address) = &mut **resources;
        // SAFETY: `address` and `iovecs` outlive `msg`.
        unsafe { msg.init_send::<A>(address, iovecs) };

        submission.0.opcode = match *send_op {
            SendCall::Normal => libc::IORING_OP_SENDMSG as u8,
            SendCall::ZeroCopy => libc::IORING_OP_SENDMSG_ZC as u8,
        };
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr::from_mut(&mut *msg).addr() as u64,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: *flags as u32,
        };
        submission.0.len = 1;
    }

    fn map_ok(_: &AsyncFd, _: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        n as usize
    }
}

impl<B: BufSlice<N>, A: SocketAddress, const N: usize> FdOpExtract for SendMsgOp<B, A, N> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        (buf, _): Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        (buf, n as usize)
    }
}

pub(crate) struct AcceptOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> io_uring::FdOp for AcceptOp<A> {
    type Output = (AsyncFd, A);
    type Resources = AddressStorage<Box<(MaybeUninit<A::Storage>, libc::socklen_t)>>;
    type Args = libc::c_int; // flags

    #[allow(clippy::cast_sign_loss)] // For flags as u32.
    fn fill_submission(
        fd: &AsyncFd,
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
            off: ptr::from_mut(address_length).addr() as u64,
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr.addr() as u64,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            accept_flags: *flags as u32,
        };
        submission.0.flags |= libc::IOSQE_ASYNC;
        fd.create_flags(submission);
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok(lfd: &AsyncFd, resources: Self::Resources, (_, fd): cq::OpReturn) -> Self::Output {
        let sq = lfd.sq.clone();
        // SAFETY: the accept operation ensures that `fd` is valid.
        let socket = unsafe { AsyncFd::from_raw(fd as RawFd, lfd.kind(), sq) };
        // SAFETY: the kernel has written the address for us.
        let address = unsafe { A::init((resources.0).0, (resources.0).1) };
        (socket, address)
    }
}

pub(crate) struct MultishotAcceptOp;

impl io_uring::FdOp for MultishotAcceptOp {
    type Output = AsyncFd;
    type Resources = ();
    type Args = libc::c_int; // flags

    #[allow(clippy::cast_sign_loss)] // For flags as u32.
    fn fill_submission(
        fd: &AsyncFd,
        (): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_ACCEPT as u8;
        submission.0.ioprio = libc::IORING_ACCEPT_MULTISHOT as u16;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            accept_flags: *flags as u32,
        };
        submission.0.flags = libc::IOSQE_ASYNC;
        fd.create_flags(submission);
    }

    fn map_ok(lfd: &AsyncFd, (): Self::Resources, ok: cq::OpReturn) -> Self::Output {
        MultishotAcceptOp::map_next(lfd, &mut (), ok)
    }
}

impl FdIter for MultishotAcceptOp {
    #[allow(clippy::cast_possible_wrap)]
    fn map_next(lfd: &AsyncFd, (): &mut Self::Resources, (_, fd): cq::OpReturn) -> Self::Output {
        let sq = lfd.sq.clone();
        // SAFETY: the accept operation ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as RawFd, lfd.kind(), sq) }
    }
}

pub(crate) struct SocketOptionOp<T>(PhantomData<*const T>);

impl<T> io_uring::FdOp for SocketOptionOp<T> {
    type Output = T;
    type Resources = Box<MaybeUninit<T>>;
    type Args = (libc::c_int, libc::c_int); // level, optname.

    #[allow(clippy::cast_sign_loss)] // For level and optname as u32.
    fn fill_submission(
        fd: &AsyncFd,
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
                level: *level as u32,
                optname: *optname as u32,
            },
        };
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            optlen: size_of::<T>() as u32,
        };
        submission.0.__bindgen_anon_6 = libc::io_uring_sqe__bindgen_ty_6 {
            optval: ManuallyDrop::new(value.as_mut_ptr().addr() as u64),
        };
    }

    fn map_ok(_: &AsyncFd, value: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == (size_of::<T>() as u32));
        // SAFETY: the kernel initialised the value for us as part of the
        // getsockopt call.
        unsafe { MaybeUninit::assume_init(*value) }
    }
}

pub(crate) struct SetSocketOptionOp<T>(PhantomData<*const T>);

impl<T> io_uring::FdOp for SetSocketOptionOp<T> {
    type Output = ();
    type Resources = Box<T>;
    type Args = (libc::c_int, libc::c_int); // level, optname.

    #[allow(clippy::cast_sign_loss)] // For level and optname as u32.
    fn fill_submission(
        fd: &AsyncFd,
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
                level: *level as u32,
                optname: *optname as u32,
            },
        };
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            optlen: size_of::<T>() as u32,
        };
        submission.0.__bindgen_anon_6 = libc::io_uring_sqe__bindgen_ty_6 {
            optval: ManuallyDrop::new(ptr::from_ref(&**value).addr() as u64),
        };
    }

    fn map_ok(_: &AsyncFd, _: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

impl<T> FdOpExtract for SetSocketOptionOp<T> {
    type ExtractOutput = T;

    fn map_ok_extract(
        _: &AsyncFd,
        value: Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        debug_assert!(n == 0);
        *value
    }
}

pub(crate) struct ShutdownOp;

impl io_uring::FdOp for ShutdownOp {
    type Output = ();
    type Resources = ();
    type Args = std::net::Shutdown;

    #[allow(clippy::cast_sign_loss)] // For shutdown as u32.
    fn fill_submission(
        fd: &AsyncFd,
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

    fn map_ok(_: &AsyncFd, (): Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}
