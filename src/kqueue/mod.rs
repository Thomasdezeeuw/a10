//! kqueue implementation.

use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Mutex;
use std::time::Duration;
use std::{cmp, fmt, io, mem, ptr};

use crate::sq::QueueFull;
use crate::{syscall, WAKE_ID};

pub(crate) mod config;

/// Maximum size of the change list before it's submitted to the kernel, without
/// waiting on a call to poll.
// TODO: make this configurable.
const MAX_CHANGE_LIST_SIZE: usize = 64;

/// `kevent.udata` to indicate a waker.
const WAKE_USER_DATA: usize = usize::MAX;

/// kqueue implemetnation.
pub(crate) enum Implementation {}

impl crate::Implementation for Implementation {
    type Shared = Shared;
    type Submissions = Submissions;
    type Completions = Completions;
}

pub(crate) struct Shared {
    /// kqueue(2) file descriptor.
    kq: OwnedFd,
    change_list: Mutex<Vec<Event>>,
}

impl Shared {
    /// Merge the change list.
    ///
    /// Reusing allocations (if it makes sense).
    fn merge_change_list(&self, mut changes: Vec<Event>) {
        if changes.capacity() == 0 {
            return;
        }

        let mut change_list = self.change_list.lock().unwrap();
        if change_list.len() < changes.capacity() {
            // Existing is smaller than `changes` alloc, reuse it.
            mem::swap(&mut *change_list, &mut changes);
        }
        change_list.append(&mut changes);
        drop(change_list); // Unlock before any deallocations.
    }
}

impl crate::sq::Submissions for Shared {
    type Submission = Event;

    fn add<F>(&self, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Self::Submission),
    {
        // Create and fill the submission event.
        // SAFETY: all zero is valid for `libc::kevent`.
        let mut event = unsafe { mem::zeroed() };
        submit(&mut event);
        event.0.flags = libc::EV_ADD | libc::EV_RECEIPT | libc::EV_ONESHOT;
        // TODO: see if we can replace `EV_ONESHOT` with `EV_DISPATCH`, might be
        // a cheaper operation.

        // Add the event to the list of waiting events.
        let mut change_list = self.change_list.lock().unwrap();
        change_list.push(event);
        // If we haven't collected enough events yet we're done quickly.
        if change_list.len() < MAX_CHANGE_LIST_SIZE {
            drop(change_list); // Unlock first.
            return Ok(());
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
            self.kq.as_raw_fd(),
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            changes.as_ptr().cast(),
            changes.len() as _,
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            changes.as_mut_ptr().cast(),
            changes.capacity() as _,
            &timeout,
        ));
        if let Err(err) = result {
            // According to the manual page of FreeBSD: "When kevent() call
            // fails with EINTR error, all changes in the changelist have been
            // applied", so we can safely ignore it.
            if err.raw_os_error() != Some(libc::EINTR) {
                // NOTE: this should never happen.
                log::warn!("failed to submit change list: {err}, dropping changes: {changes:?}");
            }
        }
        // Check all events for possible errors and log them.
        for event in &changes {
            // NOTE: this can happen if one of the file descriptors was closed
            // before the change was submitted to the kernel. We'll log it, but
            // otherwise ignore it.
            if let Some(err) = event.error() {
                // TODO: see if we can some how get this error to the operation
                // that submitted it.
                log::warn!("submitted change has an error: {err}, event: {event:?}, dropping it");
            }
        }

        // Reuse the change list allocation (if it makes sense).
        changes.clear();
        self.merge_change_list(changes);
        Ok(())
    }

    fn wake(&self) -> io::Result<()> {
        let mut kevent = libc::kevent {
            ident: 0,
            filter: libc::EVFILT_USER,
            flags: libc::EV_ADD | libc::EV_RECEIPT,
            fflags: libc::NOTE_TRIGGER,
            udata: WAKE_ID as _,
            // SAFETY: all zeros is valid for `libc::kevent`.
            ..unsafe { mem::zeroed() }
        };
        let kq = self.kq.as_raw_fd();
        syscall!(kevent(kq, &kevent, 1, &mut kevent, 1, ptr::null()))?;
        if (kevent.flags & libc::EV_ERROR) != 0 && kevent.data != 0 {
            Err(io::Error::from_raw_os_error(kevent.data as i32))
        } else {
            Ok(())
        }
    }
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("kqueue::Shared")
            .field("kq", &self.kq)
            .field("change_list", &self.change_list)
            .finish()
    }
}

