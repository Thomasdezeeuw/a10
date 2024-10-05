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
    change_list: Mutex<Vec<Event>>,
}

impl crate::sq::Submissions for Shared {
    type Submission = Event;

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

#[repr(transparent)] // Requirement for `kevent` calls.
pub(crate) struct Event(libc::kevent);

impl crate::cq::Event for Event {
    /// No additional state is needed.
    type State = ();

    fn id(&self) -> usize {
        self.0.udata as usize
    }

    fn update_state(&self, _: &mut Self::State) -> bool {
        false // Using `EV_ONESHOT`, so expecting one event.
    }
}

impl crate::sq::Submission for Event {
    fn set_id(&mut self, id: usize) {
        self.0.udata = id as _;
    }
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO.
        f.write_str("CompletitionEvent")
    }
}

// SAFETY: `libc::kevent` is thread safe.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}
