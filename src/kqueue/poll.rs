use std::io;
use std::mem::replace;
use std::task::{self, Poll};

use crate::SubmissionQueue;
use crate::kqueue::fd::OpKind;
use crate::kqueue::op::EventedState;
use crate::poll::PollableState;

pub(crate) struct PollableOp;

impl crate::op::Iter for PollableOp {
    type Output = io::Result<()>;
    type Resources = PollableState;
    type Args = ();
    type State = EventedState<Self::Resources, Self::Args>;

    fn poll_next(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Option<Self::Output>> {
        const OP: OpKind = OpKind::Read;

        match state {
            EventedState::NotStarted { resources, .. }
            | EventedState::ToSubmit { resources, .. } => {
                // Add ourselves to the waiters for the operation.
                let fd_state = &resources.state;
                let needs_register = {
                    let mut fd_state = fd_state.lock();
                    let needs_register = !fd_state.has_waiting_op(OP);
                    fd_state.add(OP, ctx.waker().clone());
                    needs_register
                }; // Unlock fd state.

                // If we're to first we need to register an event with the
                // kernel.
                if needs_register {
                    sq.submissions().add(|event| {
                        event.0.filter = libc::EVFILT_READ;
                        event.0.ident = resources.sq.submissions().fd().cast_unsigned() as _;
                        event.0.udata = fd_state.as_udata();
                    });
                }

                // Set ourselves to waiting for an event from the kernel.
                if let EventedState::NotStarted { resources, args }
                | EventedState::ToSubmit { resources, args } =
                    replace(state, EventedState::Complete)
                {
                    *state = EventedState::Waiting { resources, args };
                }
                // We've added our waker above to the list, we'll be woken up
                // once we can make progress.
                Poll::Pending
            }
            EventedState::Waiting { resources, .. } => {
                if resources.state.lock().has_waiting_op(OP) {
                    // Polled before we got an event.
                    Poll::Pending
                } else {
                    // Return Ok and reset the state to wait for another event.
                    if let EventedState::Waiting { resources, args } =
                        replace(state, EventedState::Complete)
                    {
                        *state = EventedState::ToSubmit { resources, args };
                    }
                    Poll::Ready(Some(Ok(())))
                }
            }
            EventedState::Complete => Poll::Ready(None),
        }
    }
}
