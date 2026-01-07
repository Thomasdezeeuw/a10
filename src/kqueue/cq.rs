use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::{cmp, io, mem, ptr};

use crate::kqueue::{self, fd, Event, Shared};
use crate::syscall;

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
        let mut change_list = shared.change_list.lock().unwrap();
        let mut changes = if change_list.is_empty() {
            Vec::new() // No point in taking an empty vector.
        } else {
            mem::replace(&mut *change_list, Vec::new())
        };
        drop(change_list); // Unlock, to not block others.

        log::trace!(submissions = changes.len(), timeout:? = timeout; "waiting for events");
        shared.is_polling.store(true, Ordering::Release);
        let result = syscall!(kevent(
            shared.kq.as_raw_fd(),
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            changes.as_ptr().cast(),
            changes.len() as _,
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            self.events.as_mut_ptr().cast(),
            self.events.capacity() as _,
            ts.as_ref()
                .map(|s| s as *const _)
                .unwrap_or(ptr::null_mut()),
        ));
        shared.is_polling.store(false, Ordering::Release);
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
                        // TODO: do we want to put in fake error events or
                        // something to ensure the Futures don't stall?
                        log::warn!(change_list:? = changes; "failed to submit change list: {err}, dropping changes");
                    }
                    result_err = Some(err);
                }
            }
        }

        changes.clear();
        shared.merge_change_list(changes);

        if let Some(err) = result_err {
            return Err(err);
        }

        for event in &mut self.events {
            if let Some(err) = event.error() {
                // TODO: see if we can some how get this error to the operation
                // that submitted it or something to ensure the Future doesn't
                // stall.
                log::warn!(kevent:? = event; "submitted change has an error: {err}, dropping it");
                continue;
            }

            log::trace!(event:? = event; "got event");
            match event.0.filter {
                libc::EVFILT_USER if event.0.udata == WAKE_USER_DATA => continue,
                libc::EVFILT_USER => {
                    let ptr = event.0.udata.cast::<kqueue::fd::SharedState>();
                    debug_assert!(!ptr.is_null());
                    // SAFETY: see kqueue::fd::State::drop.
                    unsafe { ptr::drop_in_place(ptr) };
                }
                libc::EVFILT_READ | libc::EVFILT_WRITE => {
                    let ptr = event.0.udata.cast::<kqueue::fd::SharedState>();
                    debug_assert!(!ptr.is_null());
                    // SAFETY: in kqueue::op we ensure that the pointer is
                    // always valid (the kernel should copy it over for us).
                    fd::lock_state(unsafe { &*ptr }).wake(&event);
                }
                _ => log::debug!(event:? = event; "unexpected event, ignoring it"),
            }
        }
        Ok(())
    }
}
