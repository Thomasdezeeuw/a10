//! [`Extract`] trait.

/// Extract input arguments from operations.
///
/// Because of the way that iouring works the kernel needs mutable access to the
/// input to a systemcall for entire duration the operation is in progress. For
/// example when reading into a buffer the buffer needs to stay alive until the
/// kernel has written into it, or until the kernel knows the operation is
/// canceled and won't write into the buffer anymore. If we can't ensure this
/// the kernel might write into memory we don't own causing write-after-free
/// bugs.
///
/// To ensure the input stays alive A10 needs ownership of the input arguments
/// and leaks the inputs when a [`Future`] operation is dropped before
/// completion. However to give the `Future`s a nice API we don't return the
/// input arguments and try to match the APIs that don't take ownership of
/// arguments, e.g [`fs::File::open`] just returns a [`File`] not the path
/// argument.
///
/// In some cases though we would like to get back the input arguments from the
/// operation, e.g. for performance reasons. This trait allow you to do just
/// that: extract the input arguments from `Future` operations.
///
/// See the list of [supported operations below].
///
/// [`Future`]: std::future::Future
/// [`fs::File::open`]: crate::fs::File::open
/// [`File`]: crate::fs::File
/// [supported operations below]: #implementors
///
/// # Examples
///
/// Extracting the argument from [`fs::File::open`] and [`fs::File::write`]
/// operations.
///
/// ```
/// use std::io;
/// use std::path::PathBuf;
///
/// use a10::fs::File;
/// use a10::{SubmissionQueue, Extract};
///
/// async fn write_all_to_file(sq: SubmissionQueue, path: PathBuf, buf: Vec<u8>) -> io::Result<(PathBuf, Vec<u8>)> {
///     // This `Future` returns just the opened file.
///     let open_file_future = File::open(sq, path)?;
///     // Calling `extract` and awaiting that will return both the file and the
///     // path.
///     let (file, path) = open_file_future.extract().await?;
///
///     // This just returned the number of bytes written.
///     let write_future = file.write(buf)?;
///     // When extracing it also returns the buffer.
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
///
/// [`fs::File::write`]: crate::fs::File::write
pub trait Extract {
    /// Extract input arguments from the future.
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
pub struct Extractor<Fut> {
    pub(crate) fut: Fut,
}
