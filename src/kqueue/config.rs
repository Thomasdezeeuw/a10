//! kqueue configuration.

use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;
use std::{io, mem, ptr};

use crate::sys::{self, Completions, Shared, Submissions};
use crate::{syscall, WAKE_ID};

#[derive(Debug, Clone)]
pub(crate) struct Config<'r> {
    max_change_list_size: Option<usize>,
    _unused: PhantomData<&'r ()>,
}

impl<'r> Config<'r> {
    pub(crate) const fn new() -> Config<'r> {
        Config {
            max_change_list_size: None,
            _unused: PhantomData,
        }
    }
}

impl<'r> crate::Config<'r> {
    /// Maximum size of the change list before it's submitted to the kernel,
    /// without waiting on a call to [`Ring::poll`].
    ///
    /// Defaults to the amount of `entries` (passed to [`Ring::new`]), with a
    /// maximum of 64 changes.
    ///
    /// [`Ring::poll`]: crate::Ring::poll
    ///
    /// # Notes
    ///
    /// If the `entries` is smaller than `max` it will be set to `max`.
    ///
    /// [`Ring::new`]: crate::Ring::new
    pub fn max_change_list_size(mut self, max: usize) -> Self {
        self.sys.max_change_list_size = Some(max);
        if self.queued_operations < max as u32 {
            self.queued_operations = max as u32;
        }
        self
    }

    pub(crate) fn _build(self) -> io::Result<(Submissions, Shared, Completions)> {
        #[rustfmt::skip]
        let crate::Config { queued_operations, sys: Config { max_change_list_size, .. }} = self;
        let queued_operations = queued_operations as usize;
        // NOTE: `queued_operations` must be at least `max_change_list_size` to
        // ensure we can handle all submission errors.
        #[rustfmt::skip]
        let max_change_list_size = max_change_list_size.unwrap_or(if queued_operations > 64 { 64 } else { queued_operations });
        debug_assert!(queued_operations >= max_change_list_size);

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

        let submissions = sys::Submissions::new(max_change_list_size);
        let change_list = Mutex::new(Vec::new());
        let shared = sys::Shared { kq, change_list };
        let completions = sys::Completions::new(queued_operations);
        Ok((submissions, shared, completions))
    }
}
