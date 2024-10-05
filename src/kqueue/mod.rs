//! kqueue implementation.

use std::os::fd::OwnedFd;
use std::sync::Mutex;
use std::{fmt, mem};

use crate::sq::QueueFull;

pub(crate) mod config;
mod cq;

pub(crate) use cq::Completions;

pub(crate) struct Shared {
    /// kqueue(2) file descriptor.
    kq: OwnedFd,
    change_list: Mutex<Vec<libc::kevent>>,
}

impl crate::sq::Submissions for Shared {
    type Submission = libc::kevent;

    fn add<F>(&self, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Self::Submission),
    {
        // SAFETY: all zero is valid for `libc::kevent`.
        let mut event = unsafe { mem::zeroed() };
        submit(&mut event);
        self.change_list.lock().unwrap().push(event);
        Ok(())
    }
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO.
        f.write_str("Shared")
    }
}
