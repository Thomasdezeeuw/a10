//! Socket options.
//!
//! See [`AsyncFd::socket_option2`].
//!
//! [`AsyncFd::socket_option2`]: crate::fd::AsyncFd::socket_option2

use std::io;
use std::mem::MaybeUninit;

use crate::net::{Level, Opt, SocketOpt, private};

#[doc(no_inline)]
pub use crate::net::GetSocketOption;

/// Get and clear the pending socket error.
#[doc(alias = "SO_ERROR")]
#[doc(alias = "take_error")] // Used by types in std lib.
#[allow(missing_debug_implementations)]
pub enum Error {}

impl GetSocketOption for Error {
    const LEVEL: Level = Level::SOCKET;
    const OPT: Opt = SocketOpt::ERROR.into_opt();

    type Output = Option<io::Error>;
    type Storage = libc::c_int;

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self::Output {
        assert!(length == size_of::<Self::Storage>() as u32);
        let errno = unsafe { storage.assume_init() };
        if errno == 0 {
            None
        } else {
            Some(io::Error::from_raw_os_error(errno))
        }
    }
}

impl private::GetSocketOption for Error {}
