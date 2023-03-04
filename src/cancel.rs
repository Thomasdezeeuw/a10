//! Cancelation of operations.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::op::{poll_state, OpState};
use crate::{libc, AsyncFd, OpIndex, QueueFull, SubmissionQueue};

/// Cancelation of operations.
impl AsyncFd {
    /// Attempt to cancel an in progress operation.
    ///
    /// If the previous I/O operation was succesfully canceled this returns
    /// `Ok(())` and the canceled operation will return `ECANCELED` to indicate
    /// it was canceled.
    ///
    /// If no previous operation was found, for example if it was already
    /// completed, this will return `io::ErrorKind::NotFound`.
    ///
    /// In general, requests that are interruptible (like socket IO) will get
    /// canceled, while disk IO requests cannot be canceled if already started.
    ///
    /// # Notes
    ///
    /// Due to the lazyness of [`Future`]s it's possible that this will return
    /// `NotFound` if the previous operation was never polled.
    pub const fn cancel_previous<'fd>(&'fd self) -> Cancel<'fd> {
        Cancel {
            fd: self,
            state: OpState::NotStarted(0),
        }
    }

    /// Same as [`AsyncFd::cancel_previous`], but attempts to cancel all
    /// operations.
    pub const fn cancel_all<'fd>(&'fd self) -> Cancel<'fd> {
        Cancel {
            fd: self,
            state: OpState::NotStarted(libc::IORING_ASYNC_CANCEL_ALL),
        }
    }
}

/// [`Future`] behind [`AsyncFd::cancel_previous`] and [`AsyncFd::cancel_all`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct Cancel<'fd> {
    fd: &'fd AsyncFd,
    state: OpState<u32>,
}

impl<'fd> Future for Cancel<'fd> {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let op_index = poll_state!(Cancel, *self, ctx, |submission, fd, flags| unsafe {
            submission.cancel(fd.fd, flags);
        });

        match self.fd.sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok(_) => Poll::Ready(Ok(())),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Cancelation of operations.
pub trait CancelOperation {
    /// Attempt to cancel this operation.
    fn try_cancel(&mut self) -> CancelResult;

    /// Cancel this operation.
    fn cancel(&mut self) -> CancelOp;
}

/// Result of a cancelation attempt.
#[derive(Copy, Clone, Debug)]
pub enum CancelResult {
    /// Operation was cancelled.
    Canceled,
    /// Operation was not started.
    NotStarted,
    /// Operation queue is currently full, can't cancel the operation.
    ///
    /// To resolve this call [`Ring::poll`] or use [`CancelOperation::cancel`].
    ///
    /// [`Ring::poll`]: crate::Ring::poll
    QueueFull,
}

/// [`Future`] behind functions such as [`MultishotAccept::cancel`].
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
