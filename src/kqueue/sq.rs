use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{io, mem, ptr};

use crate::kqueue::{self, Event, Shared, cq};
use crate::{lock, syscall};

#[derive(Clone, Debug)]
pub(crate) struct Submissions {
    shared: Arc<Shared>,
}

impl Submissions {
    pub(crate) fn new(shared: Shared) -> Submissions {
        Submissions {
            shared: Arc::new(shared),
        }
    }

    /// Register a new event.
    pub(super) fn add<F>(&self, fill_event: F)
    where
        F: FnOnce(&mut Event),
    {
        self._add(false, fill_event)
    }

    fn _add<F>(&self, force_kevent: bool, fill_event: F)
    where
        F: FnOnce(&mut Event),
    {
        let shared = &*self.shared;
        // Create and fill the submission event.
        // SAFETY: all zero is valid for `libc::kevent`.
        let mut event: Event = unsafe { mem::zeroed() };
        event.0.flags |= libc::EV_RECEIPT | libc::EV_DISPATCH | libc::EV_ENABLE | libc::EV_ADD;
        fill_event(&mut event);
        log::trace!(event:? = event; "registering event");

        // Add the event to the list of waiting events.
        let mut change_list = lock(&shared.change_list);
        change_list.push(event);
        // If we haven't collected enough events yet and we're not polling,
        // we're done quickly.
        if force_kevent
            || (change_list.len() < (shared.max_change_list_size as usize)
                && !shared.is_polling.load(Ordering::Acquire))
        {
            drop(change_list); // Unlock first.
            return;
        }

        // Take ownership of the change list to submit it to the kernel.
        let mut changes = mem::replace(&mut *change_list, Vec::new());
        drop(change_list); // Unlock, to not block others.

        // Submit the all changes to the kernel.
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let result = syscall!(kevent(
            shared.kq.as_raw_fd(),
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            changes.as_ptr().cast(),
            changes.len() as _,
            // SAFETY: Same cast as above.
            changes.as_mut_ptr().cast(),
            changes.capacity() as _,
            &timeout,
        ));
        if let Err(err) = result {
            // According to the manual page of FreeBSD: "When kevent() call
            // fails with EINTR error, all changes in the changelist have been
            // applied", so we can safely ignore it.
            if err.raw_os_error() != Some(libc::EINTR) {
                // TODO: do we want to put in fake error events or something to
                // ensure the Futures don't stall?
                log::warn!(change_list:? = changes; "failed to submit change list: {err}, dropping changes");
            }
        }
        // Check all events for possible errors and log them.
        for event in &changes {
            // NOTE: this can happen if one of the file descriptors was closed
            // before the change was submitted to the kernel. We'll log it, but
            // otherwise ignore it.
            if let Some(err) = event.error() {
                // TODO: see if we can some how get this error to the operation
                // that submitted it or something to ensure the Future doesn't
                // stall.
                log::warn!(kevent:? = event; "submitted change has an error: {err}, dropping it");
            }
        }

        // Reuse the change list allocation (if it makes sense).
        changes.clear();
        shared.merge_change_list(changes);
    }

    pub(crate) fn wake(&self) -> io::Result<()> {
        if !self.shared.is_polling.load(Ordering::Acquire) {
            // If we're not polling we don't need to wake up.
            return Ok(());
        }

        self._add(true, |kevent| {
            kevent.0.filter = libc::EVFILT_USER;
            kevent.0.flags = libc::EV_ADD;
            kevent.0.fflags = libc::NOTE_TRIGGER;
            kevent.0.udata = cq::WAKE_USER_DATA as _;
        });
        Ok(())
    }

    pub(crate) fn shared(&self) -> &Shared {
        &self.shared
    }
}
