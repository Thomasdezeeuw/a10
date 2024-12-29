use std::marker::PhantomData;

use crate::fd::{AsyncFd, Descriptor};
use crate::net::{AddressStorage, SocketAddress};
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
