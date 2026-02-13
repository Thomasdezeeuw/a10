use std::io;
use std::mem::{self, drop as unlock, take};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;

use crate::kqueue::{Event, Shared, UseEvents, cq};
use crate::lock;

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
        self.submit(ForceSubmit::Normal, fill_event);
    }

    fn submit<F>(&self, force_submit: ForceSubmit, fill_event: F)
    where
        F: FnOnce(&mut Event),
    {
        let shared = &*self.shared;
        // Create and fill the submission event.
        // SAFETY: all zero is valid for `libc::kevent`.
        let mut event: Event = unsafe { mem::zeroed() };
        event.0.flags |=
            libc::EV_CLEAR | libc::EV_DISPATCH | libc::EV_ENABLE | libc::EV_ADD | libc::EV_RECEIPT;
        fill_event(&mut event);
        log::trace!(event:?; "registering event");

        // Add the event to the list of waiting events.
        let mut change_list = lock(&shared.change_list);
        change_list.push(event);
        // If we haven't collected enough events yet and we're not polling,
        // we're done quickly.
        if let ForceSubmit::Normal = force_submit
            && (change_list.len() < (shared.max_change_list_size as usize))
        {
            unlock(change_list); // Unlock first.
            return;
        }

        // Take ownership of the change list to submit it to the kernel.
        let mut changes = take(&mut *change_list);
        unlock(change_list); // Unlock, to not block others.

        // Submit the all changes to the kernel.
        let ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        log::trace!(changes = changes.len(); "submitting changes");
        let events = match force_submit {
            ForceSubmit::Normal => UseEvents::UseChangeList,
            ForceSubmit::Wakeup => UseEvents::UseChangeListWithWakeUp,
        };
        shared.kevent(&mut changes, events, Some(&ts));
        shared.reuse_change_list(changes);
    }

    /// Remove all unsubmitted events for `fd`.
    #[allow(clippy::cast_sign_loss)]
    pub(crate) fn remove_unsubmitted_events(&self, fd: RawFd) {
        let mut change_list = lock(&self.shared.change_list);
        change_list.retain(|event| {
            !(event.0.ident == fd as _
                && (event.0.filter == libc::EVFILT_READ || event.0.filter == libc::EVFILT_WRITE))
        });
        unlock(change_list);
    }

    #[allow(clippy::unnecessary_wraps)]
    pub(crate) fn wake(&self) -> io::Result<()> {
        if !self.shared.polling.wake() {
            // If we're not polling we don't need to wake up.
            return Ok(());
        }

        self.submit(ForceSubmit::Wakeup, |kevent| {
            kevent.0.filter = libc::EVFILT_USER;
            kevent.0.flags = libc::EV_ADD;
            kevent.0.fflags = libc::NOTE_TRIGGER;
            kevent.0.udata = cq::WAKE_USER_DATA;
        });
        Ok(())
    }

    pub(crate) fn shared(&self) -> &Shared {
        &self.shared
    }

    pub(crate) fn fd(&self) -> RawFd {
        self.shared.kq.as_raw_fd()
    }
}

#[derive(Copy, Clone)]
enum ForceSubmit {
    Normal,
    Wakeup,
}

impl PartialEq for Submissions {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }
}

impl Eq for Submissions {}
