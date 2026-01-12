use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{ptr, slice};

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice, ReadBuf, ReadBufPool};
use crate::io_uring::op::{FdIter, FdOp, FdOpExtract, Op, OpReturn};
use crate::io_uring::{libc, sq};
use crate::net::{
    AcceptFlag, AddressStorage, Domain, Level, Name, NoAddress, Opt, OptionStorage, Protocol,
    RecvFlag, SendCall, SendFlag, SocketAddress, Type, option,
};
use crate::{AsyncFd, SubmissionQueue, asan, fd, msan};

pub(crate) use crate::unix::MsgHeader;

pub(crate) struct SocketOp;

impl Op for SocketOp {
    type Output = AsyncFd;
    type Resources = fd::Kind;
    type Args = (Domain, Type, Protocol);

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd_kind: &mut Self::Resources,
        (domain, r#type, protocol): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_SOCKET as u8;
        submission.0.fd = domain.0;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: r#type.0.into(),
        };
        submission.0.len = protocol.0;
        // Must currently always be set to zero per the manual.
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { rw_flags: 0 };
        fd_kind.create_flags(submission);
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok(sq: &SubmissionQueue, fd_kind: Self::Resources, (_, fd): OpReturn) -> Self::Output {
        // SAFETY: kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as RawFd, fd_kind, sq.clone()) }
    }
}

pub(crate) struct BindOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> FdOp for BindOp<A> {
    type Output = ();
    type Resources = AddressStorage<A::Storage>;
    type Args = ();

    fn fill_submission(
        fd: &AsyncFd,
        address: &mut Self::Resources,
        (): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_BIND as u8;
        submission.0.fd = fd.fd();
        let (ptr, length) = unsafe { A::as_ptr(&address.0) };
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            addr2: u64::from(length),
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr.addr() as u64,
        };
    }

    fn map_ok(_: &AsyncFd, _: Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct ListenOp;

impl FdOp for ListenOp {
    type Output = ();
    type Resources = ();
    type Args = libc::c_int; // backlog.

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
        (): &mut Self::Resources,
        backlog: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_LISTEN as u8;
        submission.0.fd = fd.fd();
        submission.0.len = *backlog as u32;
    }

    fn map_ok(_: &AsyncFd, (): Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct ConnectOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> FdOp for ConnectOp<A> {
    type Output = ();
    type Resources = AddressStorage<A::Storage>;
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

    fn map_ok(_: &AsyncFd, _: Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct SocketNameOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> FdOp for SocketNameOp<A> {
    type Output = A;
    type Resources = AddressStorage<(MaybeUninit<A::Storage>, libc::socklen_t)>;
    type Args = Name;

    fn fill_submission(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        name: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (ptr, length) = unsafe { A::as_mut_ptr(&mut (resources.0).0) };
        let address_length = &mut (resources.0).1;
        *address_length = length;
        submission.0.opcode = libc::IORING_OP_URING_CMD as u8;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            __bindgen_anon_1: libc::io_uring_sqe__bindgen_ty_1__bindgen_ty_1 {
                cmd_op: libc::SOCKET_URING_OP_GETSOCKNAME,
                __pad1: 0,
            },
        };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr.addr() as u64,
        };
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            optlen: match name {
                Name::Local => 1,
                Name::Peer => 0,
            },
        };
        submission.0.__bindgen_anon_6 = libc::io_uring_sqe__bindgen_ty_6 {
            __bindgen_anon_1: ManuallyDrop::new(libc::io_uring_sqe__bindgen_ty_6__bindgen_ty_1 {
                addr3: ptr::from_mut(address_length).addr() as u64,
                __pad2: [0; 1],
            }),
        };
    }

    fn map_ok(_: &AsyncFd, resources: Self::Resources, _: OpReturn) -> Self::Output {
        msan::unpoison_region(
            ptr::from_ref(&(resources.0).1).cast(),
            size_of::<libc::socklen_t>(),
        );
        msan::unpoison_region((resources.0).0.as_ptr().cast(), (resources.0).1 as usize);
        // SAFETY: the kernel has written the address for us.
        unsafe { A::init((resources.0).0, (resources.0).1) }
    }
}

pub(crate) struct RecvOp<B>(PhantomData<*const B>);

impl<B: BufMut> FdOp for RecvOp<B> {
    type Output = B;
    type Resources = B;
    type Args = RecvFlag;

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
        buf: &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_RECV as u8;
        submission.0.fd = fd.fd();
        let (ptr, len) = unsafe { buf.parts_mut() };
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr.addr() as u64,
        };
        asan::poison_region(ptr.cast(), len as usize);
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { msg_flags: flags.0 };
        submission.0.len = len;
        if let Some(buf_group) = buf.buffer_group() {
            submission.0.__bindgen_anon_4.buf_group = buf_group.0;
            submission.0.flags |= libc::IOSQE_BUFFER_SELECT;
        }
    }

    fn map_ok(_: &AsyncFd, mut buf: Self::Resources, (buf_id, n): OpReturn) -> Self::Output {
        let (ptr, len) = unsafe { buf.parts_mut() };
        asan::unpoison_region(ptr.cast(), len as usize);
        msan::unpoison_region(ptr.cast(), len as usize);
        // SAFETY: kernel just initialised the bytes for us.
        unsafe {
            buf.buffer_init(BufId(buf_id), n);
        };
        buf
    }
}

