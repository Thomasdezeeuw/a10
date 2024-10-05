//! kqueue configuration.

use std::io;
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use crate::{sys, syscall, Ring};

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
        syscall!(fcntl(kq.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC))?;
        let events = Vec::with_capacity(self.events_capacity as usize);
        Ok(Ring {
            sys: sys::Ring { kq, events },
        })
    }
}
