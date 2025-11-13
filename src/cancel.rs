//! Cancelation of operations.
//!
//! See the [`Cancel`] trait to cancel a specific operation or
//! [`AsyncFd::cancel_all`] to cancel all operations on a fd.

use std::future::Future;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io};

use crate::op::{FdOperation, Operation, fd_operation};
use crate::{AsyncFd, OperationId, SubmissionQueue, sys};

/// Cancelation of operations, also see the [`Cancel`] trait to cancel specific
/// operations.
impl AsyncFd {
    /// Attempt to cancel all in progress operations on this fd.
    ///
    /// If the I/O operations were succesfully canceled this returns `Ok(n)`,
    /// where `n` is the number of operations canceled. The canceled operations
    /// will return `ECANCELED` to indicate they were canceled.
    ///
    /// If no operations were found, for example if they were already completed,
    /// this will return `Ok(0)`.
    ///
    /// In general, operations that are interruptible (like socket IO) will get
    /// canceled, while disk IO operations cannot be canceled if already
    /// started.
    ///
    /// # Notes
    ///
    /// Due to the lazyness of [`Future`]s it is possible that this will return
    /// `Ok(0)` if operations were never polled only to start it after their
    /// first poll.
    ///
    /// Using kqueue (BSD family, macOS) this always returns `Ok(0)` (or an
    /// error) as we can't determine how many operations where canceled.
    ///
    /// [`Future`]: std::future::Future
    pub const fn cancel_all<'fd>(&'fd self) -> CancelAll<'fd> {
        CancelAll(FdOperation::new(self, (), ()))
    }
}

fd_operation!(
    /// [`Future`] behind [`AsyncFd::cancel_all`].
    pub struct CancelAll(sys::cancel::CancelAllOp) -> io::Result<usize>;
);

/// Cancelation of an in progress operations.
pub trait Cancel {
    /// Attempt to cancel this operation.
    ///
    /// The cancelation attempt will be done asynchronously, without returning
    /// the result. If you want to know the result of the cancelation attempt
    /// use [`cancel`] instead.
    ///
    /// [`cancel`]: Cancel::cancel
    fn try_cancel(&mut self) -> CancelResult;

    /// Cancel this operation.
    ///
    /// Once the returned future is completed it will asynchronously cancel the
    /// related operation. This means that it *may* still return results that
    /// were created before the operation was actually canceled.
    ///
    /// For example using a TCP listener and multishot accept it's possible that
    /// `MultishotAccept` will return more accepted connections after it's canceled.
    /// Simply keep accepting the connections and it will return `None` after all
    /// pending connections have been accepted.
    ///
    /// If this returns `ENOENT` it means the operation was not found. This can
    /// be caused by the operation never starting, due to the inert nature of
    /// [`Future`]s, or the operation has already been completed.
    ///
    /// If this returns `EALREADY` it means the operation was found, but it was
    /// already canceled previously.
    ///
    /// If the operation was found and canceled this returns `Ok(())`.
    ///
    /// If this is called on an [`AsyncIterator`] it will cause them to return
    /// `None` (eventually, it may still return pending items).
    ///
    /// [`Future`]: std::future::Future
    /// [`AsyncIterator`]: std::async_iter::AsyncIterator
    fn cancel(&mut self) -> CancelOperation;
}

/// Result of a cancelation attempt.
#[derive(Copy, Clone, Debug)]
#[allow(clippy::module_name_repetitions)] // Don't care.
pub enum CancelResult {
    /// Operation was cancelled asynchronously.
    Canceled,
    /// Operation was not started.
    NotStarted,
    /// Operation queue is currently full, can't cancel the operation.
    ///
    /// To resolve this call [`Ring::poll`] or use [`Cancel::cancel`] to await
    /// the cancelation.
    ///
    /// [`Ring::poll`]: crate::Ring::poll
    QueueFull,
}

/// [`Future`] behind [`Cancel::cancel`].
#[must_use = "`Future`s do nothing unless polled"]
pub struct CancelOperation(pub(crate) CancelOperationState);

impl CancelOperation {
    /// Create a new `CancelOperation`.
    pub(crate) fn new(sq: SubmissionQueue, op_id: Option<OperationId>) -> CancelOperation {
        if let Some(op_id) = op_id {
            let operation = Operation::new(sq, (), op_id);
            CancelOperation(CancelOperationState::InProgress(operation))
        } else {
            CancelOperation(CancelOperationState::Done)
        }
    }
}

/// State of `CancelOperation`.
pub(crate) enum CancelOperationState {
    /// Cancellation is already done, or the operation was never started.
    Done,
    /// Cancellation is in progress.
    InProgress(Operation<sys::cancel::CancelOperationOp>),
}

impl Future for CancelOperation {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match &mut self.0 {
            CancelOperationState::Done => Poll::Ready(Ok(())),
            CancelOperationState::InProgress(op) => Pin::new(op).poll(ctx),
        }
    }
}

impl fmt::Debug for CancelOperation {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        const NAME: &str = "a10::CancelOperation";
        match &self.0 {
            CancelOperationState::Done => f.debug_struct(NAME).field("state", &"done").finish(),
            CancelOperationState::InProgress(op) => op.fmt_dbg(NAME, f),
        }
    }
}
