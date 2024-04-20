//! User space messages.
//!
//! To setup a [`MsgListener`] use [`msg_listener`]. It returns the listener as
//! well as a [`MsgToken`], which can be used in [`try_send_msg`] and
//! [`send_msg`] to send a message to the created `MsgListener`.

use std::future::Future;
use std::io;
use std::os::fd::AsRawFd;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::{OpIndex, SubmissionQueue};

/// Token used to the messages.
///
/// See [`msg_listener`].
#[derive(Copy, Clone, Debug)]
#[allow(clippy::module_name_repetitions)]
pub struct MsgToken(pub(crate) OpIndex);

/// Setup a listener for user space messages.
///
/// The returned [`MsgListener`] will return all messages send using
/// [`try_send_msg`] and [`send_msg`] using the returned `MsgToken`.
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

/// [`AsyncIterator`] behind [`msg_listener`].
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

/// Try to send a message to iterator listening for message using [`MsgToken`].
///
/// This will use the io_uring submission queue to share `data` with the
/// receiving end. This means that it will wake up the thread if it's
/// currently [polling].
///
/// This will fail if the submission queue is currently full. See [`send_msg`]
/// for a version that tries again when the submission queue is full.
///
/// See [`msg_listener`] for examples.
///
/// [polling]: crate::Ring::poll
#[allow(clippy::module_name_repetitions)]
pub fn try_send_msg(sq: &SubmissionQueue, token: MsgToken, data: u32) -> io::Result<()> {
    sq.add_no_result(|submission| unsafe {
        submission.msg(sq.shared.ring_fd.as_raw_fd(), (token.0).0 as u64, data, 0);
        submission.no_completion_event();
    })?;
    Ok(())
}

/// Send a message to iterator listening for message using [`MsgToken`].
#[allow(clippy::module_name_repetitions)]
pub const fn send_msg<'sq>(sq: &'sq SubmissionQueue, token: MsgToken, data: u32) -> SendMsg<'sq> {
    SendMsg { sq, token, data }
}

/// [`Future`] behind [`send_msg`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
#[allow(clippy::module_name_repetitions)]
pub struct SendMsg<'sq> {
    sq: &'sq SubmissionQueue,
    token: MsgToken,
    data: u32,
}

impl<'sq> Future for SendMsg<'sq> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match try_send_msg(self.sq, self.token, self.data) {
            Ok(()) => Poll::Ready(Ok(())),
            Err(_) => {
                self.sq.wait_for_submission(ctx.waker().clone());
                Poll::Pending
            }
        }
    }
}
