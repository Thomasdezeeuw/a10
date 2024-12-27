use std::ffi::CString;
use std::marker::PhantomData;

use crate::fd::{AsyncFd, Descriptor};
use crate::sys::{self, cq, libc, sq};
use crate::SubmissionQueue;

pub(crate) struct OpenOp<D>(PhantomData<*const D>);

impl<D: Descriptor> sys::Op for OpenOp<D> {
    type Output = AsyncFd<D>;
    type Resources = CString; // path.
    type Args = (libc::c_int, libc::mode_t); // flags, mode.

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        path: &mut Self::Resources,
        (flags, mode): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_OPENAT as u8;
        submission.0.fd = libc::AT_FDCWD;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: path.as_ptr() as _,
        };
        submission.0.len = *mode;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            open_flags: *flags as _,
        };
        D::create_flags(submission);
    }

    fn map_ok(sq: &SubmissionQueue, _: Self::Resources, (_, fd): cq::OpReturn) -> Self::Output {
        // SAFETY: kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as _, sq.clone()) }
    }
}
