use std::io;
use std::task::{self, Poll};

use crate::SubmissionQueue;
use crate::io_uring::libc;
use crate::io_uring::op::{Multishot, State, fallback, poll_next};
use crate::poll::PollableState;

pub(crate) struct PollableOp;

impl crate::op::Iter for PollableOp {
    type Output = io::Result<()>;
    type Resources = PollableState;
    type Args = ();
    type State = State<Multishot, Self::Resources, Self::Args>;

    fn poll_next(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Option<Self::Output>> {
        poll_next(
            sq,
            state,
            ctx,
            |_, state, (), submission| {
                submission.0.opcode = libc::IORING_OP_POLL_ADD as u8;
                submission.0.fd = state.sq.submissions().ring_fd();
                submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
                    poll32_events: (libc::EPOLLIN
                        | libc::EPOLLHUP
                        | libc::EPOLLERR
                        | libc::EPOLLET
                        | libc::EPOLLEXCLUSIVE)
                        .cast_unsigned(),
                };
                submission.0.len = libc::IORING_POLL_ADD_MULTI;
            },
            |_, _, _| (),
            |_, _, (), err| Err(fallback(err)),
        )
    }
}