#[derive(Debug)]
pub(crate) struct Completions {
    events: Vec<Event>,
}

impl Completions {
    pub(crate) fn new(events_capacity: usize) -> Completions {
        // NOTE: the `events` capacity must be at least MAX_CHANGE_LIST_SIZE to
        // ensure we can handle all submission errors.
        let events = Vec::with_capacity(cmp::max(events_capacity, MAX_CHANGE_LIST_SIZE));
        Completions { events }
    }
}

impl crate::cq::Completions for Completions {
    type Event = Event;
    type Shared = Shared;

    fn poll<'a>(
        &'a mut self,
        shared: &Self::Shared,
        timeout: Option<Duration>,
    ) -> io::Result<impl Iterator<Item = &'a Self::Event>> {
        self.events.clear();

        let timeout = timeout.map(|to| libc::timespec {
            tv_sec: cmp::min(to.as_secs(), libc::time_t::MAX as u64) as libc::time_t,
            // `Duration::subsec_nanos` is guaranteed to be less than one
            // billion (the number of nanoseconds in a second), making the
            // cast to i32 safe. The cast itself is needed for platforms
            // where C's long is only 32 bits.
            tv_nsec: libc::c_long::from(to.subsec_nanos() as i32),
        });

        // Submit any submissions (changes) to the kernel.
        let mut change_list = shared.change_list.lock().unwrap();
        let mut changes = if change_list.is_empty() {
            Vec::new() // No point in taking an empty vector.
        } else {
            mem::replace(&mut *change_list, Vec::new())
        };
        drop(change_list); // Unlock, to not block others.

        let result = syscall!(kevent(
            shared.kq.as_raw_fd(),
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            changes.as_ptr().cast(),
            changes.capacity() as _,
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            self.events.as_mut_ptr().cast(),
            self.events.capacity() as _,
            timeout
                .as_ref()
                .map(|s| s as *const _)
                .unwrap_or(ptr::null_mut()),
        ));
        let mut result_err = None;
        match result {
            // SAFETY: `kevent` ensures that `n` events are written.
            Ok(n) => unsafe { self.events.set_len(n as usize) },
            Err(err) => {
                // According to the manual page of FreeBSD: "When kevent() call
                // fails with EINTR error, all changes in the changelist have been
                // applied", so we can safely ignore it. We'll have zero
                // completions though.
                if err.raw_os_error() != Some(libc::EINTR) {
                    if !changes.is_empty() {
                        log::warn!(
                            "failed to submit change list: {err}, dropping changes: {changes:?}"
                        );
                    }
                    result_err = Some(err);
                }
            }
        }

        changes.clear();
        shared.merge_change_list(changes);

        if let Some(err) = result_err {
            Err(err)
        } else {
            Ok(self.events.iter())
        }
    }
}

/// Wrapper around `libc::kevent` to implementation traits and methods.
#[repr(transparent)] // Requirement for `kevent` calls.
pub(crate) struct Event(libc::kevent);

impl Event {
    /// Returns an error from the event, if any.
    fn error(&self) -> Option<io::Error> {
        // We can't use references to packed structures (in checking the ignored
        // errors), so we need copy the data out before use.
        let data = self.0.data as i64;
        // Check for the error flag, the actual error will be in the `data`
        // field.
        //
        // Older versions of macOS (OS X 10.11 and 10.10 have been witnessed)
        // can return EPIPE when registering a pipe file descriptor where the
        // other end has already disappeared. For example code that creates a
        // pipe, closes a file descriptor, and then registers the other end will
        // see an EPIPE returned from `register`.
        //
        // It also turns out that kevent will still report events on the file
        // descriptor, telling us that it's readable/hup at least after we've
        // done this registration. As a result we just ignore `EPIPE` here
        // instead of propagating it.
        //
        // More info can be found at tokio-rs/mio#582.
        //
        // The ENOENT error informs us that a filter we're trying to remove
        // wasn't there in first place, but we don't really care since our goal
        // is accomplished.
        if (self.0.flags & libc::EV_ERROR != 0)
            && data != 0
            && data != libc::EPIPE as i64
            && data != libc::ENOENT as i64
        {
            Some(io::Error::from_raw_os_error(data as i32))
        } else {
            None
        }
    }
}

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

// SAFETY: `libc::kevent` is thread safe.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO.
        f.write_str("kqueue::Event")
    }
}
