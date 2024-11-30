#![allow(dead_code)] // FIXME: remove.

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
//! ## Examples
//!
//! The example below implements the `cat(1)` program that concatenates files
//! and prints them to standard out.
//!
//! ```
//! use std::path::PathBuf;
//! use std::future::Future;
//! use std::io;
//!
//! use a10::fd::File;
//! use a10::{Extract, Ring, SubmissionQueue};
//!
//! # fn main() -> io::Result<()> {
//! // Create a new I/O uring supporting 8 submission entries.
//! let mut ring = Ring::new(8)?;
//!
//! // Get access to the submission queue, used to... well queue submissions.
//! let sq = ring.submission_queue().clone();
//! // A10 makes use of `Future`s to represent the asynchronous nature of
//! // io_uring.
//! let future = cat(sq, "./src/lib.rs");
//!
//! // This `block_on` function would normally be implement by a `Future`
//! // runtime, but we show a simple example implementation below.
//! block_on(&mut ring, future)?;
//! # Ok(()) }
//!
//! /// A "cat" like function, which reads from `filename` and writes it to
//! /// standard out.
//! async fn cat(sq: SubmissionQueue, filename: &str) -> io::Result<()> {
//!     // Because io_uring uses asychronous operation it needs access to the
//!     // path for the duration the operation is active. To prevent use-after
//!     // free and similar issues we need ownership of the arguments. In the
//!     // case of opening a file it means we need ownership of the file name.
//!     let filename = PathBuf::from(filename);
//!     // Open a file for reading.
//!     let file = a10::fs::OpenOptions::new().open::<File>(sq.clone(), filename).await?;
//!
//!     // Next we'll read from the from the file.
//!     // Here we need ownership of the buffer, same reason as discussed above.
//!     let buf = file.read(Vec::with_capacity(32 * 1024)).await?;
//!
//!     // Let's write what we read from the file to standard out.
//!     let stdout = a10::io::stdout(sq);
//!     // For writing we also need ownership of the buffer, so we move the
//!     // buffer into function call. However by default we won't get it back,
//!     // to match the API you see in the standard libray.
//!     // But using buffers just once it a bit wasteful, so we can it back
//!     // using the `Extract` trait (the call to `extract`). It changes the
//!     // return values (and `Future` type) to return the buffer and the amount
//!     // of bytes written.
//!     let (buf, n) = stdout.write(buf).extract().await?;
//!
//!     // All done.
//!     Ok(())
//! }
//!
//! /// Block on the `future`, expecting polling `ring` to drive it forward.
//! fn block_on<Fut, T>(ring: &mut Ring, future: Fut) -> Fut::Output
//! where
//!     Fut: Future<Output = io::Result<T>>
//! {
//!     use std::task::{self, RawWaker, RawWakerVTable, Poll};
//!     use std::ptr;
//!
//!     // Pin the future to the stack so we don't move it around.
//!     let mut future = std::pin::pin!(future);
//!
//!     // Create a task context to poll the future work.
//!     let waker = unsafe { task::Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE)) };
//!     let mut ctx = task::Context::from_waker(&waker);
//!
//!     loop {
//!         match future.as_mut().poll(&mut ctx) {
//!             Poll::Ready(result) => return result,
//!             Poll::Pending => {
//!                 // Poll the `Ring` to get an update on the operation(s).
//!                 //
//!                 // In pratice you would first yield to another future, but
//!                 // in this example we don't have one, so we'll always poll
//!                 // the `Ring`.
//!                 ring.poll(None)?;
//!             }
//!         }
//!     }
//!
//!     // A waker implementation that does nothing.
//!     static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
//!         |_| RawWaker::new(ptr::null(), &WAKER_VTABLE),
//!         |_| {},
//!         |_| {},
//!         |_| {},
//!     );
//! }
//! ```

#![cfg_attr(feature = "nightly", feature(async_iterator, io_error_more))]
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

mod bitmap;
mod drop_waker;

