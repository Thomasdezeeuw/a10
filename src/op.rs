//! Module with [`Operation`] and [`FdOperation`] [`Future`]s.

use std::cell::UnsafeCell;
use std::future::Future;
use std::panic::RefUnwindSafe;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io, mem};

use crate::cancel::{Cancel, CancelOperation, CancelResult};
use crate::fd::{AsyncFd, Descriptor, File};
use crate::sq::QueueFull;
use crate::{cq, sq, sys, OperationId, SubmissionQueue};

/// Generic [`Future`] that powers other I/O operation futures.
pub(crate) struct Operation<O: Op> {
    sq: SubmissionQueue,
    state: State<O::Resources, O::Args>,
}

impl<O: Op> Operation<O> {
    /// Create a new `Operation`.
    pub(crate) const fn new(
        sq: SubmissionQueue,
        resources: O::Resources,
        args: O::Args,
    ) -> Operation<O> {
        Operation {
            sq,
            state: State::new(resources, args),
        }
    }
}

impl<O: Op> Operation<O>
where
    O::Resources: fmt::Debug,
    O::Args: fmt::Debug,
{
    pub(crate) fn fmt_dbg(&self, name: &'static str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(name)
            .field("sq", &self.sq)
            .field("state", &self.state)
            .finish()
    }
}

impl<O> Future for Operation<O>
where
    // TODO: this is silly.
    O: Op<
        Submission = <<sys::Implementation as crate::Implementation>::Submissions as sq::Submissions>::Submission,
        OperationState = <<<sys::Implementation as crate::Implementation>::Completions as cq::Completions>::Event as cq::Event>::State,
    >,
    O::OperationOutput: fmt::Debug,
{
    type Output = io::Result<O::Output>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `fd` or `state`.
        let Operation { sq, state } = unsafe { self.get_unchecked_mut() };
        state.poll(
            ctx,
            sq,
            O::fill_submission,
            O::check_result,
            O::map_ok,
        )
    }
}

/// Only implement `Unpin` if the underlying operation implement `Unpin`.
impl<O: Op + Unpin> Unpin for Operation<O> {}

/// Implementation of a [`Operation`].
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
    type OperationState;
    /// Output of the operation specific operation. This can differ from
    /// `Output`, e.g. for a read this will be the amount bytes read, but the
    /// `Output` will be the buffer the bytes are read into.
    type OperationOutput;

    /// Fill a submission for the operation.
    fn fill_submission(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut Self::Submission,
    );

    /// Check the result of an operation based on the `QueuedOperation.state`
    /// (`Self::OperationState`).
    fn check_result(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        state: &mut Self::OperationState,
    ) -> OpResult<Self::OperationOutput>;

    /// Map the system call output to the future's output.
    fn map_ok(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        operation_output: Self::OperationOutput,
    ) -> Self::Output;
}

/// Generic [`Future`] that powers other I/O operation futures on a file
/// descriptor.
pub(crate) struct FdOperation<'fd, O: FdOp, D: Descriptor = File> {
    fd: &'fd AsyncFd<D>,
    state: State<O::Resources, O::Args>,
}

impl<'fd, O: FdOp, D: Descriptor> FdOperation<'fd, O, D> {
    /// Create a new `FdOperation`.
    pub(crate) const fn new(
        fd: &'fd AsyncFd<D>,
        resources: O::Resources,
        args: O::Args,
    ) -> FdOperation<'fd, O, D> {
        FdOperation {
            fd,
            state: State::new(resources, args),
        }
    }

    pub(crate) const fn fd(&self) -> &'fd AsyncFd<D> {
        self.fd
    }
}

impl<'fd, O: FdOp, D: Descriptor> FdOperation<'fd, O, D>
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

impl<'fd, O, D> Future for FdOperation<'fd, O, D>
where
    // TODO: this is silly.
    O: FdOp<
        Submission = <<sys::Implementation as crate::Implementation>::Submissions as sq::Submissions>::Submission,
        OperationState = <<<sys::Implementation as crate::Implementation>::Completions as cq::Completions>::Event as cq::Event>::State,
    >,
    D: Descriptor,
    O::OperationOutput: fmt::Debug,
{
    type Output = io::Result<O::Output>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `fd` or `state`.
        let FdOperation { fd, state } = unsafe { self.get_unchecked_mut() };
        state.poll(
            ctx,
            fd.sq(),
            |resources, args, submission| {
                O::fill_submission(fd, resources, args, submission);
                D::use_flags(submission);
            },
            |resources, args, state| O::check_result(fd, resources, args, state),
            |_, resources, operation_output| O::map_ok(resources, operation_output),
        )
    }
}

impl<'fd, O: FdOp, D: Descriptor> Cancel for FdOperation<'fd, O, D> {
    fn try_cancel(&mut self) -> CancelResult {
        if let Some(op_id) = self.state.op_id() {
            let result = self.fd.sq.inner.submit_no_completion(|submission| {
                sys::cancel::operation(op_id, submission);
            });
            match result {
                Ok(()) => CancelResult::Canceled,
                Err(QueueFull) => CancelResult::QueueFull,
            }
        } else {
            CancelResult::NotStarted
        }
    }

