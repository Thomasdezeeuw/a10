use std::os::fd::RawFd;

use crate::poll::PollEvent;
use crate::sys::{self, cq, libc, sq};
use crate::SubmissionQueue;

pub(crate) struct OneshotPollOp;

impl sys::Op for OneshotPollOp {
    type Output = PollEvent;
    type Resources = ();
    type Args = (RawFd, libc::c_int); // mask;

    fn fill_submission(
        (): &mut Self::Resources,
        (fd, mask): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_POLL_ADD as u8;
        submission.0.fd = *fd;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            poll32_events: *mask as _,
        };
    }

    fn map_ok(_: &SubmissionQueue, (): Self::Resources, (_, events): cq::OpReturn) -> Self::Output {
        PollEvent(events as _)
    }
}
