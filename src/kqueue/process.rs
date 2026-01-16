use std::io;
use std::mem::replace;
use std::task::{self, Poll};

use crate::kqueue::op::EventedState;
use crate::process::{Signal, WaitInfo, WaitOn, WaitOption};
use crate::{SubmissionQueue, syscall};

pub(crate) struct WaitIdOp;

impl crate::op::Op for WaitIdOp {
    type Output = io::Result<WaitInfo>;
    type Resources = WaitInfo;
    type Args = (WaitOn, WaitOption);
    type State = EventedState<Self::Resources, Self::Args>;

    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::Output> {
        match state {
            EventedState::NotStarted { args, .. } => {
                let (wait, _) = args;
                let pid = match *wait {
                    WaitOn::Process(pid) => pid,
                };
                sq.submissions().add(|event| {
                    event.0.filter = libc::EVFILT_PROC;
                    event.0.ident = pid as _;
                    event.0.flags |= libc::EV_RECEIPT | libc::EV_ONESHOT | libc::EV_ADD;
                    event.0.fflags = libc::NOTE_EXIT;
                    // Wake the Future once the process has exited.
                    event.0.udata = Box::into_raw(Box::new(ctx.waker().clone())).cast();
                });

                // Set ourselves to waiting for an event from the kernel.
                if let EventedState::NotStarted { resources, args } =
                    replace(state, EventedState::Complete)
                {
                    *state = EventedState::Waiting { resources, args };
                }
                // We've added our waker above to the list, we'll be woken up
                // once we can make progress.
                Poll::Pending
            }
            EventedState::Waiting { resources, args } => {
                let info = resources;
                let (wait, options) = args;
                let (id_type, pid) = match *wait {
                    WaitOn::Process(pid) => (libc::P_PID, pid),
                };

                let options = options.0 as libc::c_int | libc::WNOHANG; // Don't block.
                syscall!(waitid(id_type, pid, &mut info.0, options))?;

                if info.0.si_pid == 0 {
                    // Got polled without the process stopping, will have to
                    // wait again.
                    Poll::Pending
                } else {
                    let info = WaitInfo(info.0);
                    *state = EventedState::Complete;
                    Poll::Ready(Ok(info))
                }
            }
            // Shouldn't be reachable, but if the Future is used incorrectly it
            // can be.
            EventedState::Complete => panic!("polled Future after completion"),
        }
    }
}

// kqueue doesn't give us a lot of info.
pub(crate) use crate::process::Signal as SignalInfo;

#[allow(clippy::trivially_copy_pass_by_ref)]
pub(crate) const fn signal(info: &SignalInfo) -> Signal {
    *info
}
