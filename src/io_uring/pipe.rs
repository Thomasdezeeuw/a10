use std::io;
use std::os::fd::RawFd;

use crate::io_uring::op::{CompletionFlags, Op, OpReturn};
use crate::io_uring::{libc, sq};
use crate::pipe::PipeFlag;
use crate::{AsyncFd, SubmissionQueue, fd, syscall};

pub(crate) struct PipeOp;

impl Op for PipeOp {
    type Output = [AsyncFd; 2];
    type Resources = ([RawFd; 2], fd::Kind);
    type Args = PipeFlag;

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        (fds, fd_kind): &mut Self::Resources,
        flags: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_PIPE as u8;
        submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: (&raw mut *fds) as u64,
        };
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            pipe_flags: (flags.0 | fd_kind.cloexec_flag() as u32),
        };
        fd_kind.create_flags(submission);
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok(
        sq: &SubmissionQueue,
        (fds, fd_kind): Self::Resources,
        (_, res): OpReturn,
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

    fn fallback(
        sq: &SubmissionQueue,
        (mut fds, _): Self::Resources,
        err: io::Error,
    ) -> io::Result<Self::Output> {
        if let Some(libc::EINVAL) = err.raw_os_error() {
            let res = syscall!(pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC))?;
            let flags = CompletionFlags::empty();
            Ok(Self::map_ok(sq, (fds, fd::Kind::File), (flags, res as u32)))
        } else {
            Err(err)
        }
    }
}