pub(crate) struct MultishotRecvOp;

impl FdIter for MultishotRecvOp {
    type Output = ReadBuf;
    type Resources = ReadBufPool;
    type Args = RecvFlag;

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
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { msg_flags: flags.0 };
        submission.0.__bindgen_anon_4.buf_group = buf_pool.group_id().0;
    }

    fn map_next(_: &AsyncFd, buf_pool: &Self::Resources, (buf_id, n): OpReturn) -> Self::Output {
        // NOTE: the asan/msan unpoisoning is done in `ReadBufPool::init_buffer`.
        // SAFETY: the kernel initialised the buffers for us as part of the read
        // call.
        unsafe { buf_pool.new_buffer(BufId(buf_id), n) }
    }
}

pub(crate) struct RecvVectoredOp<B, const N: usize>(PhantomData<*const B>);

impl<B: BufMutSlice<N>, const N: usize> FdOp for RecvVectoredOp<B, N> {
    type Output = (B, libc::c_int);
    type Resources = (B, MsgHeader, [crate::io::IoMutSlice; N]);
    type Args = RecvFlag;

    fn fill_submission(
        fd: &AsyncFd,
        (_, msg, iovecs): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let address = &mut MaybeUninit::new(NoAddress);
        fill_recvmsg_submission::<NoAddress>(fd.fd(), msg, iovecs, address, *flags, submission);
    }

    fn map_ok(
        _: &AsyncFd,
        (mut bufs, msg, iovecs): Self::Resources,
        (_, n): OpReturn,
    ) -> Self::Output {
        asan::unpoison_iovecs_mut(&iovecs);
        msan::unpoison_iovecs_mut(&iovecs, n as usize);
        // NOTE: don't need to unpoison the address as we didn't use one.
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { bufs.set_init(n as usize) };
        (bufs, msg.flags())
    }
}

pub(crate) struct RecvFromOp<B, A>(PhantomData<*const (B, A)>);

impl<B: BufMut, A: SocketAddress> FdOp for RecvFromOp<B, A> {
    type Output = (B, A, libc::c_int);
    type Resources = (B, MsgHeader, crate::io::IoMutSlice, MaybeUninit<A::Storage>);
    type Args = RecvFlag;

    fn fill_submission(
        fd: &AsyncFd,
        (buf, msg, iovec, address): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let iovecs = slice::from_mut(&mut *iovec);
        fill_recvmsg_submission::<A>(fd.fd(), msg, iovecs, address, *flags, submission);
        if let Some(buf_group) = buf.buffer_group() {
            submission.0.__bindgen_anon_4.buf_group = buf_group.0;
            submission.0.flags |= libc::IOSQE_BUFFER_SELECT;
        }
    }

