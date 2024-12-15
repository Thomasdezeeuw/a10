//! Cancelation of operations.
//!
//! See the [`Cancel`] trait to cancel a specific operation or
//! [`AsyncFd::cancel_all`] to cancel all operations on a fd.

use std::io;

use crate::fd::{AsyncFd, Descriptor};
use crate::op::{op_future, Operation};
use crate::sys;

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
        CancelAll(Operation::new(self, (), ()))
    }
}

op_future!(
    /// [`Future`] behind [`AsyncFd::cancel_all`].
    pub struct CancelAll(sys::cancel::CancelAll) -> io::Result<usize>;
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

    /* TODO(port): finish below.
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
    /// `None` (eventually, it may still return pending items).
    ///
    /// [`AsyncIterator`]: std::async_iter::AsyncIterator
    fn cancel(&mut self) -> CancelOperation;
    */
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

/* TODO(port): finish this.
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
pub struct CancelOperation<'fd> {
    sq: &'fd SubmissionQueue,
    state: State<(), Option<OperationId>>,
}

impl<'fd> CancelOperation<'fd> {
    /// Create a new `CancelOperation`.
    pub(crate) const fn new(
        sq: &'fd SubmissionQueue,
        op_id: Option<OperationId>,
    ) -> CancelOperation<'fd> {
        let state = State::new((), op_id);
        CancelOperation { sq, state }
    }
}

impl<'fd> Future for CancelOperation<'fd> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `fd` or `state`.
        let CancelOperation { sq, state } = unsafe { self.get_unchecked_mut() };
        match state {
            State::NotStarted {
                resources: _,
                args: Some(to_cancel_op_id),
            } => {
                let result = sq.inner.submit(
                    |submission| {
                        crate::sys::cancel::operation(*to_cancel_op_id, submission);
                    },
                    ctx.waker().clone(),
                );
                if let Ok(op_id) = result {
                    state.running(op_id);
                }
                // We'll be awoken once the operation is done, or if the
                // submission queue is full we'll be awoken once a submission
                // slot is available.
                return Poll::Pending;
            }
            State::NotStarted {
                resources: _,
                args: None,
            } => {
                // If the operation was not started we pretend like we didn't
                // find it to match io_uring behaviour.
                state.done();
                return Poll::Ready(Err(io::Error::from_raw_os_error(libc::ENOENT)));
            }
            State::Running {
                resources: _,
                args,
                op_id,
            } => {
                // TODO.
                todo!()
            }
            State::Done => unreachable!("Future polled after completion"),
        }
    }
}
*/