mod config;
mod cq;
mod op;
mod sq;
#[cfg_attr(any(target_os = "linux"), path = "io_uring/mod.rs")]
#[cfg_attr(
    any(
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "ios",
        target_os = "macos",
        target_os = "netbsd",
        target_os = "openbsd",
        target_os = "tvos",
        target_os = "visionos",
        target_os = "watchos",
    ),
    path = "kqueue/mod.rs"
)]
mod sys;

#[rustfmt::skip] // This must come before the other modules for the documentation.
pub mod fd;
pub mod io;

#[doc(inline)]
pub use config::Config;
#[doc(no_inline)]
pub use fd::AsyncFd;

use crate::bitmap::AtomicBitMap;

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
    /// For io_uring `entries` is the size of the submission queue (passed to
    /// `io_uring_setup(2)`). It must be a power of two and in the range
    /// 1..=4096.
    ///
    /// For kqueue `entries` is the number of events that can be collected in a
    /// single call to `kevent(2)`.
    ///
    /// # Notes
    ///
    /// A10 uses `IORING_SETUP_SQPOLL` by default for io_uring, which required
    /// Linux kernel 5.11 to work correctly. Furthermore before Linux 5.13 the
    /// user needs the `CAP_SYS_NICE` capability if run as non-root. This can be
    /// disabled by [`Config::with_kernel_thread`].
    pub const fn config<'r>(entries: u32) -> Config<'r> {
        Config::new(entries)
    }

    /// Create a new `Ring` with the default configuration.
    ///
    /// For more configuration options see [`Config`].
    #[doc(alias = "io_uring_setup")]
    #[doc(alias = "kqueue")]
    pub fn new(entries: u32) -> io::Result<Ring> {
        Config::new(entries).build()
    }

    /// Build a new `Ring`.
    fn build(
        submissions: sys::Submissions,
        shared_data: sys::Shared,
        completions: sys::Completions,
        queued_operations: usize,
    ) -> io::Result<Ring> {
        let shared = SharedState::new(submissions, shared_data, queued_operations);
        let sq = SubmissionQueue::new(shared.clone());
        let cq = cq::Queue::new(completions, shared);
        Ok(Ring { sq, cq })
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
        self.inner.wake()
    }

    /// See [`sq::Queue::get_op`].
    pub(crate) unsafe fn get_op(
        &self,
        op_id: OperationId,
    ) -> MutexGuard<
        Option<QueuedOperation<<<<sys::Implementation as Implementation>::Completions as cq::Completions>::Event as cq::Event>::State>>,
    >{
        self.inner.get_op(op_id)
    }

    /// See [`sq::Queue::make_op_available`].
    pub(crate) unsafe fn make_op_available(
        &self,
        op_id: OperationId,
        op: MutexGuard<
        Option<QueuedOperation<<<<sys::Implementation as Implementation>::Completions as cq::Completions>::Event as cq::Event>::State>>,
    >,
    ) {
        self.inner.make_op_available(op_id, op)
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
    /// Bitmap which can be used to create [`OperationIds`], used as index into
    /// `queued_ops`.
    op_ids: Box<AtomicBitMap>,
    /// State of queued operations.
    ///
    /// Indexed by a [`OperationIds`], created by `op_ids`.
    #[rustfmt::skip]
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
}

/// Operation id.
///
/// Used to relate completion events to submission events and operations. Also
/// used as index into [`SharedState::queued_ops`], created by
/// [`SharedState::op_ids`].
// TODO: reduce this to a `u32`. Could shrink some types.
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

#[allow(unused_macros)] // Not used on all OS.
macro_rules! debug_detail {
    (
        // Match a value exactly.
        match $type: ident ($event_type: ty),
        $( $( #[$target: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
        struct $type($event_type);

        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(match self.0 {
                    $(
                    $(#[$target])*
                    #[allow(clippy::bad_bit_mask)] // Apparently some flags are zero.
                    $libc :: $flag => stringify!($flag),
                    )+
                    _ => "<unknown>",
                })
            }
        }
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
use {debug_detail, man_link, syscall};
