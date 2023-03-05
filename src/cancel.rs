//! Cancelation of operations.

use std::future::Future;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::op::op_future;
use crate::{libc, AsyncFd, OpIndex, QueueFull, SubmissionQueue};

/// Cancelation of operations, also see the [`Cancel`] trait.
impl AsyncFd {
    /// Attempt to cancel all in progress operations on this fd.
    ///
    /// If the I/O operations were succesfully canceled this returns `Ok(n)`,
    /// where `n` is the number of operations canceled, and the canceled
    /// operations will return `ECANCELED` to indicate they were canceled.
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
    pub const fn cancel_all<'fd>(&'fd self) -> CancelAll<'fd> {
        CancelAll::new(self, libc::IORING_ASYNC_CANCEL_ALL)
    }
}

// CancelAll.
op_future! {
    fn AsyncFd::cancel_all -> usize,
    struct CancelAll<'fd> {
        // Doesn't need any fields.
    },
    setup_state: flags: u32,
    setup: |submission, fd, (), flags| unsafe {
        submission.cancel(fd.fd, flags);
    },
    map_result: |n| {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok(n as usize)
    },
}

/// Cancelation of in-progress operations.
pub trait Cancel {
    /// Attempt to cancel this operation.
    fn try_cancel(&mut self) -> CancelResult;

    /// Cancel this operation.
    fn cancel(&mut self) -> CancelOp;
}

/// Result of a cancelation attempt.
#[derive(Copy, Clone, Debug)]
#[allow(clippy::module_name_repetitions)] // Don't care.
pub enum CancelResult {
    /// Operation was cancelled.
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
///
/// Once this future is completed it will asynchronously cancel the related
/// operation. This means that it *may* still return results that were created
/// before the operation was actually canceled.
///
/// For example using a TCP listener and multishot accept it's possible that
/// `MultishotAccept` will return more accepted connections after it's canceled.
/// Simply keep accepting the connections and it will return `None` after all
/// pending connections have been accepted.
///
///[`MultishotAccept::cancel`]: crate::net::MultishotAccept::cancel
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)] // Don't care.
pub struct CancelOp<'fd> {
    pub(crate) sq: &'fd SubmissionQueue,
    pub(crate) op_index: Option<OpIndex>,
}

impl<'fd> Future for CancelOp<'fd> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let Some(op_index) = self.op_index else {
            return Poll::Ready(());
        };
        let res = self
            .sq
            .add_no_result(|submission| unsafe { submission.cancel_op(op_index) });
        match res {
            Ok(()) => {
                self.op_index = None;
                Poll::Ready(())
            }
            Err(QueueFull(())) => {
                self.sq.wait_for_submission(ctx.waker().clone());
                Poll::Pending
            }
        }
    }
}
