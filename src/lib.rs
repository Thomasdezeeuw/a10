//! The [A10] io_uring library.
//!
//! This library is meant as a low-level library safely exposing the io_uring
//! API. For simplicity this only has two main types and a number of helper
//! types:
//!  * [`Ring`] is a wrapper around io_uring used to poll for completion events.
//!  * [`AsyncFd`] is a wrapper around a file descriptor that provides a safe
//!    API to schedule operations.
//!
//! Some modules provide ways to create `AsyncFd`, e.g. [`OpenOptions`], others
//! are simply a place to expose the [`Future`]s supporting the scheduled
//! operations. The modules try to follow the same structure as that of the
//! standard library.
//!
//! Additional documentation can be found in the [`io_uring(7)`] manual.
//!
//! [A10]: https://en.wikipedia.org/wiki/A10_motorway_(Netherlands)
//! [`OpenOptions`]: fs::OpenOptions
//! [`Future`]: std::future::Future
//! [`io_uring(7)`]: https://man7.org/linux/man-pages/man7/io_uring.7.html
//!
//! # Notes
//!
//! Most I/O operations need ownership of the data, e.g. a buffer, so it can
//! delay deallocation if needed. For example when a `Future` is dropped before
//! being polled to completion. This data can be retrieved again by using the
//! [`Extract`] trait.
//!
//! # Examples
//!
//! Examples can be found in the examples directory of the source code,
//! [available online on GitHub].
//!
//! [available online on GitHub]: https://github.com/Thomasdezeeuw/a10/tree/main/examples

#![cfg_attr(
    feature = "nightly",
    feature(async_iterator, cfg_sanitize, io_error_more)
)]

use std::ptr;
use std::time::Duration;

// This must come before the other modules for the documentation.
pub mod fd;

mod asan;
mod config;
mod msan;
mod op;
#[cfg(unix)]
mod unix;

pub mod extract;
pub mod fs;
pub mod io;
pub mod mem;
pub mod net;
pub mod pipe;
pub mod process;

#[cfg(any(target_os = "android", target_os = "linux"))]
mod io_uring;
#[cfg(any(target_os = "android", target_os = "linux"))]
use io_uring as sys;

#[cfg(any(
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "tvos",
    target_os = "visionos",
    target_os = "watchos",
))]
mod kqueue;
#[cfg(any(
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "tvos",
    target_os = "visionos",
    target_os = "watchos",
))]
use kqueue as sys;

#[doc(inline)]
pub use config::Config;
#[doc(no_inline)]
pub use extract::Extract;
#[doc(no_inline)]
pub use fd::AsyncFd;

/// This type represents the user space side of an io_uring.
///
/// An io_uring is split into two queues: the submissions and completions queue.
/// The [`SubmissionQueue`] is public, but doesn't provide many methods. The
/// `SubmissionQueue` is used by I/O types in the crate to schedule asynchronous
/// operations.
///
/// The completions queue is not exposed by the crate and only used internally.
/// Instead it will wake the [`Future`]s exposed by the various I/O types, such
/// as [`AsyncFd::write`]'s [`Write`] `Future`.
///
/// [`Future`]: std::future::Future
/// [`AsyncFd::write`]: AsyncFd::write
/// [`Write`]: io::Write
#[derive(Debug)]
pub struct Ring {
    cq: sys::Completions,
    sq: sys::Submissions,
}

impl Ring {
    /// Configure a `Ring`.
    pub const fn config<'r>() -> Config<'r> {
        Config {
            sys: crate::sys::Config::new(),
        }
    }

    /// Create a new `Ring` with the default configuration.
    ///
    /// For more configuration options see [`Config`].
    #[doc(alias = "io_uring_setup")]
    #[doc(alias = "kqueue")]
    pub fn new() -> io::Result<Ring> {
        Ring::config().build()
    }

    /// Returns the `SubmissionQueue` used by this ring.
    ///
    /// The submission queue can be used to queue asynchronous I/O operations.
    pub fn sq(&self) -> &SubmissionQueue {
        SubmissionQueue::from_ref(&self.sq)
    }

    /// Poll the ring for completions.
    ///
    /// This will wake all completed [`Future`]s with the result of their
    /// operations.
    ///
    /// [`Future`]: std::future::Future
    #[doc(alias = "io_uring_enter")]
    #[doc(alias = "kevent")]
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.cq.poll(self.sq.shared(), timeout)
    }
}

/// Queue to submit asynchronous operations to.
///
/// This type doesn't have many public methods, but is used by all I/O types, to
/// queue asynchronous operations. The queue can be acquired by using
/// [`Ring::sq`].
///
/// The submission queue can be shared by cloning it, it's a cheap operation.
#[derive(Clone, Debug)]
#[repr(transparent)]
pub struct SubmissionQueue(sys::Submissions);

impl SubmissionQueue {
    pub(crate) fn from_ref(submissions: &sys::Submissions) -> &SubmissionQueue {
        // SAFETY: this is safe because `SubmissionQueue` and `sys::Submissions`
        // have the same layout due to `repr(transparent)`.
        unsafe { &*ptr::from_ref(submissions).cast() }
    }

