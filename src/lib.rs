#![allow(unused_macros, unused_imports, dead_code)] // FIXME: remove.

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

use std::time::Duration;

mod bitmap;
mod drop_waker;

#[cfg(target_os = "linux")]
mod io_uring;

#[rustfmt::skip] // This must come before the other modules for the documentation.
pub mod fd;
pub mod io;

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
    // TODO(port).
}

impl Ring {
    /// Returns the `SubmissionQueue` used by this ring.
    ///
    /// The `SubmissionQueue` can be used to queue asynchronous I/O operations.
    pub const fn submission_queue(&self) -> &SubmissionQueue {
        // TODO(port).
        todo!()
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
    pub fn poll(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        // TODO(port).
        _ = timeout;
        todo!("Ring::poll")
    }
}

/// Queue to submit asynchronous operations to.
///
/// This type doesn't have many public methods, but is used by all I/O types, to
/// queue asynchronous operations. The queue can be acquired by using
/// [`Ring::submission_queue`].
///
/// The submission queue can be shared by cloning it, it's a cheap operation.
#[derive(Clone, Debug)]
pub struct SubmissionQueue {
    // TODO(port).
}

impl SubmissionQueue {
    /// Wake the connected [`Ring`].
    ///
    /// All this does is interrupt a call to [`Ring::poll`].
    pub fn wake(&self) {
        // TODO(port).
        todo!("SubmissionQueue::wake")
    }
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
        let res = unsafe { libc::$fn($( $arg, )*) };
        if res == -1 {
            ::std::result::Result::Err(::std::io::Error::last_os_error())
        } else {
            ::std::result::Result::Ok(res)
        }
    }};
}

use {man_link, syscall};
