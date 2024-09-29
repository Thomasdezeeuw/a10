//! Extraction of input arguments.
//!
//! See the [`Extract`] trait for more information.

use crate::io_uring::cancel::{Cancel, CancelOp, CancelResult};

/// Extract input arguments from operations.
///
/// Because of the way that io_uring works the kernel needs mutable access to
/// the inputs of a system call for entire duration the operation is in
/// progress. For example when reading into a buffer the buffer needs to stay
/// alive until the kernel has written into it, or until the kernel knows the
/// operation is canceled and won't write into the buffer any more. If we can't
/// ensure this the kernel might write into memory we don't own causing
/// write-after-free bugs.
///
/// To ensure the input stays alive A10 needs ownership of the input arguments
/// and delays the deallocation of the inputs when a [`Future`] operation is
/// dropped before completion. However to give the `Future`s a nice API we don't
/// return the input arguments and try to match the APIs that don't take
/// ownership of arguments, e.g [`fs::OpenOptions::open`] just returns a
/// [`AsyncFd`] not the path argument.
///
/// In some cases though we would like to get back the input arguments from the
/// operation, e.g. for performance reasons. This trait allow you to do just
/// that: extract the input arguments from `Future` operations.
///
/// All I/O operations, that is the `Future` implementations, that support
/// extract the input argument will implement this `Extract` trait. A list of
/// the supported operations can be found [below]. For the actual
/// implementations see the `Future` implementations on the [`Extractor`] type.
///
/// [`Future`]: std::future::Future
/// [`fs::OpenOptions::open`]: crate::fs::OpenOptions::open
/// [`AsyncFd`]: crate::AsyncFd
/// [below]: #implementors
///
/// # Examples
///
/// Extracting the arguments from [`fs::OpenOptions::open`] and
/// [`AsyncFd::write`] operations.
///
/// [`AsyncFd::write`]: crate::AsyncFd::write
///
/// ```
/// use std::io;
/// use std::path::PathBuf;
///
/// use a10::fd::File;
/// use a10::fs::OpenOptions;
/// use a10::{SubmissionQueue, Extract};
///
/// async fn write_all_to_file(sq: SubmissionQueue, path: PathBuf, buf: Vec<u8>) -> io::Result<(PathBuf, Vec<u8>)> {
///     // This `Future` returns just the opened file.
///     let open_file_future = OpenOptions::new().open::<File>(sq, path);
///     // Calling `extract` and awaiting that will return both the file and
///     // the path buffer.
///     let (file, path) = open_file_future.extract().await?;
///
///     // This just returns the number of bytes written.
///     let write_future = file.write(buf);
///     // When extracting it also returns the buffer.
///     let (buf, n) = write_future.extract().await?;
///
///     if n != buf.len() {
///         // NOTE: `WriteZero` is not entirely accurate, but just for the sake
///         // of the example.
///         return Err(io::Error::new(io::ErrorKind::WriteZero, "didn't write entire buffer"));
///     }
///
///     Ok((path, buf))
/// }
/// ```
pub trait Extract {
    /// Returns a [`Future`] that returns the input arguments in addition to the
    /// regular return value(s).
    ///
    /// [`Future`]: std::future::Future
    fn extract(self) -> Extractor<Self>
    where
        Self: Sized,
    {
        Extractor { fut: self }
    }
}

/// [`Future`] wrapper to extract input arguments from a `Future` operation.
///
/// See the [`Extract`] trait for more information.
///
/// [`Future`]: std::future::Future
#[must_use = "`Future`s do nothing unless polled"]
#[derive(Debug)]
pub struct Extractor<Fut> {
    pub(crate) fut: Fut,
}

impl<Fut: Cancel> Cancel for Extractor<Fut> {
    fn try_cancel(&mut self) -> CancelResult {
        self.fut.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
        self.fut.cancel()
    }
}
