use std::io;
use std::mem::replace;
use std::task::{self, Poll};

use crate::kqueue::fd::OpKind;
use crate::op::OpState;
use crate::{AsyncFd, SubmissionQueue};

// # Usage
//
// We have two kinds of states for operations DirectState and EventedState. The
// first is for operations for which we can't poll for readiness, e.g.
// socket(2). The latter is for operations for which we can poll for readiness,
// e.g. read(2).
//
// ## Direct
//
// For a direct operation (not to be confused with direct descriptors) we create
// a new DirectState. The operation has unique access to the entire state for
// the entire duration. The following in the common flow.
//
// 1. The DirectState is created as NotStarted.
// 2. The operation is polled, the state is set to Complete, and using the
//    resources and arguments the operation is executed.
//
// Yes, it's that simple.
//
// ## Evented
//
// For evented operations the flow is a little more complex.
//
// 1. The EventedState is created as NotStarted.
// 2. After the operation is polled the Future's Waker is added to the list of
//    waiting futures stored in fd::State (see fd::OpState for more details).
//    If this operation is the first OpKind in the list it will also submit an
//    event to poll for the readiness.
// 3. Once we get a readiness event for the fd we wake all operation waiting for
//    that kind of operation (OpKind).
// 4. Once the operation is awoken again, not in the Waiting state, it will try
//    the operation. If this returns a WouldBlock error we got back to step 2
//    and set the state to NotStarted. If it does succeed we return the result
//    and we're done.
//
// # Dropping
//
// Since neither the DirectState or EventedState shares any resources with the
// OS we can safely drop both of them without any special handling.
//
// This is not true for fd::State (part of AsyncFd). Because there could be
// outstanding readiness polls that use the fd::State as user_data we delay the
// allocation. We do this by checking if the state has been initialised and if
// there are any pending ops. If there are no pending ops we can safely drop the
// state. If there are pending ops we need to delay the deallcation since a
// readiness poll could be returned at any time in the completion queue, which
// would access the state.
//
// We delay the deallcation by sending a user-space event (EVFILT_USER) to the
// polling thread with the pointer as identifier, which returns as user_data in
// the completion event where we can safely deallocate it. Since we close the
// file descriptor *before* we send this event we're ensured that any previous
// events that may hold a pointer to the fd::State will have been processed.

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

    unsafe fn drop(&mut self, _: &SubmissionQueue) {
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
        _: &mut task::Context<'_>,
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

/// Operation that is done using a synchronous function.
pub(crate) trait DirectOpExtract: DirectOp {
    /// Extracted output of the operation.
    type ExtractOutput;

    /// Same as [`DirectOp::run`], but returns extracted output.
    fn run_extract(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        args: Self::Args,
    ) -> io::Result<Self::ExtractOutput>;
}

impl<T: DirectOpExtract> crate::op::OpExtract for T {
    type ExtractOutput = io::Result<<Self as DirectOpExtract>::ExtractOutput>;

    fn poll_extract(
        state: &mut Self::State,
        _: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::ExtractOutput> {
        match replace(state, DirectState::Complete) {
            DirectState::NotStarted { resources, args } => {
                Poll::Ready(T::run_extract(sq, resources, args))
            }
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
                _: &mut ::std::task::Context<'_>,
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
    /// Ran the setup, not yet submitted an event, or need to submit an event
    /// again.
    ToSubmit { resources: R, args: A },
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

    unsafe fn drop(&mut self, _: &SubmissionQueue) {
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

    /// Setup to run *before* waiting for an event.
    ///
    /// Defaults to doing nothing.
    fn setup(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
    ) -> io::Result<()> {
        _ = (fd, resources, args);
        Ok(())
    }

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
        poll::<T, _>(state, ctx, fd, T::map_ok)
    }
}

pub(crate) trait FdOpExtract: FdOp {
    /// Extracted output of the operation.
    type ExtractOutput;

    /// Same as [`FdOp::map_ok`], returning the extract output.
    fn map_ok_extract(
        fd: &AsyncFd,
        resources: Self::Resources,
        output: Self::OperationOutput,
    ) -> Self::ExtractOutput;
}

impl<T: FdOpExtract> crate::op::FdOpExtract for T {
    type ExtractOutput = io::Result<T::ExtractOutput>;

    fn poll_extract(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Self::ExtractOutput> {
        poll::<T, _>(state, ctx, fd, T::map_ok_extract)
    }
}

#[allow(clippy::needless_pass_by_ref_mut)] // for ctx, matches Future::poll.
fn poll<T: FdOp, Out>(
    state: &mut EventedState<T::Resources, T::Args>,
    ctx: &mut task::Context<'_>,
    fd: &AsyncFd,
    map_ok: impl FnOnce(&AsyncFd, T::Resources, T::OperationOutput) -> Out,
) -> Poll<io::Result<Out>> {
    loop {
        match state {
            EventedState::NotStarted { resources, args } => {
                // Perform any setup required before waiting for an event.
                T::setup(fd, resources, args)?;

                if let EventedState::NotStarted { resources, args } =
                    replace(state, EventedState::Complete)
                {
                    *state = EventedState::ToSubmit { resources, args };
                    // Continue in the next loop iteration.
                }
            }
            EventedState::ToSubmit { .. } => {
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
                        event.0.ident = fd.fd().cast_unsigned() as _;
                        event.0.udata = fd_state.as_udata();
                    });
                }

                // Set ourselves to waiting for an event from the kernel.
                if let EventedState::ToSubmit { resources, args } =
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
                        if let EventedState::Waiting { resources, .. } =
                            replace(state, EventedState::Complete)
                        {
                            return Poll::Ready(Ok(map_ok(fd, resources, res)));
                        }
                    }
                    Err(ref err) if err.kind() == io::ErrorKind::WouldBlock => {
                        if let EventedState::Waiting { resources, args } =
                            replace(state, EventedState::Complete)
                        {
                            *state = EventedState::ToSubmit { resources, args };
                            // Try again in the next loop iteration.
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
