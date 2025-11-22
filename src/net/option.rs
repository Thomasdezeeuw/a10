//! Socket options.
//!
//! See [`AsyncFd::socket_option2`] and [`AsyncFd::set_socket_option2`].
//!
//! [`AsyncFd::socket_option2`]: crate::fd::AsyncFd::socket_option2
//! [`AsyncFd::set_socket_option2`]: crate::fd::AsyncFd::set_socket_option2

use std::io;
use std::mem::MaybeUninit;

use crate::net::{Level, Opt, SocketOpt};

/// Trait that defines how get the value of a socket option.
///
/// See [`AsyncFd::socket_option2`].
pub trait Get: private::Get + Sized {
    /// Returned output.
    type Output: Sized;
    /// Type used by the OS.
    ///
    /// Often this is an `i32` (`libc::c_int`), which can be mean different
    /// things depending on the socket retrieved.
    #[doc(hidden)]
    type Storage: Sized;

    /// Level to use, see [`Level`].
    #[doc(hidden)]
    const LEVEL: Level;
    /// Option to reitreve, see [`Opt`].
    #[doc(hidden)]
    const OPT: Opt;

    /// Returns a mutable raw pointer and length to `storage`.
    ///
    /// Default implementation casts a the pointer to `storage` and returns the
    /// size of `Storage` as length.
    ///
    /// # Safety
    ///
    /// Only initialised bytes may be written to the pointer returned.
    #[doc(hidden)]
    unsafe fn as_mut_ptr(storage: &mut MaybeUninit<Self::Storage>) -> (*mut std::ffi::c_void, u32) {
        (
            storage.as_mut_ptr().cast(),
            size_of::<Self::Storage>() as u32,
        )
    }

    /// Initialise the value from `storage`, to which at least `length` bytes
    /// have been written (by the kernel).
    ///
    /// # Safety
    ///
    /// Caller must ensure that at least `length` bytes have been written to
    /// `address`.
    #[doc(hidden)]
    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> Self::Output;
}

/// Trait that defines how set the value of a socket option.
///
/// See [`AsyncFd::set_socket_option2`].
pub trait Set: private::Set + Sized {
    /// Value to set.
    type Value: Sized;
    /// Type used by the OS.
    #[doc(hidden)]
    type Storage: Sized;

    /// Level to use, see [`Level`].
    #[doc(hidden)]
    const LEVEL: Level;
    /// Option to reitreve, see [`Opt`].
    #[doc(hidden)]
    const OPT: Opt;

    /// Returns the value as storage for the OS to read.
    #[doc(hidden)]
    fn as_storage(value: Self::Value) -> Self::Storage;
}

mod private {
    pub trait Get {
        // Just here to ensure it can't be implemented outside of the crate.
        // Because need `Output` to be public we need to have the methods on the
        // public trait, otherwise they would be moved here.
    }

    pub trait Set {
        // Just here to ensure it can't be implemented outside of the crate.
    }
}

/// Get and clear the pending socket error.
#[doc(alias = "SO_ERROR")]
#[doc(alias = "take_error")] // Used by types in std lib.
#[allow(missing_debug_implementations)]
pub enum Error {}

impl Get for Error {
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

impl private::Get for Error {}

/// Allow reuse of local addresses.
#[doc(alias = "SO_REUSEADDR")]
#[allow(missing_debug_implementations)]
pub enum ReuseAddress {}

impl Get for ReuseAddress {
    const LEVEL: Level = Level::SOCKET;
    const OPT: Opt = SocketOpt::REUSE_ADDR.into_opt();

    type Output = bool;
    type Storage = libc::c_int;

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self::Output {
        assert!(length == size_of::<Self::Storage>() as u32);
        unsafe { storage.assume_init() >= 1 }
    }
}

impl private::Get for ReuseAddress {}

impl Set for ReuseAddress {
    const LEVEL: Level = Level::SOCKET;
    const OPT: Opt = SocketOpt::REUSE_ADDR.into_opt();

    type Value = bool;
    type Storage = libc::c_int;

    fn as_storage(value: Self::Value) -> Self::Storage {
        value.into()
    }
}

impl private::Set for ReuseAddress {}
