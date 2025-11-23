use std::os::fd::AsRawFd;
use std::time::Duration;
use std::{cmp, io, mem, ptr};

use crate::{kqueue, syscall};

#[derive(Debug)]
pub(crate) struct Completions {
    events: Vec<kqueue::Event>,
}

impl Completions {
    pub(crate) fn new(events_capacity: usize) -> Completions {
        let events = Vec::with_capacity(events_capacity);
        Completions { events }
    }
}

impl crate::cq::Completions for Completions {
    type Shared = kqueue::Shared;
    type Event = kqueue::Event;

    fn poll<'a>(
        &'a mut self,
        shared: &Self::Shared,
        timeout: Option<Duration>,
        is_polling: Option<&AtomicBool>,
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

        if let Some(is_polling) = is_polling {
            is_polling.store(true, Ordering::Release);
        }
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
        if let Some(is_polling) = is_polling {
            is_polling.store(false, Ordering::Release);
        }
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
            Err(err)
        } else {
            Ok(self.events.iter())
        }
    }

    fn sq_available(&mut self, shared: &Self::Shared) -> usize {
        // No practical limit.
        usize::MAX
    }
}
