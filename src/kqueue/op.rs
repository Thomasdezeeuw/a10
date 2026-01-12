use std::marker::PhantomData;
use std::mem::replace;
use std::task::{self, Poll};
use std::{fmt, io};

use crate::kqueue::fd::OpKind;
use crate::kqueue::Event;
use crate::op::OpState;
use crate::{AsyncFd, SubmissionQueue};

/// State of an operation that is done synchronously, e.g. opening a socket.
#[derive(Debug)]
pub(crate) enum DirectState<R, A> {
    /// Operation has not started yet.
    NotStarted { resources: R, args: A },
    /// Last state where the operation was fully cleaned up.
    Complete,
}

impl<R, A> OpState for DirectState<R, A> {
    type Resources = R;
    type Args = A;

    fn new(resources: Self::Resources, args: Self::Args) -> Self {
        DirectState::NotStarted { resources, args }
    }

    fn resources_mut(&mut self) -> Option<&mut Self::Resources> {
        if let DirectState::NotStarted { resources, .. } = self {
            Some(resources)
        } else {
            None
        }
    }

    fn args_mut(&mut self) -> Option<&mut Self::Args> {
        if let DirectState::NotStarted { args, .. } = self {
            Some(args)
        } else {
            None
        }
    }

    unsafe fn drop(&mut self, sq: &SubmissionQueue) {
        // Nothing special to do.
    }
}

/// Operation that is done using a synchronous function.
pub(crate) trait DirectOp {
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

impl<T: DirectOp> crate::op::Op for T {
    type Output = io::Result<T::Output>;
    type Resources = T::Resources;
    type Args = T::Args;
    type State = DirectState<T::Resources, T::Args>;

    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::Output> {
        match replace(state, DirectState::Complete) {
            DirectState::NotStarted { resources, args } => Poll::Ready(T::run(sq, resources, args)),
            // Shouldn't be reachable, but if the Future is used incorrectly it
            // can be.
            DirectState::Complete => panic!("polled Future after completion"),
        }
    }
}

/// Same as [`DirectOp`], but for operations using file descriptors.
pub(crate) trait DirectFdOp {
    type Output;
    type Resources;
    type Args;

    /// Same as [`DirectOp::run`], but using an `AsyncFd`.
    fn run(fd: &AsyncFd, resources: Self::Resources, args: Self::Args) -> io::Result<Self::Output>;
}

/// Macro to implement the FdOp trait.
//
// NOTE: this should be a simple implementation:
//   impl<T: DirectFdOp> crate::op::FdOp for T
// But that conflicts with the FdOp implementation for T below, causing E0119.
macro_rules! impl_fd_op {
    ( $( $T: ident $( < $( $gen: ident ),+ > )? ),* ) => {
        $(
        impl $( < $( $gen ),+ > )? crate::op::FdOp for $T $( < $( $gen ),* > )?
            where Self: $crate::kqueue::op::DirectFdOp,
        {
            type Output = ::std::io::Result<<Self as $crate::kqueue::op::DirectFdOp>::Output>;
            type Resources = <Self as $crate::kqueue::op::DirectFdOp>::Resources;
            type Args = <Self as $crate::kqueue::op::DirectFdOp>::Args;
            type State = $crate::kqueue::op::DirectState<<Self as $crate::kqueue::op::DirectFdOp>::Resources, <Self as $crate::kqueue::op::DirectFdOp>::Args>;

            fn poll(
                state: &mut Self::State,
                ctx: &mut ::std::task::Context<'_>,
                fd: &$crate::AsyncFd,
            ) -> ::std::task::Poll<Self::Output> {
                match ::std::mem::replace(state, $crate::kqueue::op::DirectState::Complete) {
                    $crate::kqueue::op::DirectState::NotStarted { resources, args } => ::std::task::Poll::Ready(Self::run(fd, resources, args)),
                    // Shouldn't be reachable, but if the Future is used incorrectly it
                    // can be.
                    $crate::kqueue::op::DirectState::Complete => ::std::panic!("polled Future after completion"),
                }
            }
        }
        )*
    };
}

pub(super) use impl_fd_op;

/// State of an operation that whats for an event (on a file descriptor) first.
#[derive(Debug)]
pub(crate) enum EventedState<R, A> {
    /// Operation has not started yet.
    NotStarted { resources: R, args: A },
    /// Event was submitted, waiting for a result.
    Waiting { resources: R, args: A },
    /// Last state where the operation was fully cleaned up.
    Complete,
}

impl<R, A> OpState for EventedState<R, A> {
    type Resources = R;
    type Args = A;

    fn new(resources: Self::Resources, args: Self::Args) -> Self {
        EventedState::NotStarted { resources, args }
    }

    fn resources_mut(&mut self) -> Option<&mut Self::Resources> {
        if let EventedState::NotStarted { resources, .. } = self {
            Some(resources)
        } else {
            None
        }
    }

    fn args_mut(&mut self) -> Option<&mut Self::Args> {
        if let EventedState::NotStarted { args, .. } = self {
            Some(args)
        } else {
            None
        }
    }

    unsafe fn drop(&mut self, sq: &SubmissionQueue) {
        // Nothing special to do.
    }
}

/// Operation on a file descriptor that waits for an event first and uses
/// non-blocking I/O.
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
    type State = EventedState<T::Resources, T::Args>;

    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Self::Output> {
        loop {
            match state {
                EventedState::NotStarted { .. } => {
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
                    if let EventedState::NotStarted { resources, args } =
                        replace(state, EventedState::Complete)
                    {
                        *state = EventedState::Waiting { resources, args };
                    }
                    // We've added our waker above to the list, we'll be woken up
                    // once we can make progress.
                    return Poll::Pending;
                }
                EventedState::Waiting { resources, args } => {
                    match T::try_run(fd, resources, args) {
                        Ok(res) => {
                            if let EventedState::Waiting { resources, args } =
                                replace(state, EventedState::Complete)
                            {
                                return Poll::Ready(Ok(T::map_ok(fd, resources, res)));
                            }
                        }
                        Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                            if let EventedState::Waiting { resources, args } =
                                replace(state, EventedState::Complete)
                            {
                                *state = EventedState::NotStarted { resources, args };
                                // Try again in the next loop iteration.
                                continue;
                            }
                        }
                        Err(err) => {
                            *state = EventedState::Complete;
                            return Poll::Ready(Err(err));
                        }
                    }
                }
                // Shouldn't be reachable, but if the Future is used incorrectly it
                // can be.
                EventedState::Complete => panic!("polled Future after completion"),
            }
        }
    }
}
