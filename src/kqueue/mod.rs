//! kqueue implementation.

use std::os::fd::OwnedFd;

// TODO: experiment with keeping a change list based on the operations started
// and submit them in a single system call.

pub(crate) mod config;
mod cq;

pub(crate) use cq::Poll;

pub(crate) struct Shared {
    /// kqueue(2) file descriptor.
    kq: OwnedFd,
}
