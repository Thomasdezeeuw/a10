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
/// input arguments and try to match the API that don't take ownership of
/// argument, e.g [`fs::File::open`] just returns a [`File`] not the path
/// argument.
///
/// In some cases though we would like to get back the input arguments from the
/// operation, e.g. for performance reasons. This trait allow you to do just
/// that: extract the input arguments from `Future` operations.
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
pub struct Extractor<Fut> {
    pub(crate) fut: Fut,
}
