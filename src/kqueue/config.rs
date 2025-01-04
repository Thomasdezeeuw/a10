//! kqueue configuration.

use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;
use std::{io, mem, ptr};

use crate::kqueue::{self, Completions, Shared, Submissions};
use crate::{syscall, WAKE_ID};

#[derive(Debug, Clone)]
pub(crate) struct Config<'r> {
    max_events: Option<usize>,
    max_change_list_size: Option<usize>,
    _unused: PhantomData<&'r ()>,
}

impl<'r> Config<'r> {
    pub(crate) const fn new() -> Config<'r> {
        Config {
            max_events: None,
            max_change_list_size: None,
            _unused: PhantomData,
        }
    }
}

/// kqueue specific configuration.
impl<'r> crate::Config<'r> {
    /// Set the maximum number of events that can be collected in a single call
    /// to `kevent(2)`.
    ///
    /// Defaults to the same value as the maximum number of queued operations
    /// (see [`Ring::config`]).
    ///
    /// [`Ring::config`]: crate::Ring::config
    pub const fn max_events(mut self, max_events: usize) -> Self {
        self.sys.max_events = Some(max_events);
        self
    }

    /// Set the maximum number of submissions that are buffered before
    /// submitting to the kernel, without waiting on a call to [`Ring::poll`].
    ///
    /// This must be larger or equal to [`max_events`]. Defaults to 64 changes.
    ///
    /// [`Ring::poll`]: crate::Ring::poll
    /// [`max_events`]: crate::Config::max_events
    pub const fn max_change_list_size(mut self, max: usize) -> Self {
        self.sys.max_change_list_size = Some(max);
        self
    }

    pub(crate) fn build_sys(self) -> io::Result<(Submissions, Shared, Completions)> {
        let max_events = self.sys.max_events.unwrap_or(self.queued_operations);
        #[rustfmt::skip]
        let max_change_list_size = self.sys.max_change_list_size.unwrap_or(if max_events >= 64 { 64 } else { max_events });
        // NOTE: `max_events` must be at least `max_change_list_size` to ensure
        // we can handle all submission errors.
        debug_assert!(
            max_events >= max_change_list_size,
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
            udata: WAKE_ID as _,
            // SAFETY: all zeros is valid for `libc::kevent`.
            ..unsafe { mem::zeroed() }
        };
        syscall!(kevent(kq_fd, &kevent, 1, &mut kevent, 1, ptr::null()))?;
        if (kevent.flags & libc::EV_ERROR) != 0 && kevent.data != 0 {
            return Err(io::Error::from_raw_os_error(kevent.data as i32));
        }

        let submissions = kqueue::Submissions::new(max_change_list_size);
        let change_list = Mutex::new(Vec::new());
        let shared = kqueue::Shared { kq, change_list };
        let completions = kqueue::Completions::new(max_events);
        Ok((submissions, shared, completions))
    }
}
