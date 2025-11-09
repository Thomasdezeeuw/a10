use std::os::fd::RawFd;

use crate::io_uring::{self, cq, libc, sq};
use crate::pipe::PipeFlag;
use crate::{fd, AsyncFd, SubmissionQueue};

pub(crate) struct PipeOp;

impl io_uring::Op for PipeOp {
    type Output = [AsyncFd; 2];
    type Resources = (Box<[RawFd; 2]>, fd::Kind);
    type Args = PipeFlag;

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        (fds, fd_kind): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_PIPE as u8;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: (&raw mut **fds) as u64,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            pipe_flags: (flags.0 | fd_kind.cloexec_flag() as u32),
        };
        if let fd::Kind::Direct = *fd_kind {
            io_uring::fd::create_direct_flags(submission);
        }
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok(
        sq: &SubmissionQueue,
        (fds, fd_kind): Self::Resources,
        (_, res): cq::OpReturn,
    ) -> Self::Output {
        debug_assert!(res == 0);
        // SAFETY: kernel ensures that `fds` are valid.
        unsafe {
            [
                AsyncFd::from_raw(fds[0], fd_kind, sq.clone()),
                AsyncFd::from_raw(fds[1], fd_kind, sq.clone()),
            ]
        }
    }
}
