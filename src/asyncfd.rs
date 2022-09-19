//! Module with [`AsyncFd`].

use std::os::unix::io::RawFd;

use crate::op::SharedOperationState;

/// An open file descriptor.
///
/// All functions on `AsyncFd` are asynchronous and return a [`Future`].
///
/// [`Future`]: std::future::Future
#[derive(Debug)]
pub struct AsyncFd {
    pub(crate) fd: RawFd,
    pub(crate) state: SharedOperationState,
}

impl Drop for AsyncFd {
    fn drop(&mut self) {
        let result = self
            .state
            .start(|submission| unsafe { submission.close_fd(self.fd) });
        if let Err(err) = result {
            log::error!("error closing fd: {}", err);
        }
    }
}
