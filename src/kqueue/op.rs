use std::marker::PhantomData;
use std::mem::replace;
use std::task::{self, Poll};
use std::{fmt, io};

use crate::kqueue::fd::OpKind;
use crate::kqueue::Event;
use crate::op::OpState;
use crate::{AsyncFd, SubmissionQueue};

/// For (not fd) operations we keep a simple state as the operation is
/// synchronous.
#[derive(Debug)]
pub(crate) enum State<R, A> {
    /// Operation has not started yet.
    NotStarted { resources: R, args: A },
    /// Last state where the operation was fully cleaned up.
    Complete,
}

impl<R, A> OpState for State<R, A> {
    type Resources = R;
    type Args = A;

    fn new(resources: Self::Resources, args: Self::Args) -> Self {
        State::NotStarted { resources, args }
    }
}

pub(crate) trait Op {
    type Output;
    type Resources;
    type Args;

    /// Run the synchronous operation.
    fn run(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        args: Self::Args,
    ) -> io::Result<Self::Output>;
}

impl<T: Op> crate::op::Op for T {
    type Output = io::Result<T::Output>;
    type Resources = T::Resources;
    type Args = T::Args;
    type State = State<T::Resources, T::Args>;

    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::Output> {
        match replace(state, State::Complete) {
            State::NotStarted { resources, args } => Poll::Ready(T::run(sq, resources, args)),
            // Shouldn't be reachable, but if the Future is used incorrectly it
            // can be.
            State::Complete => panic!("polled Future after completion"),
        }
    }
}

/// State of an operation on a file descriptor.
#[derive(Debug)]
pub(crate) enum FdState<R, A> {
    /// Operation has not started yet.
    NotStarted { resources: R, args: A },
    /// Event was submitted, waiting for a result.
    Waiting { resources: R, args: A },
    /// Last state where the operation was fully cleaned up.
    Complete,
}

impl<R, A> OpState for FdState<R, A> {
    type Resources = R;
    type Args = A;

    fn new(resources: Self::Resources, args: Self::Args) -> Self {
        FdState::NotStarted { resources, args }
    }
}

pub(crate) trait FdOp {
    type Output;
    type Resources;
    type Args;
    type OperationOutput;

    /// What kind of operation is being done.
    const OP_KIND: OpKind;

    /// Try the operation.
    ///
    /// If this returns [`WouldBlock`] the operation is tried again.
    ///
    /// [`WouldBlock`]: std::io::Error::WouldBlock
    fn try_run(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
    ) -> io::Result<Self::OperationOutput>;

    /// Map a succesful operation result.
    ///
    /// It's always the `Ok(OperationOutput)` from `try_run`.
    fn map_ok(
        fd: &AsyncFd,
        resources: Self::Resources,
        output: Self::OperationOutput,
    ) -> Self::Output;
}

impl<T: FdOp> crate::op::FdOp for T {
    type Output = io::Result<T::Output>;
    type Resources = T::Resources;
    type Args = T::Args;
    type State = FdState<T::Resources, T::Args>;

    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Self::Output> {
        loop {
            match state {
                FdState::NotStarted { .. } => {
                    let fd_state = fd.state();
                    // Add ourselves to the waiters for the operation.
                    let needs_register = {
                        let mut fd_state = fd_state.lock();
                        let needs_register = !fd_state.has_waiting_op(T::OP_KIND);
                        fd_state.add(T::OP_KIND, ctx.waker().clone());
                        needs_register
                    }; // Unlock fd state.

                    // If we're to first we need to register an event with the
                    // kernel.
                    if needs_register {
                        fd.sq.submissions().add(|event| {
                            event.0.filter = match T::OP_KIND {
                                OpKind::Read => libc::EVFILT_READ,
                                OpKind::Write => libc::EVFILT_WRITE,
                            };
                            event.0.ident = fd.fd() as _;
                            event.0.udata = fd_state.as_udata();
                        });
                    }

                    // Set ourselves to waiting for an event from the kernel.
                    if let FdState::NotStarted { resources, args } =
                        replace(state, FdState::Complete)
                    {
                        *state = FdState::Waiting { resources, args };
                    }
                    // We've added our waker above to the list, we'll be woken up
                    // once we can make progress.
                    return Poll::Pending;
                }
                FdState::Waiting { resources, args } => {
                    match T::try_run(fd, resources, args) {
                        Ok(res) => {
                            if let FdState::Waiting { resources, args } =
                                replace(state, FdState::Complete)
                            {
                                return Poll::Ready(Ok(T::map_ok(fd, resources, res)));
                            }
                        }
                        Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                            if let FdState::Waiting { resources, args } =
                                replace(state, FdState::Complete)
                            {
                                *state = FdState::NotStarted { resources, args };
                                // Try again in the next loop iteration.
                                continue;
                            }
                        }
                        Err(err) => {
                            *state = FdState::Complete;
                            return Poll::Ready(Err(err));
                        }
                    }
                }
                // Shouldn't be reachable, but if the Future is used incorrectly it
                // can be.
                FdState::Complete => panic!("polled Future after completion"),
            }
        }
    }
}
