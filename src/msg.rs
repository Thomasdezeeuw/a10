//! User space messages.
//!
//! To setup a [`MsgListener`] use [`msg_listener`]. It returns the listener as
//! well as a [`MsgToken`], which can be used in [`try_send_msg`] and
//! [`send_msg`] to send a message to the created `MsgListener`.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::{sys, OperationId, SubmissionQueue};

/// Setup a listener for user space messages.
///
/// The returned [`MsgListener`] will return all messages send using
/// [`try_send_msg`] and [`send_msg`] using the returned `MsgToken`.
///
/// # Notes
///
/// This will return an error if too many operations are already queued, this is
/// usually resolved by calling [`Ring::poll`].
///
/// The returned `MsgToken` has an implicit lifetime linked to `MsgListener`. If
/// `MsgListener` is dropped the `MsgToken` will become invalid.
///
/// Due to the limitations mentioned above it's advised to consider the
/// usefulness of the type severly limited. The returned `MsgListener` iterator
/// should live for the entire lifetime of the `Ring`, to ensure we don't use
/// `MsgToken` after it became invalid. Furthermore to ensure the creation of it
/// succeeds it should be done early in the lifetime of `Ring`.
///
/// [`Ring::poll`]: crate::Ring::poll
#[allow(clippy::module_name_repetitions)]
pub fn msg_listener(sq: SubmissionQueue) -> io::Result<(MsgListener, MsgToken)> {
    let op_id = sq.inner.queue_multishot()?;
    Ok((MsgListener { sq, op_id }, MsgToken(op_id)))
}

/// [`AsyncIterator`] behind [`msg_listener`].
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)]
pub struct MsgListener {
    sq: SubmissionQueue,
    op_id: OperationId,
}

impl MsgListener {
    /// This is the same as the [`AsyncIterator::poll_next`] function, but then
    /// available on stable Rust.
    ///
    /// [`AsyncIterator::poll_next`]: std::async_iter::AsyncIterator::poll_next
    pub fn poll_next(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Option<MsgData>> {
        let op_id = self.op_id;
        // SAFETY: we've ensured that `op_id` is valid.
        let mut queued_op_slot = unsafe { self.sq.get_op(op_id) };
        match queued_op_slot.as_mut() {
            Some(queued_op) => match sys::msg::next(&mut queued_op.state) {
                Some(data) => Poll::Ready(Some(data)),
                None => {
                    if !queued_op.waker.will_wake(ctx.waker()) {
                        queued_op.waker.clone_from(ctx.waker());
                    }
                    Poll::Pending
                }
            },
            // Somehow the queued operation is gone. This shouldn't happen.
            None => Poll::Ready(None),
        }
    }
}

#[cfg(feature = "nightly")]
impl std::async_iter::AsyncIterator for MsgListener {
    type Item = MsgData;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll_next(ctx)
    }
}

/// Try to send a message to [`MsgListener`] using [`MsgToken`].
///
/// This will use the io_uring submission queue to share `data` with the
/// receiving end. This means that it will wake up the thread if it's currently
/// [polling].
///
/// This will fail if the submission queue is currently full. See [`send_msg`]
/// for a version that tries again when the submission queue is full.
///
/// See [`msg_listener`] for examples.
///
/// [polling]: crate::Ring::poll
#[allow(clippy::module_name_repetitions)]
pub fn try_send_msg(sq: &SubmissionQueue, token: MsgToken, data: MsgData) -> io::Result<()> {
    sq.inner
        .submit_no_completion(|submission| sys::msg::send(sq, token.0, data, submission))?;
    Ok(())
}

/// Send a message to iterator listening for message using [`MsgToken`].
#[allow(clippy::module_name_repetitions)]
pub const fn send_msg(sq: SubmissionQueue, token: MsgToken, data: MsgData) -> SendMsg {
    SendMsg { sq, token, data }
}

/// [`Future`] behind [`send_msg`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)]
pub struct SendMsg {
    sq: SubmissionQueue,
    token: MsgToken,
    data: MsgData,
}

impl Future for SendMsg {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match try_send_msg(&self.sq, self.token, self.data) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(_) => {
                self.sq.inner.wait_for_submission(ctx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// Type of data this module can send.
pub type MsgData = u32;

/// Token used to the messages.
///
/// See [`msg_listener`].
#[derive(Copy, Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct MsgToken(pub(crate) OperationId);
