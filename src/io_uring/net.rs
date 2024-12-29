use std::marker::PhantomData;

use crate::fd::{AsyncFd, Descriptor};
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
