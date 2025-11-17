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
#![warn(
    anonymous_parameters,
    bare_trait_objects,
    missing_debug_implementations,
    missing_docs,
    trivial_numeric_casts,
    unused_extern_crates,
    unused_import_braces,
    variant_size_differences
)]

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use std::{fmt, task};

// This must come before the other modules for the documentation.
pub mod fd;

mod asan;
mod bitmap;
mod config;
mod cq;
mod drop_waker;
mod msan;
mod op;
mod sq;
#[cfg(unix)]
mod unix;

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

pub mod cancel;
pub mod extract;
pub mod fs;
pub mod io;
pub mod mem;
pub mod msg;
pub mod net;
pub mod pipe;
pub mod poll;
pub mod process;

#[doc(no_inline)]
pub use cancel::Cancel;
#[doc(inline)]
pub use config::Config;
#[doc(no_inline)]
pub use extract::Extract;
#[doc(no_inline)]
pub use fd::AsyncFd;

use crate::bitmap::AtomicBitMap;
use crate::sys::Submission;

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
    sq: SubmissionQueue,
    cq: cq::Queue<sys::Implementation>,
}

impl Ring {
    /// Configure a `Ring`.
    ///
    /// `queued_operations` is the number of queued operations, i.e. the number
    /// of concurrent A10 operation.
    ///
    /// # Notes
    ///
    /// A10 uses `IORING_SETUP_SQPOLL` by default for io_uring, which required
    /// Linux kernel 5.11 to work correctly. Furthermore before Linux 5.13 the
    /// user needs the `CAP_SYS_NICE` capability if run as non-root. This can be
    /// disabled by [`Config::with_kernel_thread`].
    pub const fn config<'r>(queued_operations: usize) -> Config<'r> {
        Config {
            queued_operations,
            sys: crate::sys::Config::new(),
        }
    }

    /// Create a new `Ring` with the default configuration.
    ///
    /// For more configuration options see [`Config`].
    #[doc(alias = "io_uring_setup")]
    #[doc(alias = "kqueue")]
    pub fn new(queued_operations: usize) -> io::Result<Ring> {
        Ring::config(queued_operations).build()
    }

    /// Build a new `Ring`.
    fn build(
        submissions: sys::Submissions,
        shared_data: sys::Shared,
        completions: sys::Completions,
        queued_operations: usize,
    ) -> Ring {
        let shared = SharedState::new(submissions, shared_data, queued_operations);
        let sq = SubmissionQueue::new(shared.clone());
        let cq = cq::Queue::new(completions, shared);
        Ring { sq, cq }
    }

    /// Returns the `SubmissionQueue` used by this ring.
    ///
    /// The `SubmissionQueue` can be used to queue asynchronous I/O operations.
    pub const fn submission_queue(&self) -> &SubmissionQueue {
        &self.sq
    }

    /// Poll the ring for completions.
    ///
    /// This will wake all completed [`Future`]s with the result of their
    /// operations.
    ///
    /// If a zero duration timeout (i.e. `Some(Duration::ZERO)`) is passed this
    /// function will only wake all already completed operations. When using
    /// io_uring it also guarantees to not make a system call, but it also means
    /// it doesn't guarantee at least one completion was processed.
    ///
    /// [`Future`]: std::future::Future
    #[doc(alias = "io_uring_enter")]
    #[doc(alias = "kevent")]
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        self.cq.poll(timeout)
    }
}

/// Queue to submit asynchronous operations to.
///
/// This type doesn't have many public methods, but is used by all I/O types, to
/// queue asynchronous operations. The queue can be acquired by using
/// [`Ring::submission_queue`].
///
/// The submission queue can be shared by cloning it, it's a cheap operation.
#[derive(Clone)]
pub struct SubmissionQueue {
    inner: sq::Queue<sys::Implementation>,
}

impl SubmissionQueue {
    const fn new(shared: Arc<SharedState<sys::Implementation>>) -> SubmissionQueue {
        SubmissionQueue {
            inner: sq::Queue::new(shared),
        }
    }

    /// Wake the connected [`Ring`].
    ///
    /// All this does is interrupt a call to [`Ring::poll`].
    pub fn wake(&self) {
        self.inner.wake();
    }

    /// See [`sq::Queue::get_op`].
    #[allow(clippy::type_complexity)]
    pub(crate) unsafe fn get_op(
        &self,
        op_id: OperationId,
    ) -> MutexGuard<
        '_,
        Option<QueuedOperation<<<<sys::Implementation as Implementation>::Completions as cq::Completions>::Event as cq::Event>::State>>,
    >{
        unsafe { self.inner.get_op(op_id) }
    }

    /// See [`sq::Queue::make_op_available`].
    #[allow(clippy::type_complexity)]
    pub(crate) unsafe fn make_op_available(
        &self,
        op_id: OperationId,
        op: MutexGuard<
        '_,
        Option<QueuedOperation<<<<sys::Implementation as Implementation>::Completions as cq::Completions>::Event as cq::Event>::State>>,
        >,
    ) {
        unsafe { self.inner.make_op_available(op_id, op) };
    }
}

impl fmt::Debug for SubmissionQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

