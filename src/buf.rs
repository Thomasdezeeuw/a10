//! Module with the [`ReadBuf`] and [`WriteBuf`] traits.

/// Trait that defines the behaviour of buffers used in reading.
pub trait ReadBuf: 'static {
    /// Returns the writable buffer as pointer and length.
    ///
    /// # Safety
    ///
    /// Only initialised bytes may be written to the pointer returned. The
    /// pointer *may* point to uninitialised bytes, so reading from the pointer
    /// is UB.
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointign to memoty owned by the buffer. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the buffer.
    ///
    /// # Why not a slice?
    ///
    /// Returning a slice `&[u8]` would prevent us to use unitialised bytes,
    /// meaning we have to zero the buffer before usage, not ideal for
    /// performance. So, naturally you would suggest `&[MaybeUninit<u8>]`,
    /// however that would prevent buffer types with only initialised bytes,
    /// such as `[0u8; 4096]`. Returning a slice with `MaybeUninit` to such as
    /// type would be unsound as it would allow the caller to write unitialised
    /// bytes without using `unsafe`.
    ///
    /// # Notes
    ///
    /// Although a `usize` is used in the function signature io_uring actually
    /// uses a `u32` (similar to `iovec`), so the length will truncated.
    unsafe fn as_ptr(&mut self) -> (*mut u8, usize);

    /// Mark `n` bytes as initialised.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `n` bytes, starting at the pointer returned
    /// by [`ReadBuf::as_ptr`], are initialised.
    unsafe fn set_init(&mut self, n: usize);
}

/// The implementation for `Vec<u8>` only uses the unused capacity, so any bytes
/// already in the buffer will be untouched.
impl ReadBuf for Vec<u8> {
    unsafe fn as_ptr(&mut self) -> (*mut u8, usize) {
        let slice = self.spare_capacity_mut();
        (slice.as_mut_ptr().cast(), slice.len())
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.set_len(self.len() + n);
    }
}

/// Trait that defines the behaviour of buffers used in writing.
pub trait WriteBuf: 'static {
    /// Returns the bytes to write.
    unsafe fn as_slice(&self) -> &[u8];
}

impl WriteBuf for Vec<u8> {
    unsafe fn as_slice(&self) -> &[u8] {
        self.as_slice()
    }
}

impl<const N: usize> WriteBuf for [u8; N] {
    unsafe fn as_slice(&self) -> &[u8] {
        self.as_slice()
    }
}
