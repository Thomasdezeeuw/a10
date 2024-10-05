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
    events_capacity: u32,
    _unused: PhantomData<&'r ()>,
}

impl<'r> Config<'r> {
    pub(crate) const fn new(events_capacity: u32) -> Config<'r> {
        Config {
            events_capacity,
            _unused: PhantomData,
        }
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

        let change_list = Mutex::new(Vec::new());
        let shared = sys::Shared { kq, change_list };
        let poll = sys::Completions::new(self.events_capacity as usize);
        Ring::build(
            shared,
            poll,
            self.events_capacity as usize, // TODO: add option for # queued operations.
        )
    }
}