    /// Wake the connected [`Ring`].
    ///
    /// All this does is interrupt a call to [`Ring::poll`].
    pub fn wake(&self) {
        if let Err(err) = self.0.wake() {
            log::warn!("failed to wake a10::Ring: {err}");
        }
    }

    pub(crate) fn submissions(&self) -> &sys::Submissions {
        &self.0
    }

    /// Returns itself.
    ///
    /// Used by the operation macro to be generic over `SubmissionQueue` and
    /// `AsyncFd`.
    pub(crate) const fn sq(&self) -> &SubmissionQueue {
        self
    }
}

/// Helper macro to execute a system call that returns an `io::Result`.
macro_rules! syscall {
    ($fn: ident ( $($arg: expr),* $(,)? ) ) => {{
        #[allow(unused_unsafe)]
        let res = unsafe { libc::$fn($( $arg, )*) };
        if res == -1 {
            ::std::result::Result::Err(::std::io::Error::last_os_error())
        } else {
            ::std::result::Result::Ok(res)
        }
    }};
}

/// Link to online manual.
#[rustfmt::skip]
macro_rules! man_link {
    ($syscall: tt ( $section: tt ) ) => {
        concat!(
            "\n\nAdditional documentation can be found in the ",
            "[`", stringify!($syscall), "(", stringify!($section), ")`]",
            "(https://man7.org/linux/man-pages/man", stringify!($section), "/", stringify!($syscall), ".", stringify!($section), ".html)",
            " manual.\n"
        )
    };
}

macro_rules! new_flag {
    (
        $(
        $(#[$type_meta:meta])*
        $type_vis: vis struct $type_name: ident ( $type_repr: ty ) $(impl BitOr $( $type_or: ty )*)? {
            $(
            $(#[$value_meta:meta])*
            $value_name: ident = $libc: ident :: $value_type: ident,
            )*
        }
        )+
    ) => {
        $(
        $(#[$type_meta])*
        #[derive(Copy, Clone, Eq, PartialEq)]
        $type_vis struct $type_name(pub(crate) $type_repr);

        impl $type_name {
            $(
            $(#[$value_meta])*
            #[allow(trivial_numeric_casts, clippy::cast_sign_loss)]
            $type_vis const $value_name: $type_name = $type_name($libc::$value_type as $type_repr);
            )*
        }

        $crate::debug_detail!(impl for $type_name($type_repr) match $( $(#[$value_meta])* $libc::$value_type ),*);

        $(
        impl std::ops::BitOr for $type_name {
            type Output = Self;

            fn bitor(self, rhs: Self) -> Self::Output {
                $type_name(self.0 | rhs.0)
            }
        }

        $(
        impl std::ops::BitOr<$type_or> for $type_name {
            type Output = Self;

            #[allow(clippy::cast_sign_loss)]
            fn bitor(self, rhs: $type_or) -> Self::Output {
                $type_name(self.0 | rhs as $type_repr)
            }
        }
        )*
        )?
        )+
    };
}

macro_rules! debug_detail {
    (
        // Match a value exactly.
        impl for $type: ident ($type_repr: ty) match
        $( $( #[$meta: meta] )* $libc: ident :: $flag: ident ),* $(,)?
    ) => {
        impl ::std::fmt::Debug for $type {
            #[allow(trivial_numeric_casts, unreachable_patterns, unreachable_code, unused_doc_comments, clippy::bad_bit_mask)]
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                mod consts {
                    $(
                    $(#[$meta])*
                    pub(super) const $flag: $type_repr = $libc :: $flag as $type_repr;
                    )*
                }

                f.write_str(match self.0 {
                    $(
                    $(#[$meta])*
                    consts::$flag => stringify!($flag),
                    )*
                    value => return value.fmt(f),
                })
            }
        }
    };
    (
        // Match a value exactly.
        match $type: ident ($event_type: ty),
        $( $( #[$meta: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
        struct $type($event_type);

        $crate::debug_detail!(impl for $type($event_type) match $( $(#[$meta])* $libc::$flag ),*);
    };
    (
        // Integer bitset.
        bitset $type: ident ($event_type: ty),
        $( $( #[$meta: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
        struct $type($event_type);

        impl ::std::fmt::Debug for $type {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                let mut written_one = false;
                $(
                    $(#[$meta])*
                    #[allow(clippy::bad_bit_mask)] // Apparently some flags are zero.
                    {
                        if self.0 & $libc :: $flag != 0 {
                            if !written_one {
                                write!(f, "{}", stringify!($flag))?;
                                written_one = true;
                            } else {
                                write!(f, "|{}", stringify!($flag))?;
                            }
                        }
                    }
                )+
                if !written_one {
                    write!(f, "(empty)")
                } else {
                    Ok(())
                }
            }
        }
    };
}

use {debug_detail, man_link, new_flag, syscall};

/// Lock `mutex` clearing any poison set.
fn lock<'a, T>(mutex: &'a std::sync::Mutex<T>) -> std::sync::MutexGuard<'a, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(err) => {
            mutex.clear_poison();
            err.into_inner()
        }
    }
}
