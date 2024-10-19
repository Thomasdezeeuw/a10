//! Module with [`Operation`] [`Future`].

use std::cell::UnsafeCell;
use std::future::Future;
use std::panic::RefUnwindSafe;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io, mem};

use crate::fd::{AsyncFd, Descriptor, File};
#[cfg(not(target_os = "linux"))]
use crate::sq::QueueFull;
use crate::{cq, sq, sys, OperationId};

/// Generic [`Future`] that powers other I/O operation futures.
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

impl<'fd, O: Op, D: Descriptor> Operation<'fd, O, D>
where
    O::Resources: fmt::Debug,
    O::Args: fmt::Debug,
{
    pub(crate) fn fmt_dbg(&self, name: &'static str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(name)
            .field("fd", &self.fd)
            .field("state", &self.state)
            .finish()
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
    O::OperationOutput: fmt::Debug,
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
                log::trace!(queued_op:? = &*queued_op_slot; "mapping operation result");
                let result = match queued_op_slot.as_mut() {
                    // Only map the result if the operation is marked as done.
                    // Otherwise we wait for another event.
                    Some(queued_op) if !queued_op.done => return Poll::Pending,
                    Some(queued_op) => O::check_result(fd, resources.get_mut(), args, &mut queued_op.state),
                    // Somehow the queued operation is gone. This shouldn't
                    // happen, but we'll deal with it anyway.
                    None => OpResult::Again,
                };
                drop(queued_op_slot); // Unlock.
                log::trace!(result:? = result; "mapped operation result");
                match result {
                    OpResult::Ok(ok) => {
                        let resources = state.done();
                        Poll::Ready(Ok(O::map_ok(resources, ok)))
                    }
                    OpResult::Again => {
                        // Operation wasn't completed, need to try again.
                        // TODO: can we do this differently than using a `cfg`?
                        #[cfg(not(target_os = "linux"))]
                        {
                            let result = fd.sq().inner.resubmit(
                                *op_id,
                                |submission| O::fill_submission(fd, resources.get_mut(), args, submission),
                            );
                            match result {
                                Ok(()) => { /* Running again using the same operation id. */ }
                                Err(QueueFull) => state.not_started(),
                            }
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
            State::Done => unreachable!("Future polled after completion"),
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

// SAFETY: `UnsafeCell` is `!Sync`, but as long as `R` is `Sync` so it while
// wrapped in `UnsafeCell`.
unsafe impl<R: Send, A: Send> Send for State<R, A> {}
unsafe impl<R: Sync, A: Sync> Sync for State<R, A> {}

impl<R: RefUnwindSafe, A: RefUnwindSafe> RefUnwindSafe for State<R, A> {}

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
        args: &mut Self::Args,
        state: &mut Self::CompletionState,
    ) -> OpResult<Self::OperationOutput>;

    /// Map the system call output to the future's output.
    fn map_ok(resources: Self::Resources, operation_output: Self::OperationOutput) -> Self::Output;
}

/// [`Op`] result.
#[derive(Debug)]
pub(crate) enum OpResult<T> {
    /// [`Result::Ok`].
    Ok(T),
    /// Try the operation again.
    Again,
    /// [`Result::Err`].
    Err(io::Error),
}

/// Create a [`Future`] based on [`Operation`].
macro_rules! op_future {
    (
        $(#[ $meta: meta ])*
        $vis: vis struct $name: ident <$resources: ident : $trait: ident>($sys: ty) -> $output: ty;
    ) => {
        $(#[ $meta ])*
        $vis struct $name<'fd, $resources: $trait, D: $crate::fd::Descriptor = $crate::fd::File>($crate::op::Operation<'fd, $sys, D>);

        impl<'fd, $resources: $trait + ::std::marker::Unpin, D: $crate::fd::Descriptor + ::std::marker::Unpin> ::std::future::Future for $name<'fd, $resources, D> {
            type Output = $output;

            fn poll(mut self: ::std::pin::Pin<&mut Self>, ctx: &mut ::std::task::Context<'_>) -> ::std::task::Poll<Self::Output> {
                ::std::pin::Pin::new(&mut self.0).poll(ctx)
            }
        }

        impl<'fd, $resources: $trait + ::std::fmt::Debug, D: $crate::fd::Descriptor> ::std::fmt::Debug for $name<'fd, $resources, D> {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt_dbg(::std::stringify!("a10::", $name), f)
            }
        }
    };
}

pub(crate) use op_future;