    fn map_ok(
        _: &AsyncFd,
        (mut buf, msg, iovec, address): Self::Resources,
        (buf_id, n): OpReturn,
    ) -> Self::Output {
        asan::unpoison_iovecs_mut(slice::from_ref(&iovec));
        msan::unpoison_iovecs_mut(slice::from_ref(&iovec), n as usize);
        msan::unpoison_region(address.as_ptr().cast(), msg.address_len() as usize);
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { buf.buffer_init(BufId(buf_id), n) };
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

    fn fill_submission(
        fd: &AsyncFd,
        (_, msg, iovecs, address): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        fill_recvmsg_submission::<A>(fd.fd(), msg, iovecs, address, *flags, submission);
    }

    fn map_ok(
        _: &AsyncFd,
        (mut bufs, msg, iovecs, address): Self::Resources,
        (_, n): OpReturn,
    ) -> Self::Output {
        asan::unpoison_iovecs_mut(&iovecs);
        msan::unpoison_iovecs_mut(&iovecs, n as usize);
        msan::unpoison_region(address.as_ptr().cast(), msg.address_len() as usize);
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        unsafe { bufs.set_init(n as usize) };
        // SAFETY: kernel initialised the address for us.
        let address = unsafe { A::init(address, msg.address_len()) };
        (bufs, address, msg.flags())
    }
}

#[allow(clippy::cast_sign_loss)] // For flags as u32.
fn fill_recvmsg_submission<A: SocketAddress>(
    fd: RawFd,
    msg: &mut MsgHeader,
    iovecs: &mut [crate::io::IoMutSlice],
    address: &mut MaybeUninit<A::Storage>,
    flags: RecvFlag,
    submission: &mut sq::Submission,
) {
    // SAFETY: `address` and `iovecs` outlive `msg`.
    unsafe { msg.init_recv::<A>(address, iovecs) };

    submission.0.opcode = libc::IORING_OP_RECVMSG as u8;
    submission.0.fd = fd;
    submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
        addr: ptr::from_mut(&mut *msg).addr() as u64,
    };
    asan::poison_iovecs_mut(iovecs);
    submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { msg_flags: flags.0 };
    submission.0.len = 1;
}

pub(crate) struct SendOp<B>(PhantomData<*const B>);

impl<B: Buf> FdOp for SendOp<B> {
    type Output = usize;
    type Resources = B;
    type Args = (SendCall, SendFlag);

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
        let (buf_ptr, buf_len) = unsafe { buf.parts() };
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: buf_ptr.addr() as u64,
        };
        asan::poison_region(buf_ptr.cast(), buf_len as usize);
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { msg_flags: flags.0 };
        submission.0.len = buf_len;
    }

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, ret: OpReturn) -> Self::Output {
        Self::map_ok_extract(fd, resources, ret).1
    }
}

impl<B: Buf> FdOpExtract for SendOp<B> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(_: &AsyncFd, buf: Self::Resources, (_, n): OpReturn) -> Self::ExtractOutput {
        let (ptr, len) = unsafe { buf.parts() };
        asan::unpoison_region(ptr.cast(), len as usize);
        (buf, n as usize)
    }
}

pub(crate) struct SendToOp<B, A = NoAddress>(PhantomData<*const (B, A)>);

impl<B: Buf, A: SocketAddress> FdOp for SendToOp<B, A> {
    type Output = usize;
    type Resources = (B, AddressStorage<A::Storage>);
    type Args = (SendCall, SendFlag);

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
        let (buf_ptr, buf_len) = unsafe { buf.parts() };
        let (address_ptr, address_length) = unsafe { A::as_ptr(&address.0) };
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_1.addr2 = address_ptr.addr() as u64;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: buf_ptr.addr() as u64,
        };
        asan::poison_region(buf_ptr.cast(), buf_len as usize);
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { msg_flags: flags.0 };
        submission.0.__bindgen_anon_5.__bindgen_anon_1.addr_len = address_length as u16;
        submission.0.len = buf_len;
    }

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, ret: OpReturn) -> Self::Output {
        Self::map_ok_extract(fd, resources, ret).1
    }
}

