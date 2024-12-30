use std::marker::PhantomData;

use crate::fd::{AsyncFd, Descriptor};
use crate::io::{Buf, BufId, BufMut, Buffer};
use crate::net::{AddressStorage, SendCall, SocketAddress};
use crate::op::FdOpExtract;
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

pub(crate) struct SendOp<B>(PhantomData<*const B>);

impl<B: Buf> sys::FdOp for SendOp<B> {
    type Output = usize;
    type Resources = Buffer<B>;
    type Args = (SendCall, libc::c_int); // send_op, flags

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
