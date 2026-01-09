use std::os::fd::RawFd;

use crate::io_uring::op::{Iter, Op, OpReturn};
use crate::io_uring::{self, cq, libc, sq};
use crate::poll::{Event, Interest};
use crate::SubmissionQueue;

pub(crate) struct OneshotPollOp;

impl Op for OneshotPollOp {
    type Output = Event;
    type Resources = ();
    type Args = (RawFd, Interest);

    fn fill_submission(
        (): &mut Self::Resources,
        (fd, interest): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_POLL_ADD as u8;
        submission.0.fd = *fd;
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            poll32_events: interest.0,
        };
    }

    #[allow(clippy::cast_possible_wrap)] // For events as i32.
    fn map_ok(_: &SubmissionQueue, (): Self::Resources, (_, events): OpReturn) -> Self::Output {
        Event(events as libc::c_int)
    }
}

pub(crate) struct MultishotPollOp;

impl Iter for MultishotPollOp {
    type Output = Event;
    type Resources = ();
    type Args = (RawFd, Interest);

    fn fill_submission(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        OneshotPollOp::fill_submission(resources, args, submission);
        submission.0.len = libc::IORING_POLL_ADD_MULTI;
    }

    #[allow(clippy::cast_possible_wrap)] // For events as i32.
    fn map_next(_: &SubmissionQueue, (): &Self::Resources, (_, events): OpReturn) -> Self::Output {
        Event(events as libc::c_int)
    }
}