impl<B: Buf, A: SocketAddress> FdOpExtract for SendToOp<B, A> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        (buf, _): Self::Resources,
        (_, n): OpReturn,
    ) -> Self::ExtractOutput {
        let (buf_ptr, buf_len) = unsafe { buf.parts() };
        asan::unpoison_region(buf_ptr.cast(), buf_len as usize);
        (buf, n as usize)
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

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd: &AsyncFd,
        (_, msg, iovecs, address): &mut Self::Resources,
        (send_op, flags): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        // SAFETY: `address` and `iovecs` outlive `msg`.
        unsafe { msg.init_send::<A>(&mut address.0, iovecs) };

        submission.0.opcode = match *send_op {
            SendCall::Normal => libc::IORING_OP_SENDMSG as u8,
            SendCall::ZeroCopy => libc::IORING_OP_SENDMSG_ZC as u8,
        };
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: ptr::from_mut(&mut *msg).addr() as u64,
        };
        asan::poison_iovecs(iovecs);
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { msg_flags: flags.0 };
        submission.0.len = 1;
    }

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, ret: OpReturn) -> Self::Output {
        Self::map_ok_extract(fd, resources, ret).1
    }
}

impl<B: BufSlice<N>, A: SocketAddress, const N: usize> FdOpExtract for SendMsgOp<B, A, N> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(
        _: &AsyncFd,
        (buf, _, iovecs, _): Self::Resources,
        (_, n): OpReturn,
    ) -> Self::ExtractOutput {
        asan::unpoison_iovecs(&iovecs);
        (buf, n as usize)
    }
}

pub(crate) struct AcceptOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> FdOp for AcceptOp<A> {
    type Output = (AsyncFd, A);
    type Resources = AddressStorage<(MaybeUninit<A::Storage>, libc::socklen_t)>;
    type Args = AcceptFlag;

    #[allow(clippy::cast_sign_loss)] // For flags as u32.
    fn fill_submission(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let fd_kind = fd.kind();
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
            accept_flags: flags.0 | fd_kind.cloexec_flag() as u32,
        };
        submission.0.flags |= libc::IOSQE_ASYNC;
        fd_kind.create_flags(submission);
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok(lfd: &AsyncFd, resources: Self::Resources, (_, fd): OpReturn) -> Self::Output {
        msan::unpoison_region(
            ptr::from_ref(&(resources.0).1).cast(),
            size_of::<libc::socklen_t>(),
        );
        msan::unpoison_region((resources.0).0.as_ptr().cast(), (resources.0).1 as usize);
        let sq = lfd.sq.clone();
        // SAFETY: the accept operation ensures that `fd` is valid.
        let socket = unsafe { AsyncFd::from_raw(fd as RawFd, lfd.kind(), sq) };
        // SAFETY: the kernel has written the address for us.
        let address = unsafe { A::init((resources.0).0, (resources.0).1) };
        (socket, address)
    }
}

pub(crate) struct MultishotAcceptOp;

impl FdIter for MultishotAcceptOp {
    type Output = AsyncFd;
    type Resources = ();
    type Args = AcceptFlag;

    #[allow(clippy::cast_sign_loss)] // For flags as u32.
    fn fill_submission(
        fd: &AsyncFd,
        (): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let fd_kind = fd.kind();
        submission.0.opcode = libc::IORING_OP_ACCEPT as u8;
        submission.0.ioprio = libc::IORING_ACCEPT_MULTISHOT as u16;
        submission.0.fd = fd.fd();
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            accept_flags: flags.0 | fd_kind.cloexec_flag() as u32,
        };
        submission.0.flags = libc::IOSQE_ASYNC;
        fd_kind.create_flags(submission);
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_next(lfd: &AsyncFd, (): &Self::Resources, (_, fd): OpReturn) -> Self::Output {
        let sq = lfd.sq.clone();
        // SAFETY: the accept operation ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as RawFd, lfd.kind(), sq) }
    }
}

