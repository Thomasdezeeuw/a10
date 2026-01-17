use std::mem::{drop as unlock, take};
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::{cmp, io, ptr};

use crate::kqueue::{Event, Shared};
use crate::lock;

/// User data to wake up the polling thread.
pub(super) const WAKE_USER_DATA: *mut libc::c_void = ptr::null_mut();

#[derive(Debug)]
pub(crate) struct Completions {
    events: Vec<Event>,
}

impl Completions {
    pub(crate) fn new(events_capacity: u32) -> Completions {
        let events = Vec::with_capacity(events_capacity as usize);
        Completions { events }
    }

    pub(crate) fn poll(&mut self, shared: &Shared, timeout: Option<Duration>) -> io::Result<()> {
        self.events.clear();

        let ts = timeout.map(|to| libc::timespec {
            tv_sec: cmp::min(to.as_secs(), libc::time_t::MAX as u64) as libc::time_t,
            // `Duration::subsec_nanos` is guaranteed to be less than one
            // billion (the number of nanoseconds in a second), making the
            // cast to i32 safe. The cast itself is needed for platforms
            // where C's long is only 32 bits.
            tv_nsec: libc::c_long::from(to.subsec_nanos() as i32),
        });

        // Submit any submissions (changes) to the kernel.
        let mut change_list = lock(&shared.change_list);
        let mut changes = if change_list.is_empty() {
            Vec::new() // No point in taking an empty vector.
        } else {
            take(&mut *change_list)
        };
        unlock(change_list); // Unlock, to not block others.

        log::trace!(submissions = changes.len(), timeout:?; "waiting for events");
        shared.is_polling.store(true, Ordering::Release);
        shared.kevent(&mut changes, Some(&mut self.events), ts.as_ref());
        shared.is_polling.store(false, Ordering::Release);
        shared.reuse_change_list(changes);
        Ok(())
    }
}
