//! User defined messages.
//!
//! To setup a [`MsgListener`] use [`msg_listener`]. It returns the listener as
//! well as a [`MsgSender`], which can be used to send a message to the created
//! `MsgListener`.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::{sys, OperationId, SubmissionQueue};

/// Setup a listener for user defined messages.
///
/// The returned listener will return all messages send to it using the
/// returned sender.
///
/// # Notes
///
/// This will return an error if too many operations are already queued, this is
/// usually resolved by calling [`Ring::poll`].
///
/// [`Ring::poll`]: crate::Ring::poll
#[allow(clippy::module_name_repetitions)]
pub fn msg_listener(sq: SubmissionQueue) -> io::Result<(MsgListener, MsgSender)> {
    let op_id = sq.inner.queue_multishot()?;
    let listener = MsgListener {
        sq: sq.clone(),
        op_id,
    };
    let sender = MsgSender { sq, op_id };
    Ok((listener, sender))
}

/// Listener for user defined messages.
///
/// This is implemented as an [`AsyncIterator`] that iterates over the message
/// it receives.
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
                    queued_op.update_waker(ctx.waker());
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

/// Send message to the connected [`MsgListener`].
///
/// When messages are send using this type it will use the io_uring submission
/// queue to share the message with the receiving end. This means that it will
/// wake up the thread if it's currently [polling]. *If this wake-up is not
/// needed it's better to use a user-space queue such as the one found the
/// [standard library]*.
///
/// [polling]: crate::Ring::poll
/// [standard library]: std::sync::mpsc
#[derive(Debug)]
pub struct MsgSender {
    sq: SubmissionQueue,
    op_id: OperationId,
}

impl MsgSender {
    /// Try to send a message to the connected listener.
    ///
    /// This will fail if the submission queue is currently full. See [`send`]
    /// for a version that tries again when the submission queue is full.
    ///
    /// [`send`]: MsgSender::send
    pub fn try_send(&self, data: MsgData) -> io::Result<()> {
        self.sq.inner.submit_no_completion(|submission| {
            sys::msg::send(&self.sq, self.op_id, data, submission);
        })?;
        Ok(())
    }

    /// Send a message to the connected listener.
    pub fn send<'s>(&'s self, data: MsgData) -> SendMsg<'s> {
        SendMsg { sender: self, data }
    }
}

/// [`Future`] behind [`MsgSender::send`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)]
pub struct SendMsg<'s> {
    sender: &'s MsgSender,
    data: MsgData,
}

impl<'s> Future for SendMsg<'s> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match self.sender.try_send(self.data) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(_) => {
                self.sender
                    .sq
                    .inner
                    .wait_for_submission(ctx.waker().clone());
                Poll::Pending
            }
        }
    }
}

/// Type of data this module can send.
pub type MsgData = u32;
