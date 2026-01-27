//! The A10 I/O library. [^1]
//!
//! This library is meant as a low-level library safely exposing different OS's
//! abilities to perform non-blocking I/O.
//!
//! For simplicity this only has the following main types and a number of helper
//! types:
//!  * [`AsyncFd`] is a wrapper around a file descriptor that provides a safe
//!    API to perform I/O operations such as `read(2)` using [`Future`]s.
//!  * [`SubmissionQueue`] is needed to start operations, such as opening a file
//!    or new socket, but on its own can't do much.
//!  * [`Ring`] needs be [polled] so that the I/O operations can make progress.
//!
//! Some modules provide ways to create `AsyncFd`, e.g. [`socket`] or
//! [`OpenOptions`], others are simply a place to expose the [`Future`]s
//! supporting the I/O operations. The modules try to roughly follow the same
//! structure as that of the standard library.
//!
//! [polled]: Ring::poll
//! [`socket`]: net::socket
//! [`OpenOptions`]: fs::OpenOptions
//! [`Future`]: std::future::Future
//!
//! # Implementation Notes
//!
//! On Linux this uses io_uring, which is a completion based API. For the BSD
//! family of OS (FreeBSD, OpenBSD, NetBSD, etc.) and for the Apple family
//! (macOS, iOS, etc.) this uses kqueue, which is a poll based API.
//!
//! To support both the completion and poll based API most I/O operations need
//! ownership of the data, e.g. a buffer, so it can delay deallocation if
//! needed. [^2] The input data can be retrieved again by using the [`Extract`]
//! trait.
//!
//! Additional documentation can be found in the [`io_uring(7)`] and
//! [`kqueue(2)`] manuals.
//!
//! [`io_uring(7)`]: https://man7.org/linux/man-pages/man7/io_uring.7.html
//! [`kqueue(2)`]: https://man.freebsd.org/cgi/man.cgi?query=kqueue
//!
//! # Examples
//!
//! Examples can be found in the examples directory of the source code,
//! [available online on GitHub].
//!
//! [available online on GitHub]: https://github.com/Thomasdezeeuw/a10/tree/main/examples
//!
//! [^1]: The name A10 comes from the [A10 ring road around Amsterdam], which
//!       relates to the ring buffers that io_uring uses in its design.
//! [^2]: Delaying of the deallocation needs to happen for completion based APIs
//!       where an I/O operation `Future` is dropped before it's complete -- the
//!       OS will continue to use the resources, which would result in a
//!       use-after-free bug.
//!
//! [A10 ring road around Amsterdam]: https://en.wikipedia.org/wiki/A10_motorway_(Netherlands)

#![cfg_attr(feature = "nightly", feature(async_iterator, cfg_sanitize))]

#[cfg(not(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "ios",
    target_os = "linux",
    target_os = "macos",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "tvos",
    target_os = "visionos",
    target_os = "watchos",
)))]
compile_error!("OS not supported");

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

/// Ring.
///
/// The API on this type is quite minimal. It provides access to the
/// [`SubmissionQueue`], which is used to perform I/O operations. And it exposes
/// [`Ring::poll`] needs to be called to make progress on the operations and
/// mark the [`Future`]s are ready to poll.
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
    pub fn sq(&self) -> SubmissionQueue {
        SubmissionQueue(self.sq.clone())
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
/// This type doesn't have many public methods, but is used by all I/O types to
/// queue asynchronous operations. The queue can be acquired by using
/// [`Ring::sq`].
///
/// The submission queue can be shared by cloning it, it's a cheap operation.
#[derive(Clone, Debug)]
#[repr(transparent)]
pub struct SubmissionQueue(sys::Submissions);

impl SubmissionQueue {
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
        $type_vis: vis struct $type_name: ident ( $type_repr: ty ) $( impl BitOr $( $type_or: ty )* )? {
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

            // Need the value_meta to set the cfg attribute, but that also
            // includes documentation, which we can ignore.
            #[allow(unused_doc_comments, dead_code)]
            pub(crate) const ALL_VALUES: &[$type_name] = &[
                $(
                $(#[$value_meta])*
                $type_name::$value_name,
                )*
            ];
        }

        $crate::debug_detail!(impl match for $type_name($type_repr), $( $(#[$value_meta])* $libc::$value_type ),*);

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
        match $type: ident ($event_type: ty),
        $( $( #[$meta: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
        struct $type($event_type);

        $crate::debug_detail!(impl match for $type($event_type), $( $(#[$meta])* $libc::$flag ),*);
    };
    (
        // Match a value exactly.
        impl match for $type: ident ($type_repr: ty),
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
        // Integer bitset.
        bitset $type: ident ($event_type: ty),
        $( $( #[$meta: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
        struct $type($event_type);

        $crate::debug_detail!(impl bitset for $type($event_type), $( $(#[$meta])* $libc::$flag ),*);
    };
    (
        // Integer bitset.
        impl bitset for $type: ident ($event_type: ty),
        $( $( #[$meta: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
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

/// Get mutable access to the lock's data.
#[cfg(any(target_os = "android", target_os = "linux"))]
fn get_mut<'a, T>(mutex: &'a mut std::sync::Mutex<T>) -> &'a mut T {
    match mutex.get_mut() {
        Ok(guard) => guard,
        Err(err) => err.into_inner(),
    }
}

/// Trait to work with results for singleshot (`io::Result`) and multishot
/// (`Option<io::Result>`) operations.
// Replace this with std::ops::FromResidual once stable.
#[allow(unused)]
trait OpPollResult<T> {
    fn from_ok(ok: T) -> Self;
    fn from_err(err: io::Error) -> Self;
    fn from_res(res: io::Result<T>) -> Self;
    fn done() -> Self;
}

impl<T> OpPollResult<T> for io::Result<T> {
    fn from_ok(ok: T) -> Self {
        Ok(ok)
    }

    fn from_err(err: io::Error) -> Self {
        Err(err)
    }

    fn from_res(res: io::Result<T>) -> Self {
        res
    }

    fn done() -> Self {
        unreachable!()
    }
}

impl<T> OpPollResult<T> for Option<io::Result<T>> {
    fn from_ok(ok: T) -> Self {
        Some(Ok(ok))
    }

    fn from_err(err: io::Error) -> Self {
        Some(Err(err))
    }

    fn from_res(res: io::Result<T>) -> Self {
        Some(res)
    }

    fn done() -> Self {
        None
    }
}