pub(crate) struct SocketOptionOp<T>(PhantomData<*const T>);

impl<T> FdOp for SocketOptionOp<T> {
    type Output = T;
    type Resources = MaybeUninit<T>;
    type Args = (Level, Opt);

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
                level: level.0,
                optname: optname.0,
            },
        };
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            optlen: size_of::<T>() as u32,
        };
        submission.0.__bindgen_anon_6 = libc::io_uring_sqe__bindgen_ty_6 {
            optval: ManuallyDrop::new(value.as_mut_ptr().addr() as u64),
        };
    }

    fn map_ok(_: &AsyncFd, value: Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == (size_of::<T>() as u32));
        // SAFETY: the kernel initialised the value for us as part of the
        // getsockopt call.
        unsafe { MaybeUninit::assume_init(value) }
    }
}

pub(crate) struct SocketOption2Op<T>(PhantomData<*const T>);

impl<T: option::Get> FdOp for SocketOption2Op<T> {
    type Output = T::Output;
    type Resources = OptionStorage<MaybeUninit<T::Storage>>;
    type Args = (Level, Opt);

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
                level: level.0,
                optname: optname.0,
            },
        };
        // SAFETY: the kernel will initiase the value for us.
        let (optval, optlen) = unsafe { T::as_mut_ptr(&mut value.0) };
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 { optlen };
        submission.0.__bindgen_anon_6 = libc::io_uring_sqe__bindgen_ty_6 {
            optval: ManuallyDrop::new(optval.addr() as u64),
        };
    }

    fn map_ok(_: &AsyncFd, value: Self::Resources, (_, n): OpReturn) -> Self::Output {
        // SAFETY: the kernel initialised the value for us as part of the
        // getsockopt call.
        unsafe { T::init(value.0, n) }
    }
}

pub(crate) struct SetSocketOptionOp<T>(PhantomData<*const T>);

impl<T> FdOp for SetSocketOptionOp<T> {
    type Output = ();
    type Resources = T;
    type Args = (Level, Opt);

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
                level: level.0,
                optname: optname.0,
            },
        };
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            optlen: size_of::<T>() as u32,
        };
        submission.0.__bindgen_anon_6 = libc::io_uring_sqe__bindgen_ty_6 {
            optval: ManuallyDrop::new(ptr::from_ref(value).addr() as u64),
        };
    }

    fn map_ok(fd: &AsyncFd, resources: Self::Resources, ret: OpReturn) -> Self::Output {
        Self::map_ok_extract(fd, resources, ret);
    }
}

impl<T> FdOpExtract for SetSocketOptionOp<T> {
    type ExtractOutput = T;

    fn map_ok_extract(
        _: &AsyncFd,
        value: Self::Resources,
        (_, n): OpReturn,
    ) -> Self::ExtractOutput {
        debug_assert!(n == 0);
        value
    }
}

pub(crate) struct SetSocketOption2Op<T>(PhantomData<*const T>);

impl<T: option::Set> FdOp for SetSocketOption2Op<T> {
    type Output = ();
    type Resources = OptionStorage<T::Storage>;
    type Args = (Level, Opt);

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
                level: level.0,
                optname: optname.0,
            },
        };
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            optlen: size_of::<T::Storage>() as u32,
        };
        submission.0.__bindgen_anon_6 = libc::io_uring_sqe__bindgen_ty_6 {
            optval: ManuallyDrop::new(ptr::from_ref(&value.0).addr() as u64),
        };
    }

    fn map_ok(_: &AsyncFd, _: Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

pub(crate) struct ShutdownOp;

impl FdOp for ShutdownOp {
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

    fn map_ok(_: &AsyncFd, (): Self::Resources, (_, n): OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}
