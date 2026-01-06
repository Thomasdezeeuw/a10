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

pub mod io;
pub mod net;

#[cfg(any(target_os = "android", target_os = "linux"))]
mod io_uring;
#[cfg(any(target_os = "android", target_os = "linux"))]
use io_uring as sys;

#[doc(inline)]
pub use config::Config;
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

        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
