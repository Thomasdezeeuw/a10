use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{ptr, slice};

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice, ReadBuf, ReadBufPool};
use crate::io_uring::op::{FdIter, FdOp, Op, OpReturn, Singleshot, State};
use crate::io_uring::{self, cq, libc, sq};
use crate::net::{
    AcceptFlag, AddressStorage, Domain, Level, Name, NoAddress, Opt, OptionStorage, Protocol,
    RecvFlag, SendCall, SendFlag, SocketAddress, Type,
};
use crate::{asan, fd, msan, AsyncFd, SubmissionQueue};

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
        /* TODO.
        if let fd::Kind::Direct = *fd_kind {
            io_uring::fd::create_direct_flags(submission);
        }
        */
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
    type Resources = AddressStorage<Box<A::Storage>>;
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
        asan::poison_box(&address.0);
    }

    fn map_ok(_: &AsyncFd, address: Self::Resources, (_, n): OpReturn) -> Self::Output {
        asan::unpoison_box(&address.0);
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
        asan::poison_box(&address.0);
    }

    fn map_ok(_: &AsyncFd, address: Self::Resources, (_, n): OpReturn) -> Self::Output {
        asan::unpoison_box(&address.0);
        debug_assert!(n == 0);
    }
}

pub(crate) struct SocketNameOp<A>(PhantomData<*const A>);

impl<A: SocketAddress> FdOp for SocketNameOp<A> {
    type Output = A;
    type Resources = AddressStorage<Box<(MaybeUninit<A::Storage>, libc::socklen_t)>>;
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
        asan::poison_box(&resources.0);
    }

    fn map_ok(_: &AsyncFd, resources: Self::Resources, _: OpReturn) -> Self::Output {
        asan::unpoison_box(&resources.0);
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
