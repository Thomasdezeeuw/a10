//! kqueue configuration.

use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;
use std::{io, mem, ptr};

use crate::{sys, syscall, Ring, WAKE_ID};

#[derive(Debug, Clone)]
#[must_use = "no ring is created until `a10::Config::build` is called"]
#[allow(missing_docs)] // NOTE: documented at the root.
pub struct Config<'r> {
    events_capacity: usize,
    max_change_list_size: usize,
    _unused: PhantomData<&'r ()>,
}

impl<'r> Config<'r> {
    pub(crate) const fn new(events_capacity: u32) -> Config<'r> {
        let events_capacity = events_capacity as usize;
        Config {
            events_capacity,
            #[rustfmt::skip]
            max_change_list_size: if events_capacity > 64 { 64 } else { events_capacity },
            _unused: PhantomData,
        }
    }

    /// Maximum size of the change list before it's submitted to the kernel,
    /// without waiting on a call to [`Ring::poll`].
    ///
    /// Defaults to 64 events.
    ///
    /// [`Ring::poll`]: crate::Ring::poll
    ///
    /// # Notes
    ///
    /// If the `entries` (passing to [`Ring::new`]) is smaller than `max` it
    /// will be set to `max`.
    ///
    /// [`Ring::new`]: crate::Ring::new
    pub fn max_change_list_size(mut self, max: usize) -> Self {
        self.max_change_list_size = max;
        if self.events_capacity < max {
            self.events_capacity = max;
        }
        self
    }

    /// Build a new [`Ring`].
    #[doc(alias = "kqueue")]
    pub fn build(self) -> io::Result<Ring> {
        // SAFETY: `kqueue(2)` ensures the fd is valid.
        let kq = unsafe { OwnedFd::from_raw_fd(syscall!(kqueue())?) };
        let kq_fd = kq.as_raw_fd();
        syscall!(fcntl(kq_fd, libc::F_SETFD, libc::FD_CLOEXEC))?;

        // Set up waking.
        let mut kevent = libc::kevent {
            ident: 0,
            filter: libc::EVFILT_USER,
            flags: libc::EV_ADD | libc::EV_CLEAR | libc::EV_RECEIPT,
            udata: WAKE_ID as _,
            // SAFETY: all zeros is valid for `libc::kevent`.
            ..unsafe { mem::zeroed() }
        };
        syscall!(kevent(kq_fd, &kevent, 1, &mut kevent, 1, ptr::null()))?;
        if (kevent.flags & libc::EV_ERROR) != 0 && kevent.data != 0 {
            return Err(io::Error::from_raw_os_error(kevent.data as i32));
        }

        let submissions = sys::Submissions::new(self.max_change_list_size);
        let change_list = Mutex::new(Vec::new());
        let shared = sys::Shared { kq, change_list };
        // NOTE: `events_capacity` must be at least `max_change_list_size` to
        // ensure we can handle all submission errors.
        debug_assert!(self.events_capacity >= self.max_change_list_size);
        let completions = sys::Completions::new(self.events_capacity);
        Ring::build(
            submissions,
            shared,
            completions,
            self.events_capacity, // TODO: add option for # queued operations.
        )
    }
}
