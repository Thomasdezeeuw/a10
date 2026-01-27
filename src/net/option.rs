//! Socket options.
//!
//! See [`AsyncFd::socket_option`] and [`AsyncFd::set_socket_option`].
//!
//! [`AsyncFd::socket_option`]: crate::fd::AsyncFd::socket_option
//! [`AsyncFd::set_socket_option`]: crate::fd::AsyncFd::set_socket_option

use std::io;
use std::mem::MaybeUninit;

use crate::net::{self, Level, Opt, SocketOpt};

/// Trait that defines how get the value of a socket option.
///
/// See [`AsyncFd::socket_option`].
///
/// [`AsyncFd::socket_option`]: crate::fd::AsyncFd::socket_option
pub trait Get {
    /// Returned output.
    type Output: Sized;
    /// Type passed to the OS in the `getsockopt(2)` call.
    type Storage: Sized;

    /// Level to use, see [`Level`].
    const LEVEL: Level;
    /// Option to retrieve, see [`Opt`].
    const OPT: Opt;

    /// Returns a mutable raw pointer and length to `storage`.
    ///
    /// Default implementation casts a the pointer to `storage` and returns the
    /// size of `Storage` as length.
    ///
    /// # Safety
    ///
    /// Only initialised bytes may be written to the pointer returned.
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
    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> Self::Output;
}

/// Trait that defines how set the value of a socket option.
///
/// See [`AsyncFd::set_socket_option`].
///
/// [`AsyncFd::set_socket_option`]: crate::fd::AsyncFd::set_socket_option
pub trait Set {
    /// Value to set.
    type Value: Sized;
    /// Type passed to the OS in the `setsockopt(2)` call.
    type Storage: Sized;

    /// Level to use, see [`Level`].
    const LEVEL: Level;
    /// Option to retrieve, see [`Opt`].
    const OPT: Opt;

    /// Returns the value as storage for the OS to read.
    fn as_storage(value: Self::Value) -> Self::Storage;
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

    /// Enable sending of keep-alive messages on connection-oriented
    /// sockets.
    #[doc(alias = "SO_KEEPALIVE")]
    pub KeepAlive {
        type Storage = libc::c_int;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::KEEP_ALIVE;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> bool {
            assert!(length == size_of::<Self::Storage>() as u32);
            unsafe { storage.assume_init() >= 1 }
        }

        fn as_storage(value: bool) -> Self::Storage {
            value.into()
        }
    }

    /// Linger option.
    #[doc(alias = "SO_LINGER")]
    pub Linger {
        type Storage = libc::linger;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::LINGER;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> Option<u32> {
            assert!(length == size_of::<Self::Storage>() as u32);
            let linger = unsafe { storage.assume_init() };
            if linger.l_onoff > 0 {
                Some(linger.l_linger.cast_unsigned())
            } else {
                None
            }
        }

        fn as_storage(value: Option<u32>) -> Self::Storage {
            libc::linger {
                l_onoff: value.is_some().into(),
                l_linger: value.unwrap_or(0).cast_signed(),
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

    /// Type.
    #[doc(alias = "SO_TYPE")]
    pub Type {
        type Storage = u32;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::TYPE;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> net::Type {
            assert!(length == size_of::<Self::Storage>() as u32);
            unsafe { net::Type(storage.assume_init()) }
        }
    }
}

#[cfg(any(
    target_os = "android",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd"
))]
new_option! {
    /// Domain.
    #[doc(alias = "SO_DOMAIN")]
    pub Domain {
        type Storage = libc::c_int;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::DOMAIN;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> net::Domain {
            assert!(length == size_of::<Self::Storage>() as u32);
            unsafe { net::Domain(storage.assume_init()) }
        }
    }

    /// Retrieves the socket protocol.
    #[doc(alias = "SO_PROTOCOL")]
    pub Protocol {
        type Storage = u32;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::PROTOCOL;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> net::Protocol {
            assert!(length == size_of::<Self::Storage>() as u32);
            unsafe { net::Protocol(storage.assume_init()) }
        }
    }

    /// Returns a value indicating whether or not this socket has been
    /// marked to accept connections with `listen(2)`.
    #[doc(alias = "SO_ACCEPTCONN")]
    pub Accept {
        type Storage = libc::c_int;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::ACCEPT_CONN;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> bool {
            assert!(length == size_of::<Self::Storage>() as u32);
            unsafe { storage.assume_init() >= 1 }
        }
    }
}

#[cfg(any(target_os = "android", target_os = "linux"))]
new_option! {
    /// CPU affinity.
    #[doc(alias = "SO_INCOMING_CPU")]
    pub IncomingCpu {
        type Storage = libc::c_int;
        const LEVEL = Level::SOCKET;
        const OPT = SocketOpt::INCOMING_CPU;

        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: u32) -> Option<u32> {
            assert!(length == size_of::<Self::Storage>() as u32);
            let value = unsafe { storage.assume_init() };
            if value.is_negative() { None } else { Some(value.cast_unsigned()) }
        }

        fn as_storage(value: u32) -> Self::Storage {
            value.cast_signed()
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
        )*
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
        )?
        )*
    };
}

use new_option;
