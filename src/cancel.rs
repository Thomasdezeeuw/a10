//! Cancelation of operations.
//!
//! See the [`Cancel`] trait to cancel a specific operation or
//! [`AsyncFd::cancel_all`] to cancel all operations on a fd.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::fd::{AsyncFd, Descriptor};
use crate::op::{op_future, poll_state, OpState};
use crate::{libc, OpIndex, QueueFull, SubmissionQueue};

/// Cancelation of operations, also see the [`Cancel`] trait to cancel specific
/// operations.
impl<D: Descriptor> AsyncFd<D> {
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
    pub const fn cancel_all<'fd>(&'fd self) -> CancelAll<'fd, D> {
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
        submission.cancel(fd.fd(), flags | D::cancel_flag());
    },
    map_result: |n| {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok(n as usize)
    },
}

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
    /// `None` (eventuaully, it may still return pending items).
    ///
    /// [`AsyncIterator`]: std::async_iter::AsyncIterator
    fn cancel(&mut self) -> CancelOp;
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
    sq: &'fd SubmissionQueue,
    state: OpState<Option<OpIndex>>,
}

impl<'fd> CancelOp<'fd> {
    /// Create a new `CancelOp`.
    pub(crate) const fn new(sq: &'fd SubmissionQueue, op_index: Option<OpIndex>) -> CancelOp<'fd> {
        CancelOp {
            sq,
            state: OpState::NotStarted(op_index),
        }
    }
}

impl<'fd> Future for CancelOp<'fd> {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let op_index = match self.state {
            OpState::Running(op_index) => op_index,
            OpState::NotStarted(Some(to_cancel_op_index)) => {
                // SAFETY: this will not panic as the resources are only removed
                // after the state is set to `Done`.
                let result = self
                    .sq
                    .add(|submission| unsafe { submission.cancel_op(to_cancel_op_index) });
                match result {
                    Ok(op_index) => {
                        self.state = OpState::Running(op_index);
                        op_index
                    }
                    Err(QueueFull(())) => {
                        self.sq.wait_for_submission(ctx.waker().clone());
                        return Poll::Pending;
                    }
                }
            }
            // If the operation is not started we pretend like we didn't find
            // it.
            OpState::NotStarted(None) => {
                return Poll::Ready(Err(io::Error::from_raw_os_error(libc::ENOENT)))
            }
            OpState::Done => poll_state!(__panic CancelOp),
        };

        match self.sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok((_, _)) => Poll::Ready(Ok(())),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
