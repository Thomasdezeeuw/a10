use std::os::fd::RawFd;

use crate::io_uring::{self, cq, libc, sq};
use crate::op::Iter;
use crate::poll::PollEvent;
use crate::SubmissionQueue;

pub(crate) struct OneshotPollOp;

impl io_uring::Op for OneshotPollOp {
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
            poll32_events: *mask as u32,
        };
    }

    fn map_ok(_: &SubmissionQueue, (): Self::Resources, (_, events): cq::OpReturn) -> Self::Output {
        PollEvent(events as libc::c_int)
    }
}

pub(crate) struct MultishotPollOp;

impl io_uring::Op for MultishotPollOp {
    type Output = PollEvent;
    type Resources = ();
    type Args = (RawFd, libc::c_int); // mask;

    fn fill_submission(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        OneshotPollOp::fill_submission(resources, args, submission);
        submission.0.len = libc::IORING_POLL_ADD_MULTI;
    }

    fn map_ok(sq: &SubmissionQueue, (): Self::Resources, ok: cq::OpReturn) -> Self::Output {
        MultishotPollOp::map_next(sq, &mut (), ok)
    }
}

impl Iter for MultishotPollOp {
    fn map_next(
        _: &SubmissionQueue,
        (): &mut Self::Resources,
        (_, events): cq::OpReturn,
    ) -> Self::Output {
        PollEvent(events as libc::c_int)
    }
}
