use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ptr;

use crate::fd::{AsyncFd, Descriptor};
use crate::io::{Buf, BufId, BufMut, Buffer, ReadBuf, ReadBufPool};
use crate::net::{AddressStorage, SendCall, SocketAddress};
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

    fn map_ok(_: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
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

    fn map_ok(mut buf: Self::Resources, (buf_id, n): cq::OpReturn) -> Self::Output {
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

    fn map_ok(mut buf_pool: Self::Resources, (buf_id, n): cq::OpReturn) -> Self::Output {
        MultishotRecvOp::map_next(&mut buf_pool, (buf_id, n))
    }
}

impl FdIter for MultishotRecvOp {
    fn map_next(buf_pool: &mut Self::Resources, (buf_id, n): cq::OpReturn) -> Self::Output {
        // SAFETY: the kernel initialised the buffers for us as part of the read
        // call.
        unsafe { buf_pool.new_buffer(BufId(buf_id), n) }
    }
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

    fn map_ok(_: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        n as usize
    }
}

impl<B: Buf> FdOpExtract for SendOp<B> {
    type ExtractOutput = (B, usize);

    fn map_ok_extract(buf: Self::Resources, (_, n): Self::OperationOutput) -> Self::ExtractOutput {
        (buf.buf, n as usize)
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

    fn map_ok(value: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
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

    fn map_ok(_: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
    }
}

impl<T> FdOpExtract for SetSocketOptionOp<T> {
    type ExtractOutput = T;

    fn map_ok_extract(
        value: Self::Resources,
        (_, n): Self::OperationOutput,
    ) -> Self::ExtractOutput {
        debug_assert!(n == 0);
        *value
    }
}
