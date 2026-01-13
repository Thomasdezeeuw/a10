//! kqueue configuration.

use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::{io, mem, ptr};

use crate::kqueue::{self, Completions, Shared, Submissions, cq};
use crate::syscall;

#[derive(Debug, Clone)]
pub(crate) struct Config<'r> {
    max_events: u32,
    max_change_list_size: u32,
    _unused: PhantomData<&'r ()>,
}

impl<'r> Config<'r> {
    pub(crate) const fn new() -> Config<'r> {
        Config {
            max_events: 64,
            max_change_list_size: 32,
            _unused: PhantomData,
        }
    }
}

/// kqueue specific configuration.
impl<'r> crate::Config<'r> {
    /// Set the maximum number of events that can be collected in a single call
    /// to `kevent(2)`.
    ///
    /// Defaults to 64.
    pub const fn max_events(mut self, max_events: u32) -> Self {
        self.sys.max_events = max_events;
        self
    }

    /// Set the maximum number of changes that are buffered before registering
    /// them with the kernel, without waiting on a call to [`Ring::poll`].
    ///
    /// This must be larger or equal to [`max_events`]. Defaults to 32.
    ///
    /// [`Ring::poll`]: crate::Ring::poll
    /// [`max_events`]: crate::Config::max_events
    pub const fn max_change_list_size(mut self, max: u32) -> Self {
        self.sys.max_change_list_size = max;
        self
    }

    pub(crate) fn build_sys(self) -> io::Result<(Completions, Submissions)> {
        // NOTE: `max_events` must be at least `max_change_list_size` to ensure
        // we can handle all submission errors.
        debug_assert!(
            self.sys.max_events >= self.sys.max_change_list_size,
            "a10::kqueue::Config: max_change_list_size larger than max_events"
        );

        // SAFETY: `kqueue(2)` ensures the fd is valid.
        let kq = unsafe { OwnedFd::from_raw_fd(syscall!(kqueue())?) };
        let kq_fd = kq.as_raw_fd();
        syscall!(fcntl(kq_fd, libc::F_SETFD, libc::FD_CLOEXEC))?;

        // Set up waking.
        let mut kevent = libc::kevent {
            ident: 0,
            filter: libc::EVFILT_USER,
            flags: libc::EV_ADD | libc::EV_CLEAR | libc::EV_RECEIPT,
            udata: cq::WAKE_USER_DATA,
            // SAFETY: all zeros is valid for `libc::kevent`.
            ..unsafe { mem::zeroed() }
        };
        syscall!(kevent(kq_fd, &kevent, 1, &mut kevent, 1, ptr::null()))?;
        if (kevent.flags & libc::EV_ERROR) != 0 && kevent.data != 0 {
            return Err(io::Error::from_raw_os_error(kevent.data as i32));
        }

        let shared = Shared {
            max_change_list_size: self.sys.max_change_list_size,
            change_list: Mutex::new(Vec::new()),
            is_polling: AtomicBool::new(false),
            kq,
        };
        let submissions = Submissions::new(shared);
        let completions = Completions::new(self.sys.max_events);
        Ok((completions, submissions))
    }
}