/// State shared between the submission and completion side.
struct SharedState<I: Implementation> {
    /// [`sq::Submissions`] implementation.
    submissions: I::Submissions,
    /// Data shared between the submission and completion queues.
    data: I::Shared,
    /// Boolean indicating a thread is [`Ring::poll`]ing.
    is_polling: AtomicBool,
    /// Bitmap which can be used to create [`OperationId`]s, used as index into
    /// `queued_ops`.
    op_ids: Box<AtomicBitMap>,
    /// State of queued operations.
    ///
    /// Indexed by a [`OperationId`]s, created by `op_ids`.
    #[rustfmt::skip]
    #[allow(clippy::type_complexity)]
    queued_ops: Box<[Mutex<Option<QueuedOperation<<<I::Completions as cq::Completions>::Event as cq::Event>::State>>>]>,
    /// Futures that are waiting for a slot in `queued_ops`.
    blocked_futures: Mutex<Vec<task::Waker>>,
}

impl<I: Implementation> SharedState<I> {
    /// `queued_operations` is the maximum number of queued operations, will be
    /// rounded up depending on the capacity of `AtomicBitMap`.
    fn new(
        submissions: I::Submissions,
        data: I::Shared,
        queued_operations: usize,
    ) -> Arc<SharedState<I>> {
        let op_ids = AtomicBitMap::new(queued_operations);
        let mut queued_ops = Vec::with_capacity(op_ids.capacity());
        queued_ops.resize_with(queued_ops.capacity(), || Mutex::new(None));
        let queued_ops = queued_ops.into_boxed_slice();
        let blocked_futures = Mutex::new(Vec::new());
        Arc::new(SharedState {
            submissions,
            data,
            is_polling: AtomicBool::new(false),
            op_ids,
            queued_ops,
            blocked_futures,
        })
    }
}

impl<I: Implementation> fmt::Debug for SharedState<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedState")
            .field("submissions", &self.submissions)
            .field("data", &self.data)
            .field("is_polling", &self.is_polling)
            .field("op_ids", &self.op_ids)
            .field("queued_ops", &self.queued_ops)
            .field("blocked_futures", &self.blocked_futures)
            .finish()
    }
}

/// In progress/queued operation.
#[derive(Debug)]
struct QueuedOperation<T> {
    /// State of the operation.
    state: T,
    /// True if the connected `Future`/`AsyncIterator` is dropped and thus no
    /// longer will retrieve the result.
    dropped: bool,
    /// Boolean used by operations that result in multiple completion events.
    /// For example zero copy: one completion to report the result another to
    /// indicate the resources are no longer used.
    /// For io_uring multishot this will be true if no more completion events
    /// are coming, for example in case a previous event returned an error.
    done: bool,
    /// Waker to wake when the operation is done.
    waker: task::Waker,
}

impl<T> QueuedOperation<T> {
    const fn new(state: T, waker: task::Waker) -> QueuedOperation<T> {
        QueuedOperation {
            state,
            dropped: false,
            done: false,
            waker,
        }
    }

    /// Update the waker to `waker`, if it's different.
    fn update_waker(&mut self, waker: &task::Waker) {
        if !self.waker.will_wake(waker) {
            self.waker.clone_from(waker);
        }
    }

    fn prep_retry(&mut self)
    where
        T: cq::OperationState,
    {
        self.state.prep_retry();
    }
}

/// Operation id.
///
/// Used to relate completion events to submission events and operations. Also
/// used as index into [`SharedState::queued_ops`], created by
/// [`SharedState::op_ids`].
type OperationId = usize;

/// Id to use for internal wake ups.
const WAKE_ID: OperationId = usize::MAX;
/// Id to use for submissions without a completions event (in the case we do
/// actually get a completion event).
const NO_COMPLETION_ID: OperationId = usize::MAX - 1;

/// Platform specific implementation.
trait Implementation {
    /// Data shared between the submission and completion queues.
    type Shared: fmt::Debug + Sized;

    /// See [`sq::Submissions`].
    type Submissions: sq::Submissions<Shared = Self::Shared>;

    /// See [`cq::Completions`].
    type Completions: cq::Completions<Shared = Self::Shared>;
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

        $crate::debug_detail!(impl for $type_name($type_repr) match $( $libc::$value_type ),*);

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

#[allow(unused_macros)] // Not used on all OS.
macro_rules! debug_detail {
    (
        // Match a value exactly.
        impl for $type: ident ($type_repr: ty) match
        $( $( #[$target: meta] )* $libc: ident :: $flag: ident ),* $(,)?
    ) => {
        impl ::std::fmt::Debug for $type {
            #[allow(trivial_numeric_casts, unreachable_patterns, unreachable_code, clippy::bad_bit_mask)]
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                mod consts {
                    $(
                    $(#[$target])*
                    pub(super) const $flag: $type_repr = $libc :: $flag as $type_repr;
                    )*
                }

                f.write_str(match self.0 {
                    $(
                    $(#[$target])*
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
        $( $( #[$target: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
        struct $type($event_type);

        $crate::debug_detail!(impl for $type($event_type) match $( $libc::$flag ),*);
    };
    (
        // Integer bitset.
        bitset $type: ident ($event_type: ty),
        $( $( #[$target: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
        struct $type($event_type);

        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut written_one = false;
                $(
                    $(#[$target])*
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

#[allow(unused_imports)] // Not used on all OS.
use {debug_detail, man_link, new_flag, syscall};
