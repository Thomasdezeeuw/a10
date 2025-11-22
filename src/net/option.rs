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
///
/// [`AsyncFd::socket_option2`]: crate::fd::AsyncFd::socket_option2
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
///
/// [`AsyncFd::set_socket_option2`]: crate::fd::AsyncFd::set_socket_option2
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

new_option! {
    /// Get and clear the pending socket error.
    #[doc(alias = "SO_ERROR")]
    #[doc(alias = "take_error")] // Used by types in std lib.
    pub Error {
        type Storage = libc::c_int;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::ERROR;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> Option<io::Error> {
            assert!(length == size_of::<Self::Storage>() as u32);
            let errno = unsafe { storage.assume_init() };
            if errno == 0 {
                None
            } else {
                Some(io::Error::from_raw_os_error(errno))
            }
        }
    }

    /// Allow reuse of local addresses.
    #[doc(alias = "SO_REUSEADDR")]
    pub ReuseAddress {
        type Storage = libc::c_int;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::REUSE_ADDR;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> bool {
            assert!(length == size_of::<Self::Storage>() as u32);
            unsafe { storage.assume_init() >= 1 }
        }

        fn as_storage(value: bool) -> Self::Storage {
            value.into()
        }
    }

    /// Allow multiple sockets to be bound to an identical socket address.
    #[doc(alias = "SO_REUSEPORT")]
    pub ReusePort {
        type Storage = libc::c_int;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::REUSE_PORT;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> bool {
            assert!(length == size_of::<Self::Storage>() as u32);
            unsafe { storage.assume_init() >= 1 }
        }

        fn as_storage(value: bool) -> Self::Storage {
            value.into()
        }
    }
}

macro_rules! new_option {
    (
        $(
        $(#[$type_meta:meta])*
        $type_vis: vis $type_name: ident {
            type Storage = $storage: ty;
            const LEVEL = $level: expr;
            const OPT = $opt: expr;

            // option::Get implementation.
            $(
            $(
            unsafe fn as_mut_ptr($as_mut_ptr_storage: ident: &mut MaybeUninit<Self::Storage>) -> (*mut std::ffi::c_void, u32) $as_mut_ptr: block
            )?

            unsafe fn init($init_storage: ident: MaybeUninit<Self::Storage>, $init_length: ident: u32) -> $output: ty $init: block
            )?

            // option::Set implementation.
            $(
            fn as_storage($as_storage_value: ident: $value: ty) -> Self::Storage $as_storage: block
            )?
        }
        )+
    ) => {
        $(
        $(#[$type_meta])*
        #[allow(missing_debug_implementations)]
        pub enum $type_name {}

        $(
        impl Get for $type_name {
            type Output = $output;
            type Storage = $storage;

            const LEVEL: Level = $level;
            const OPT: Opt = $opt.into_opt();

            $(
            unsafe fn as_mut_ptr($as_mut_ptr_storage: &mut MaybeUninit<Self::Storage>) -> (*mut std::ffi::c_void, u32) {
                $as_mut_ptr
            }
            )?

            unsafe fn init($init_storage: MaybeUninit<Self::Storage>, $init_length: u32) -> Self::Output {
                $init
            }
        }

        impl private::Get for $type_name {}
        )?

        $(
        impl Set for $type_name {
            type Value = $value;
            type Storage = $storage;

            const LEVEL: Level = $level;
            const OPT: Opt = $opt.into_opt();

            fn as_storage($as_storage_value: Self::Value) -> Self::Storage {
                $as_storage
            }
        }

        impl private::Set for $type_name {}
        )?
        )+
    };
}

use new_option;
