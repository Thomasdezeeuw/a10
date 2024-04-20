//! User space messages.
//!
//! To setup a [`MsgListener`] use [`msg_listener`]. It returns the listener as
//! well as a [`MsgToken`], which can be used in
//! [`SubmissionQueue::try_send_msg`] and [`SubmissionQueue::send_msg`] to send
//! a message to the created `MsgListener`.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::{OpIndex, SubmissionQueue};

/// Token used to the messages.
///
/// See [`SubmissionQueue::msg_listener`].
#[derive(Copy, Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct MsgToken(pub(crate) OpIndex);

/// Setup a listener for user space messages.
///
/// The returned [`MsgListener`] will return all messages send using
/// [`SubmissionQueue::try_send_msg`] and [`SubmissionQueue::send_msg`] using
/// the returned `MsgToken`.
///
/// # Notes
///
/// This will return an error if too many operations are already queued,
/// this is usually resolved by calling [`Ring::poll`].
///
/// The returned `MsgToken` has an implicitly lifetime linked to
/// `MsgListener`. If `MsgListener` is dropped the `MsgToken` will
/// become invalid.
///
/// Due to the limitations mentioned above it's advised to consider the
/// usefulness of the type severly limited. The returned `MsgListener`
/// iterator should live for the entire lifetime of the `Ring`, to ensure we
/// don't use `MsgToken` after it became invalid. Furthermore to ensure
/// the creation of it succeeds it should be done early in the lifetime of
/// `Ring`.
///
/// [`Ring::poll`]: crate::Ring::poll
#[allow(clippy::module_name_repetitions)]
pub fn msg_listener(sq: SubmissionQueue) -> io::Result<(MsgListener, MsgToken)> {
    let op_index = sq.queue_multishot()?;
    Ok((MsgListener { sq, op_index }, MsgToken(op_index)))
}

/// [`AsyncIterator`] behind [`SubmissionQueue::msg_listener`].
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)]
pub struct MsgListener {
    sq: SubmissionQueue,
    op_index: OpIndex,
}

impl MsgListener {
    /// This is the same as the `AsyncIterator::poll_next` function, but then
    /// available on stable Rust.
    pub fn poll_next(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Option<u32>> {
        log::trace!(op_index = self.op_index.0; "polling multishot messages");
        if let Some(operation) = self.sq.shared.queued_ops.get(self.op_index.0) {
            let mut operation = operation.lock().unwrap();
            if let Some(op) = &mut *operation {
                return match op.poll_msg(ctx) {
                    Poll::Ready(data) => Poll::Ready(Some(data.1)),
                    Poll::Pending => Poll::Pending,
                };
            }
        }
        panic!("a10::MsgListener called incorrectly");
    }
}

#[cfg(feature = "nightly")]
impl std::async_iter::AsyncIterator for MsgListener {
    type Item = u32;

    fn poll_next(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Option<Self::Item>> {
        self.poll_next(ctx)
    }
}

/// [`Future`] behind [`SubmissionQueue::send_msg`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)]
pub struct SendMsg<'a> {
    sq: &'a SubmissionQueue,
    token: MsgToken,
    data: u32,
}

impl<'a> SendMsg<'a> {
    /// Create a new `SendMsg`.
    pub(crate) const fn new(sq: &'a SubmissionQueue, token: MsgToken, data: u32) -> SendMsg {
        SendMsg { sq, token, data }
    }
}

impl<'a> Future for SendMsg<'a> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.sq.try_send_msg(self.token, self.data) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(_) => {
                self.sq.wait_for_submission(ctx.waker().clone());
                Poll::Pending
            }
        }
    }
}
