//! Module with the [`op_future`] and [`op_async_iter`] macros.

use std::cell::UnsafeCell;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io, mem};

use crate::fd::{AsyncFd, Descriptor, File};
use crate::io::BufMut;
use crate::sq::QueueFull;
use crate::{cq, man_link, sq, sys, syscall, Implementation, OperationId};

/// Generic [`Future`] that powers other I/O operation futures.
#[derive(Debug)]
pub(crate) struct Operation<'fd, O: Op, D: Descriptor = File> {
    fd: &'fd AsyncFd<D>,
    state: State<O::Resources, O::Args>,
}

impl<'fd, O: Op, D: Descriptor> Operation<'fd, O, D> {
    /// Create a new `Operation`.
    pub(crate) const fn new(
        fd: &'fd AsyncFd<D>,
        resources: O::Resources,
        args: O::Args,
    ) -> Operation<'fd, O, D> {
        Operation {
            fd,
            state: State::NotStarted {
                resources: UnsafeCell::new(resources),
                args,
            },
        }
    }
}

impl<'fd, O, D> Future for Operation<'fd, O, D>
where
    // TODO: this is silly.
    O: Op<
        Submission = <<sys::Implementation as crate::Implementation>::Submissions as sq::Submissions>::Submission,
        CompletionState = <<<sys::Implementation as crate::Implementation>::Completions as cq::Completions>::Event as cq::Event>::State,
    >,
    O::Resources: Unpin,
    O::Args: Unpin,
    D: Descriptor + Unpin,
{
    type Output = io::Result<O::Output>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let Operation { fd, state } = self.get_mut();
        match state {
            State::NotStarted { resources, args } => {
                let result = fd.sq().inner.submit(
                    |submission| O::fill_submission(fd, resources.get_mut(), args, submission),
                    ctx.waker().clone(),
                );
                if let Ok(op_id) = result {
                    state.running(op_id);
                }
                // We'll be awoken once the operation is ready.
                return Poll::Pending;
            }
            State::Running { resources, args, op_id } => {
                // SAFETY: we've ensured that `op_id` is valid.
                let mut queued_op_slot = unsafe { fd.sq().get_op(*op_id) };
                let result = match queued_op_slot.as_mut() {
                    Some(queued_op) => O::check_result(fd, resources.get_mut(), args, &mut queued_op.state),
                    // Somehow the queued operation is gone. This shouldn't
                    // happen, but we'll deal with it anyway.
                    None => OpResult::Again,
                };
                drop(queued_op_slot); // Unlock.
                match result {
                    OpResult::Ok(ok) => {
                        let resources = state.done();
                        Poll::Ready(Ok(O::map_ok(resources, ok)))
                    }
                    OpResult::Again => {
                        // Operation wasn't completed, need to try again.
                        let result = fd.sq().inner.resubmit(
                            *op_id,
                            |submission| O::fill_submission(fd, resources.get_mut(), args, submission),
                        );
                        match result {
                            Ok(()) => { /* Running again using the same operation id. */ }
                            Err(QueueFull) => state.not_started(),
                        }
                        // We'll be awoken once the operation is ready again or
                        // if we can submit again (in case of QueueFull).
                        return Poll::Pending;
                    }
                    OpResult::Err(err) => {
                        *state = State::Done;
                        Poll::Ready(Err(err))
                    }
                }
            }
            State::Done => unreachable!("a10::Read polled after completion"),
        }
    }
}

/// State of an [`Operation`].
///
/// Generics:
///  * `R` is [`Op::Resources`].
///  * `A` is [`Op::Args`].
#[derive(Debug)]
enum State<R, A> {
    /// Operation has not started yet. First has to be submitted.
    NotStarted { resources: UnsafeCell<R>, args: A },
    /// Operation has been submitted and is running.
    Running {
        resources: UnsafeCell<R>,
        args: A,
        op_id: OperationId,
    },
    /// Operation is done, don't poll again.
    Done,
}

impl<R, A> State<R, A> {
    /// Marks the state as not started.
    ///
    /// # Panics
    ///
    /// Panics if `self` is `Done`.
    fn not_started(&mut self) {
        let (resources, args) = match mem::replace(self, State::Done) {
            State::NotStarted { resources, args } => (resources, args),
            State::Running {
                resources, args, ..
            } => (resources, args),
            State::Done => unreachable!(),
        };
        *self = State::NotStarted { resources, args }
    }

    /// Marks the state as running with `op_id`.
    ///
    /// # Panics
    ///
    /// Panics if `self` is `Done`.
    fn running(&mut self, op_id: OperationId) {
        let (resources, args) = match mem::replace(self, State::Done) {
            State::NotStarted { resources, args } => (resources, args),
            State::Running {
                resources, args, ..
            } => (resources, args),
            State::Done => unreachable!(),
        };
        *self = State::Running {
            resources,
            args,
            op_id,
        }
    }

    /// Marks the state as done, returning the resources.
    ///
    /// # Panics
    ///
    /// Panics if `self` is `Done`.
    fn done(&mut self) -> R {
        match mem::replace(self, State::Done) {
            State::NotStarted { resources, .. } => resources,
            State::Running { resources, .. } => resources,
            State::Done => unreachable!(),
        }
        .into_inner()
    }
}

/// Implementation of an [`Operation`].
pub(crate) trait Op {
    /// Output of the operation.
    type Output;
    /// Resources used in the operation, e.g. a buffer in a read call.
    type Resources;
    /// Arguments in the system call.
    type Args;
    /// [`sq::Submission`].
    type Submission;
    /// [`cq::Event::State`].
    type CompletionState;
    /// Output of the operation specific operation. This can differ from
    /// `Output`, e.g. for a read this will be the amount bytes read, but the
    /// `Output` will be the buffer the bytes are read into.
    type OperationOutput;

    /// Fill a submission for the operation.
    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut Self::Submission,
    );

    /// Check the result of an operation based on the `QueuedOperation.state`
    /// (`Self::CompletionState`).
    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        resources: &mut Self::Resources,
        offset: &mut Self::Args,
        state: &mut Self::CompletionState,
    ) -> OpResult<Self::OperationOutput>;

    /// Map the system call output to the future's output.
    fn map_ok(resources: Self::Resources, operation_output: Self::OperationOutput) -> Self::Output;
}

/// [`Op`] result.
pub(crate) enum OpResult<T> {
    /// [`Result::Ok`].
    Ok(T),
    /// Try the operation again.
    Again,
    /// [`Result::Err`].
    Err(io::Error),
}
