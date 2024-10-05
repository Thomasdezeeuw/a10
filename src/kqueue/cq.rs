use std::os::fd::AsRawFd;
use std::time::Duration;
use std::{cmp, fmt, io, ptr};

use crate::sys::Shared;
use crate::syscall;

#[derive(Debug)]
pub(crate) struct Poll {
    events: Vec<Event>,
}

impl crate::Poll for Poll {
    type CompletionEvent = Event;
    type Shared = Shared;

    fn poll<'a>(
        &'a mut self,
        shared: &Self::Shared,
        timeout: Option<Duration>,
    ) -> io::Result<impl Iterator<Item = &'a Self::CompletionEvent>> {
        let timeout = timeout.map(|to| libc::timespec {
            tv_sec: cmp::min(to.as_secs(), libc::time_t::MAX as u64) as libc::time_t,
            // `Duration::subsec_nanos` is guaranteed to be less than one
            // billion (the number of nanoseconds in a second), making the
            // cast to i32 safe. The cast itself is needed for platforms
            // where C's long is only 32 bits.
            tv_nsec: libc::c_long::from(to.subsec_nanos() as i32),
        });
        let timeout = timeout
            .as_ref()
            .map(|s| s as *const _)
            .unwrap_or(ptr::null_mut());

        self.events.clear();
        let n = syscall!(kevent(
            shared.kq.as_raw_fd(),
            ptr::null(),
            0,
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            self.events.as_mut_ptr().cast(),
            self.events.capacity() as _,
            timeout,
        ))?;
        // This is safe because `kevent` ensures that `n` events are written.
        unsafe { self.events.set_len(n as usize) };

        Ok(self.events.iter())
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

    fn update_state(&self, state: &mut Self::State) -> bool {
        false // Using `EV_ONESHOT`, so expecting one event.
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