    fn cancel(&mut self) -> CancelOperation {
        let op_id = self.state.op_id();
        CancelOperation::new(self.fd.sq().clone(), op_id)
    }
}

/// Only implement `Unpin` if the underlying operation implement `Unpin`.
impl<'fd, O: FdOp + Unpin, D: Descriptor> Unpin for FdOperation<'fd, O, D> {}

/// Implementation of a [`FdOperation`].
pub(crate) trait FdOp {
    /// Output of the operation.
    type Output;
    /// Resources used in the operation, e.g. a buffer in a read call.
    type Resources;
    /// Arguments in the system call.
    type Args;
    /// [`sq::Submission`].
    type Submission;
    /// [`cq::Event::State`].
    type OperationState;
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
    /// (`Self::OperationState`).
    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        state: &mut Self::OperationState,
    ) -> OpResult<Self::OperationOutput>;

    /// Map the system call output to the future's output.
    fn map_ok(resources: Self::Resources, operation_output: Self::OperationOutput) -> Self::Output;
}

/// State of an [`Operation`] or [`FdOperation`].
///
/// Generics:
///  * `R` is [`Op::Resources`] or [`FdOp::Resources`].
///  * `A` is [`Op::Args`] or [`FdOp::Args`].
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
    const fn new(resources: R, args: A) -> State<R, A> {
        State::NotStarted {
            resources: UnsafeCell::new(resources),
            args,
        }
    }

    /// Poll the state of this operation.
    ///
    /// NOTE: that the functions match those of the [`FdOp`] and [`Op`] traits.
    fn poll<FillSubmission, CheckResult, OperationOutput, MapOk, Output>(
        &mut self,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
        fill_submission: FillSubmission,
        check_result: CheckResult,
        map_ok: MapOk,
    ) -> Poll<io::Result<Output>>
    where
        FillSubmission: FnOnce(&mut R, &mut A, &mut <<sys::Implementation as crate::Implementation>::Submissions as sq::Submissions>::Submission),
        CheckResult: FnOnce(&mut R, &mut A, &mut <<<sys::Implementation as crate::Implementation>::Completions as cq::Completions>::Event as cq::Event>::State) -> OpResult<OperationOutput>,
        OperationOutput: fmt::Debug,
        MapOk: FnOnce(&SubmissionQueue, R, OperationOutput) -> Output,
    {
        match self {
            State::NotStarted { resources, args } => {
                let result = sq.inner.submit(
                    |submission| {
                        fill_submission(resources.get_mut(), args, submission);
                    },
                    ctx.waker().clone(),
                );
                if let Ok(op_id) = result {
                    self.running(op_id);
                }
                // We'll be awoken once the operation is done, or if the
                // submission queue is full we'll be awoken once a submission
                // slot is available.
                Poll::Pending
            }
            State::Running {
                resources,
                args,
                op_id,
            } => {
                let op_id = *op_id;
                // SAFETY: we've ensured that `op_id` is valid.
                let mut queued_op_slot = unsafe { sq.get_op(op_id) };
                log::trace!(queued_op:? = &*queued_op_slot; "mapping operation result");
                let result = match queued_op_slot.as_mut() {
                    // Only map the result if the operation is marked as done.
                    // Otherwise we wait for another event.
                    Some(queued_op) if !queued_op.done => return Poll::Pending,
                    Some(queued_op) => {
                        check_result(resources.get_mut(), args, &mut queued_op.state)
                    }
                    // Somehow the queued operation is gone. This shouldn't
                    // happen, but we'll deal with it anyway.
                    None => OpResult::Again(true),
                };
                log::trace!(result:? = result; "mapped operation result");
                match result {
                    OpResult::Ok(ok) => {
                        let resources = self.done();
                        // SAFETY: we've ensured that `op_id` is valid.
                        unsafe { sq.make_op_available(op_id, queued_op_slot) };
                        Poll::Ready(Ok(map_ok(sq, resources, ok)))
                    }
                    OpResult::Again(resubmit) => {
                        // Operation wasn't completed, need to try again.
                        drop(queued_op_slot); // Unlock.
                        if resubmit {
                            // SAFETY: we've ensured that we own the `op_id`.
                            // Furthermore we don't use it in case an error is
                            // returned.
                            let result = unsafe {
                                sq.inner.resubmit(op_id, |submission| {
                                    fill_submission(resources.get_mut(), args, submission);
                                })
                            };
                            match result {
                                Ok(()) => { /* Running again using the same operation id. */ }
                                Err(QueueFull) => self.not_started(),
                            }
                        }
                        // We'll be awoken once the operation is ready again or
                        // if we can submit again (in case of QueueFull).
                        Poll::Pending
                    }
                    OpResult::Err(err) => {
                        *self = State::Done;
                        // SAFETY: we've ensured that `op_id` is valid.
                        unsafe { sq.make_op_available(op_id, queued_op_slot) };
                        Poll::Ready(Err(err))
                    }
                }
            }
            State::Done => unreachable!("Future polled after completion"),
        }
    }

    /// Returnt the operation id, if the operation is running.
    const fn op_id(&self) -> Option<OperationId> {
        match self {
            State::Running { op_id, .. } => Some(*op_id),
            _ => None,
        }
    }

    /// Marks the state as not started.
    ///
    /// # Panics
    ///
    /// Panics if `self` is `Done`.
    fn not_started(&mut self) {
        let (resources, args) = match mem::replace(self, State::Done) {
            State::NotStarted { resources, args }
            | State::Running {
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
            State::NotStarted { resources, args }
            | State::Running {
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
            State::NotStarted { resources, .. } | State::Running { resources, .. } => resources,
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

/// [`Op`] and [`FdOp`] result.
#[derive(Debug)]
pub(crate) enum OpResult<T> {
    /// [`Result::Ok`].
    Ok(T),
    /// Try the operation again.
    ///
    /// The boolean indicates whether or not we should resubmit.
    Again(bool),
    /// [`Result::Err`].
    Err(io::Error),
}

/// Create a [`Future`] based on [`Operation`].
macro_rules! operation {
    (
        $(
        $(#[ $meta: meta ])*
        $vis: vis struct $name: ident $( <$resources: ident : $trait: path $(; const $const_generic: ident : $const_ty: ty )?> )? ($sys: ty) -> $output: ty;
        )+
    ) => {
        $(
        $(#[ $meta ])*
        #[must_use = "`Future`s do nothing unless polled"]
        $vis struct $name<$( $resources: $trait $(, const $const_generic: $const_ty )?, )?>($crate::op::Operation<$sys>);

        impl<$( $resources: $trait $(, const $const_generic: $const_ty )?, )?> ::std::future::Future for $name<$( $resources $(, $const_generic )?, )?> {
            type Output = $output;

            fn poll(self: ::std::pin::Pin<&mut Self>, ctx: &mut ::std::task::Context<'_>) -> ::std::task::Poll<Self::Output> {
                // SAFETY: not moving `self.0` (`s.0`), directly called
                // `Future::poll` on it.
                unsafe { ::std::pin::Pin::map_unchecked_mut(self, |s| &mut s.0) }.poll(ctx)
            }
        }

        impl<$( $resources: $trait + ::std::fmt::Debug $(, const $const_generic: $const_ty )?, )?> ::std::fmt::Debug for $name<$( $resources $(, $const_generic )?, )?> {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt_dbg(::std::stringify!("a10::", $name), f)
            }
        }
        )+
    };
}

/// Create a [`Future`] based on [`FdOperation`].
macro_rules! fd_operation {
    (
        $(
        $(#[ $meta: meta ])*
        $vis: vis struct $name: ident $( <$resources: ident : $trait: path $(; const $const_generic: ident : $const_ty: ty )?> )? ($sys: ty) -> $output: ty;
        )+
    ) => {
        $(
        $(#[ $meta ])*
        #[must_use = "`Future`s do nothing unless polled"]
        $vis struct $name<'fd, $( $resources: $trait $(, const $const_generic: $const_ty )?, )? D: $crate::fd::Descriptor = $crate::fd::File>($crate::op::FdOperation<'fd, $sys, D>);

        impl<'fd, $( $resources: $trait $(, const $const_generic: $const_ty )?, )? D: $crate::fd::Descriptor> ::std::future::Future for $name<'fd, $( $resources $(, $const_generic )?, )? D> {
            type Output = $output;

            fn poll(self: ::std::pin::Pin<&mut Self>, ctx: &mut ::std::task::Context<'_>) -> ::std::task::Poll<Self::Output> {
                // SAFETY: not moving `self.0` (`s.0`), directly called
                // `Future::poll` on it.
                unsafe { ::std::pin::Pin::map_unchecked_mut(self, |s| &mut s.0) }.poll(ctx)
            }
        }

        impl<'fd, $( $resources: $trait $(, const $const_generic: $const_ty )?, )? D: $crate::fd::Descriptor> crate::cancel::Cancel for $name<'fd, $( $resources $(, $const_generic )?, )? D> {
            fn try_cancel(&mut self) -> crate::cancel::CancelResult {
                self.0.try_cancel()
            }

            fn cancel(&mut self) -> crate::cancel::CancelOperation {
                self.0.cancel()
            }
        }

        impl<'fd, $( $resources: $trait + ::std::fmt::Debug $(, const $const_generic: $const_ty )?, )? D: $crate::fd::Descriptor> ::std::fmt::Debug for $name<'fd, $( $resources $(, $const_generic )?, )? D> {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt_dbg(::std::stringify!("a10::", $name), f)
            }
        }
        )+
    };
}

pub(crate) use {fd_operation, operation};
