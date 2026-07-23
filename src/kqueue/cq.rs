use std::io;
use std::mem::{drop as unlock, take};
use std::time::Duration;

use crate::kqueue::{Event, Shared, UseEvents};
use crate::lock;

#[derive(Debug)]
pub(crate) struct Completions {
    events: Vec<Event>,
}

impl Completions {
    pub(crate) fn new(events_capacity: u32) -> Completions {
        let events = Vec::with_capacity(events_capacity as usize);
        Completions { events }
    }

    #[allow(clippy::unnecessary_wraps)]
    pub(crate) fn poll(&mut self, shared: &Shared, timeout: Option<Duration>) -> io::Result<()> {
        self.events.clear();

        let timeout = if shared.polling.set_polling(true) {
            // Got woken up, so polling without a timeout.
            Some(Duration::ZERO)
        } else {
            timeout
        };

        // Submit any submissions (changes) to the kernel.
        let mut change_list = lock(&shared.change_list);
        let mut changes = if change_list.is_empty() {
            Vec::new() // No point in taking an empty vector.
        } else {
            take(&mut *change_list)
        };
        unlock(change_list); // Unlock, to not block others.

        log::trace!(submissions = changes.len(), timeout:?; "waiting for events");
        shared.kevent(&mut changes, UseEvents::Some(&mut self.events), timeout);
        shared.polling.set_polling(false);
        shared.reuse_change_list(changes);
        Ok(())
    }

    pub(crate) fn drop(&mut self, shared: &Shared) {
        // Poll one last time to finish of any asynchronous operations such as
        // canceling multishot operations, allowing for resources to be cleaned
        // up.
        if let Err(err) = self.poll(shared, Some(Duration::ZERO)) {
            log::warn!("error processing last completions: {err}");
        }
    }
}
