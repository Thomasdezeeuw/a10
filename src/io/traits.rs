//! I/O traits.
//!
//! See [`BufMut`] and [`Buf`], and their vectored counterparts [`BufMutSlice`]
//! and [`BufSlice`].

use std::borrow::Cow;
use std::cmp::min;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::{fmt, slice};

/// Trait that defines the behaviour of buffers used in reading, which requires
/// mutable access.
///
/// # Safety
///
/// Unlike normal buffers the buffer implementations for A10 have additional
/// requirements.
///
/// If the operation (that uses this buffer) is not polled to completion, i.e.
/// the `Future` is dropped before it returns `Poll::Ready`, the kernel still
/// has access to the buffer and will still attempt to write into it. This means
/// that we must delay deallocation in such a way that the kernel will not write
/// into memory we don't have access to any more. This makes, for example, stack
/// based buffers unfit to implement `BufMut`. Because we can't delay the
/// deallocation once its dropped and the kernel will overwrite part of your
/// stack (where the buffer used to be)!
pub unsafe trait BufMut: 'static {
    /// Returns the writable buffer as pointer and length parts.
    ///
    /// # Safety
    ///
    /// Only initialised bytes may be written to the pointer returned. The
    /// pointer *may* point to uninitialised bytes, so reading from the pointer
    /// is UB.
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointing to memory owned by the buffer. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the buffer.
    ///
    /// Note that the above requirements are only required for implementations
    /// outside of A10. **This trait is unfit for external use!**
    ///
    /// # Why not a slice?
    ///
    /// Returning a slice `&[u8]` would prevent us to use unitialised bytes,
    /// meaning we have to zero the buffer before usage, not ideal for
    /// performance. So, naturally you would suggest `&[MaybeUninit<u8>]`,
    /// however that would prevent buffer types with only initialised bytes.
    /// Returning a slice with `MaybeUninit` to such as type would be unsound as
    /// it would allow the caller to write unitialised bytes without using
    /// `unsafe`.
    ///
    /// # Notes
    ///
    /// Most Rust API use a `usize` for length, but io_uring uses `u32`, hence
    /// we do also.
    unsafe fn parts_mut(&mut self) -> (*mut u8, u32);

    /// Mark `n` bytes as initialised.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `n` bytes, starting at the pointer returned
    /// by [`BufMut::parts_mut`], are initialised.
    unsafe fn set_init(&mut self, n: usize);

    /// Buffer group id, or `None` if it's not part of a buffer pool.
    ///
    /// Don't implement this.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    fn buffer_group(&self) -> Option<BufGroupId> {
        None
    }

    /// Mark `n` bytes as initialised in buffer with `idx`.
    ///
    /// Don't implement this.
    #[doc(hidden)]
    #[allow(private_interfaces)]
    unsafe fn buffer_init(&mut self, id: BufId, n: u32) {
        debug_assert!(id.0 == 0);
        unsafe { self.set_init(n as usize) };
    }

    /// Extend the buffer with `bytes`, returns the number of bytes copied.
    ///
    /// # Examples
    ///
    /// ```
    /// # use a10::io::BufMut;
    /// let mut buf = Vec::with_capacity(10);
    /// assert_eq!(write_hello(&mut buf), 5);
    /// assert_eq!(buf, b"hello");
    ///
    /// fn write_hello<B: BufMut>(buf: &mut B) -> usize {
    ///   buf.extend_from_slice(b"hello")
    /// }
    /// ```
    ///
    /// Short write.
    ///
    /// ```
    /// # use a10::io::BufMut;
    /// let mut buf = Vec::with_capacity(2);
    /// assert_eq!(write_hello(&mut buf), 2);
    /// assert_eq!(buf, b"he");
    ///
    /// fn write_hello<B: BufMut>(buf: &mut B) -> usize {
    ///   buf.extend_from_slice(b"hello")
    /// }
    /// ```
    fn extend_from_slice(&mut self, bytes: &[u8]) -> usize {
        let (ptr, capacity) = unsafe { self.parts_mut() };
        // SAFETY: `parts_mut` requirements are the same for `copy_bytes`.
        let written = unsafe { copy_bytes(ptr, capacity as usize, bytes) };
        // SAFETY: just written the bytes in the call above.
        unsafe { self.set_init(written) };
        written
    }
}

/// Copies bytes from `src` to `dst`, copies up to `min(dst_len, src.len())`,
/// i.e. it won't write beyond `dst` or read beyond `src` bounds. Returns the
/// number of bytes copied.
///
/// # Safety
///
/// Caller must ensure that `dst` and `dst_len` are valid for writing.
unsafe fn copy_bytes(dst: *mut u8, dst_len: usize, src: &[u8]) -> usize {
    let len = min(src.len(), dst_len);
    // SAFETY: since we have mutable access to `self` we know that `bytes`
    // can point to (part of) the same buffer as that would be UB already.
    // Furthermore we checked that the length doesn't overrun the buffer and
    // `parts_mut` impl must ensure that the `ptr` is valid.
    unsafe { dst.copy_from_nonoverlapping(src.as_ptr(), len) };
    len
}

/// Id for a [`ReadBufPool`].
///
/// [`ReadBufPool`]: crate::io::ReadBufPool
#[derive(Copy, Clone, Debug)]
pub struct BufGroupId(#[allow(dead_code)] pub(crate) u16);

/// Index for a [`ReadBufPool`].
///
/// [`ReadBufPool`]: crate::io::ReadBufPool
#[derive(Copy, Clone, Debug)]
pub struct BufId(#[allow(dead_code)] pub(crate) u16);

/// The implementation for `Vec<u8>` only uses the uninitialised capacity of the
/// vector. In other words the bytes currently in the vector remain untouched.
///
/// # Examples
///
/// The following example shows that the bytes already in the vector remain
/// untouched.
///
/// ```
/// use a10::io::BufMut;
///
/// let mut buf = Vec::with_capacity(100);
/// buf.extend(b"Hello world!");
///
/// write_bytes(b" Hello mars!", &mut buf);
///
/// assert_eq!(&*buf, b"Hello world! Hello mars!");
///
/// fn write_bytes<B: BufMut>(src: &[u8], buf: &mut B) {
///     // Writes `src` to `buf`.
/// #   let (dst, len) = unsafe { buf.parts_mut() };
/// #   let len = std::cmp::min(src.len(), len as usize);
/// #   // SAFETY: both the src and dst pointers are good. And we've ensured
/// #   // that the length is correct, not overwriting data we don't own or
/// #   // reading data we don't own.
/// #   unsafe {
/// #       std::ptr::copy_nonoverlapping(src.as_ptr(), dst, len as usize);
/// #       buf.set_init(len);
/// #   }
/// }
/// ```
// SAFETY: `Vec<u8>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes. When the `Vec`tor is leaked the allocation
// will also be leaked.
unsafe impl BufMut for Vec<u8> {
    unsafe fn parts_mut(&mut self) -> (*mut u8, u32) {
        let slice = self.spare_capacity_mut();
        (slice.as_mut_ptr().cast(), slice.len() as u32)
    }

    unsafe fn set_init(&mut self, n: usize) {
        unsafe { self.set_len(self.len() + n) };
    }
}

/// Trait that defines the behaviour of buffers used in reading using vectored
/// I/O, which requires mutable access.
///
/// # Safety
///
/// This has the same safety requirements as [`BufMut`], but then for all
/// buffers used.
pub unsafe trait BufMutSlice<const N: usize>: 'static {
    /// Returns the writable buffers as `IoMutSlice` structures.
    ///
    /// # Safety
    ///
    /// This has the same safety requirements as [`BufMut::parts_mut`], but then
    /// for all buffers used.
    unsafe fn as_iovecs_mut(&mut self) -> [IoMutSlice; N];

    /// Mark `n` bytes as initialised.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `n` bytes are initialised in the vectors
    /// return by [`BufMutSlice::as_iovecs_mut`].
    ///
    /// The implementation must ensure that that proper buffer(s) are
    /// initialised. For example when this is called with `n = 10` with two
    /// buffers of size `8` the implementation should initialise the first
    /// buffer with `n = 8` and the second with `n = 10 - 8 = 2`.
    unsafe fn set_init(&mut self, n: usize);

    /// Extend the buffer with `bytes`, returns the total number of bytes
    /// copied.
    ///
    /// # Examples
    ///
    /// ```
    /// # use a10::io::BufMutSlice;
    /// let mut bufs = (Vec::with_capacity(10), Vec::with_capacity(10));
    /// assert_eq!(write_hello(&mut bufs), 5);
    /// assert_eq!(bufs.0, b"hello");
    /// assert_eq!(bufs.1, b"");
    ///
    /// fn write_hello<B: BufMutSlice<N>, const N: usize>(bufs: &mut B) -> usize {
    ///   bufs.extend_from_slice(b"hello")
    /// }
    /// ```
    ///
    /// Split over multiple buffers.
    ///
    /// ```
    /// # use a10::io::BufMutSlice;
    /// let mut bufs = (Vec::with_capacity(2), Vec::with_capacity(10));
    /// assert_eq!(write_hello(&mut bufs), 5);
    /// assert_eq!(bufs.0, b"he");
    /// assert_eq!(bufs.1, b"llo");
    ///
    /// fn write_hello<B: BufMutSlice<N>, const N: usize>(bufs: &mut B) -> usize {
    ///   bufs.extend_from_slice(b"hello")
    /// }
    /// ```
    fn extend_from_slice(&mut self, bytes: &[u8]) -> usize {
        let mut left = bytes;
        for mut iovec in unsafe { self.as_iovecs_mut() } {
            let (ptr, capacity) = unsafe { iovec.0.parts_mut() };
            // SAFETY: `as_iovecs_mut` requirements are the same for `copy_bytes`.
            let n = unsafe { copy_bytes(ptr, capacity, left) };
            left = &left[n..];
            if left.is_empty() {
                break;
            }
        }
        let written = bytes.len() - left.len();
        // SAFETY: just written the bytes above.
        unsafe { self.set_init(written) };
        written
    }
}

/// Wrapper around [`libc::iovec`] to perform mutable vectored I/O operations,
/// such as write.
pub struct IoMutSlice(crate::sys::io::IoMutSlice);

impl IoMutSlice {
    /// Create a new `IoMutSlice` from `buf`.
    ///
    /// # Safety
    ///
    /// Caller must ensure that `buf` outlives the returned `IoMutSlice`.
    pub(crate) unsafe fn new<B: BufMut>(buf: &mut B) -> IoMutSlice {
        IoMutSlice(crate::sys::io::IoMutSlice::new(buf))
    }

    #[doc(hidden)] // Used by testing.
    #[allow(clippy::len_without_is_empty)]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    #[doc(hidden)] // Used by testing.
    pub unsafe fn set_len(&mut self, new_len: usize) {
        self.0.set_len(new_len);
    }

    pub(crate) unsafe fn ptr(&self) -> *const u8 {
        self.0.ptr()
    }
}

impl std::fmt::Debug for IoMutSlice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IoMutSlice")
            .field("len", &self.0.len())
            .finish()
    }
}

// SAFETY: `BufMutSlice` has the same safety requirements as `BufMut` and since
// `B` implements `BufMut` it's safe to implement `BufMutSlice` for an array of
// `B`.
unsafe impl<B: BufMut, const N: usize> BufMutSlice<N> for [B; N] {
    unsafe fn as_iovecs_mut(&mut self) -> [IoMutSlice; N] {
        let mut iovecs = [const { MaybeUninit::uninit() }; N];
        for (buf, iovec) in self.iter_mut().zip(iovecs.iter_mut()) {
            debug_assert!(
                buf.buffer_group().is_none(),
                "can't use a10::ReadBuf as a10::BufMutSlice in vectored I/O",
            );
            iovec.write(unsafe { IoMutSlice::new(buf) });
        }
        // SAFETY: `MaybeUninit<IoMutSlice>` and `IoMutSlice` have the same
        // layout as guaranteed by `MaybeUninit`.
        unsafe { std::mem::transmute_copy(&std::mem::ManuallyDrop::new(iovecs)) }
    }

    unsafe fn set_init(&mut self, n: usize) {
        let mut left = n;
        for buf in self {
            let (_, len) = unsafe { buf.parts_mut() };
            let len = len as usize;
            if len < left {
                // Fully initialised the buffer.
                unsafe { buf.set_init(len) };
                left -= len;
            } else {
                // Partially initialised the buffer.
                unsafe { buf.set_init(left) };
                return;
            }
        }
        unreachable!(
            "called BufMutSlice::set_init({n}), with buffers totaling in {} in size",
            n - left
        );
    }
}

// NOTE: Also see implementation of `BufMutSlice` for tuples in the macro
// `buf_slice_for_tuple` below.

/// Trait that defines the behaviour of buffers used in writing, which requires
/// read only access.
///
/// # Safety
///
/// Unlike normal buffers the buffer implementations for A10 have additional
/// requirements.
///
/// If the operation (that uses this buffer) is not polled to completion, i.e.
/// the `Future` is dropped before it returns `Poll::Ready`, the kernel still
/// has access to the buffer and will still attempt to read from it. This means
/// that we must delay deallocation in such a way that the kernel will not read
/// memory we don't have access to any more. This makes, for example, stack
/// based buffers unfit to implement `Buf`.  Because we can't delay the
/// deallocation once its dropped and the kernel will read part of your stack
/// (where the buffer used to be)! This would be a huge security risk.
pub unsafe trait Buf: 'static {
    /// Returns the reabable buffer as pointer and length parts.
    ///
    /// # Safety
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointing to memory owned by the buffer. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the buffer.
    ///
    /// # Notes
    ///
    /// Most Rust API use a `usize` for length, but io_uring uses `u32`, hence
    /// we do also.
    unsafe fn parts(&self) -> (*const u8, u32);

    /// Length of the buffer in bytes.
    ///
    /// # Implementation
    ///
    /// This calls [`Buf::parts`] and returns the second part.
    fn len(&self) -> usize {
        // SAFETY: not using the pointer. The implementation of `Buf::parts`
        // must ensure the length is correct.
        unsafe { self.parts() }.1 as usize
    }

    /// Returns true if the buffer is empty.
    ///
    /// # Implementation
    ///
    /// This calls [`Buf::len`] and compares it to zero.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns itself as slice of bytes.
    ///
    /// # Implementation
    ///
    /// This calls [`Buf::parts`] and converts that into a slice.
    fn as_slice(&self) -> &[u8] {
        // SAFETY: the `Buf::parts` implementation ensures that the `ptr` and
        // `len` are valid, as well as the memory allocation they point to. So
        // creating a slice from it is safe.
        unsafe {
            let (ptr, len) = self.parts();
            slice::from_raw_parts(ptr, len as usize)
        }
    }
}

// SAFETY: `Vec<u8>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes. When the `Vec`tor is leaked the allocation
// will also be leaked.
unsafe impl Buf for Vec<u8> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let slice = self.as_slice();
        (slice.as_ptr().cast(), slice.len() as u32)
    }

    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn is_empty(&self) -> bool {
        Vec::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self
    }
}

// SAFETY: `Box<[u8]>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes. When the `Box` is leaked the allocation will
// also be leaked.
unsafe impl Buf for Box<[u8]> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_ptr().cast(), self.len() as u32)
    }

    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn is_empty(&self) -> bool {
        <[u8]>::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self
    }
}

// SAFETY: `String` is just a `Vec<u8>`, see it's implementation for the safety
// reasoning.
unsafe impl Buf for String {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let slice = self.as_bytes();
        (slice.as_ptr().cast(), slice.len() as u32)
    }

    fn len(&self) -> usize {
        String::len(self)
    }

    fn is_empty(&self) -> bool {
        String::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

// SAFETY: `Box<str>` is the same as `Box<[u8]>`, see it's implementation for the safety
// reasoning.
unsafe impl Buf for Box<str> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let bytes = self.as_bytes();
        (bytes.as_ptr().cast(), bytes.len() as u32)
    }

    fn len(&self) -> usize {
        str::len(self)
    }

    fn is_empty(&self) -> bool {
        str::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// If rustc complains about "implementation of `Buf` is not general enough",
/// try [`StaticBuf`].
// SAFETY: because the reference has a `'static` lifetime we know the bytes
// can't be deallocated, so it's safe to implement `Buf`.
unsafe impl Buf for &'static [u8] {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_ptr(), self.len() as u32)
    }

    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn is_empty(&self) -> bool {
        <[u8]>::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self
    }
}

/// If rustc complains about "implementation of `Buf` is not general enough",
/// try [`StaticBuf`].
// SAFETY: because the reference has a `'static` lifetime we know the bytes
// can't be deallocated, so it's safe to implement `Buf`.
unsafe impl Buf for &'static str {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_bytes().as_ptr(), self.len() as u32)
    }

    fn len(&self) -> usize {
        str::len(self)
    }

    fn is_empty(&self) -> bool {
        str::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

// SAFETY: this is either a `Vec<u8>` or `&'static [u8]`, both have
// implementations of `Buf`.
unsafe impl Buf for Cow<'static, [u8]> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_ptr(), self.len() as u32)
    }

    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn is_empty(&self) -> bool {
        <[u8]>::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self
    }
}

// SAFETY: this is either a `String` or `&'static str`, both have
// implementations of `Buf`.
unsafe impl Buf for Cow<'static, str> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_bytes().as_ptr(), self.len() as u32)
    }

    fn len(&self) -> usize {
        str::len(self)
    }

    fn is_empty(&self) -> bool {
        str::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

// SAFETY: `Arc<[u8]>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes.
unsafe impl Buf for Arc<[u8]> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_ptr().cast(), self.len() as u32)
    }

    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn is_empty(&self) -> bool {
        <[u8]>::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self
    }
}

// SAFETY: `Arc<str>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes.
unsafe impl Buf for Arc<str> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let bytes = self.as_bytes();
        (bytes.as_ptr().cast(), bytes.len() as u32)
    }

    fn len(&self) -> usize {
        str::len(self)
    }

    fn is_empty(&self) -> bool {
        str::is_empty(self)
    }

    fn as_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Trait that defines the behaviour of buffers used in writing using vectored
/// I/O, which requires read only access.
///
/// # Safety
///
/// This has the same safety requirements as [`Buf`], but then for all buffers
/// used.
pub unsafe trait BufSlice<const N: usize>: 'static {
    /// Returns the reabable buffer as `IoSlice` structures.
    ///
    /// # Safety
    ///
    /// This has the same safety requirements as [`Buf::parts`], but then for
    /// all buffers used.
    unsafe fn as_iovecs(&self) -> [IoSlice; N];

    /// Returns the total length of all buffers in bytes.
    fn total_len(&self) -> usize {
        // SAFETY: `as_iovecs` requires the returned iovec to be valid.
        unsafe { self.as_iovecs().iter().map(IoSlice::len).sum() }
    }
}

/// Wrapper around [`libc::iovec`] to perform immutable vectored I/O operations,
/// such as read.
pub struct IoSlice(crate::sys::io::IoSlice);

impl IoSlice {
    /// Create a new `IoSlice` from `buf`.
    ///
    /// # Safety
    ///
    /// Caller must ensure that `buf` outlives the returned `IoSlice`.
    #[doc(hidden)] // Used in testing.
    pub unsafe fn new<B: Buf>(buf: &B) -> IoSlice {
        IoSlice(crate::sys::io::IoSlice::new(buf))
    }

    pub(crate) const fn len(&self) -> usize {
        self.0.len()
    }

    pub(crate) unsafe fn set_len(&mut self, new_len: usize) {
        self.0.set_len(new_len);
    }

    pub(crate) unsafe fn ptr(&self) -> *const u8 {
        self.0.ptr()
    }
}

impl std::fmt::Debug for IoSlice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_bytes().fmt(f)
    }
}

// SAFETY: `BufSlice` has the same safety requirements as `Buf` and since `B`
// implements `Buf` it's safe to implement `BufSlice` for an array of `B`.
unsafe impl<B: Buf, const N: usize> BufSlice<N> for [B; N] {
    unsafe fn as_iovecs(&self) -> [IoSlice; N] {
        let mut iovecs = [const { MaybeUninit::uninit() }; N];
        for (buf, iovec) in self.iter().zip(iovecs.iter_mut()) {
            iovec.write(unsafe { IoSlice::new(buf) });
        }
        // SAFETY: `MaybeUninit<IoSlice>` and `IoSlice` have the same layout as
        // guaranteed by `MaybeUninit`.
        unsafe { std::mem::transmute_copy(&std::mem::ManuallyDrop::new(iovecs)) }
    }

    fn total_len(&self) -> usize {
        self.iter().map(Buf::len).sum()
    }
}

macro_rules! buf_slice_for_tuple {
    (
        // Number of values.
        $N: expr,
        // Generic parameter name and tuple index.
        $( $generic: ident . $index: tt ),+
    ) => {
        // SAFETY: `BufMutSlice` has the same safety requirements as `BufMut`
        // and since all generic buffers must implement `BufMut` it's safe to
        // implement `BufMutSlice` for a tuple of all those buffers.
        unsafe impl<$( $generic: BufMut ),+> BufMutSlice<$N> for ($( $generic ),+) {
            unsafe fn as_iovecs_mut(&mut self) -> [IoMutSlice; $N] {
                [
                    $({
                        debug_assert!(
                            self.$index.buffer_group().is_none(),
                            "can't use a10::ReadBuf as a10::BufMutSlice in vectored I/O"
                        );
                        unsafe { IoMutSlice::new(&mut self.$index) }
                    }),+
                ]
            }

            unsafe fn set_init(&mut self, n: usize) {
                let mut left = n;
                $({
                    let (_, len) = unsafe { self.$index.parts_mut() };
                    let len = len as usize;
                    if len < left {
                        // Fully initialised the buffer.
                        unsafe { self.$index.set_init(len) };
                        left -= len;
                    } else {
                        // Partially initialised the buffer.
                        unsafe { self.$index.set_init(left) };
                        return;
                    }
                })+
                unreachable!(
                    "called BufMutSlice::set_init({n}), with buffers totaling in {} in size",
                    n - left
                );
            }
        }

        // SAFETY: `BufSlice` has the same safety requirements as `Buf` and
        // since all generic buffers must implement `Buf` it's safe to implement
        // `BufSlice` for a tuple of all those buffers.
        unsafe impl<$( $generic: Buf ),+> BufSlice<$N> for ($( $generic ),+) {
            unsafe fn as_iovecs(&self) -> [IoSlice; $N] {
                [
                    $({
                        unsafe { IoSlice::new(&self.$index) }
                    }),+
                ]
            }

            fn total_len(&self) -> usize {
                0
                $( + self.$index.len() )+
            }
        }
    };
}

buf_slice_for_tuple!(2, A.0, B.1);
buf_slice_for_tuple!(3, A.0, B.1, C.2);
buf_slice_for_tuple!(4, A.0, B.1, C.2, D.3);
buf_slice_for_tuple!(5, A.0, B.1, C.2, D.3, E.4);
buf_slice_for_tuple!(6, A.0, B.1, C.2, D.3, E.4, F.5);
buf_slice_for_tuple!(7, A.0, B.1, C.2, D.3, E.4, F.5, G.6);
buf_slice_for_tuple!(8, A.0, B.1, C.2, D.3, E.4, F.5, G.6, I.7);

/// Buffer using static data.
///
/// Wrapper around types such as `&'static [u8]` and `&'static str`.
///
/// This type really shouldn't exist. This only exists to work around
/// "implementation of `Buf` is not general enough" errors. Where rustc
/// complains that "`&'0 T` must implement `Buf`, for any lifetime `'0`, but
/// `Buf` is actually implemented for the type `&'static T`".
#[derive(Copy, Clone, Debug)]
pub struct StaticBuf(&'static [u8]);

impl From<&'static [u8]> for StaticBuf {
    fn from(buf: &'static [u8]) -> StaticBuf {
        StaticBuf(buf)
    }
}

impl From<&'static str> for StaticBuf {
    fn from(buf: &'static str) -> StaticBuf {
        StaticBuf(buf.as_bytes())
    }
}

// SAFETY: See `Buf` implementation for `&'static [u8]`.
unsafe impl Buf for StaticBuf {
    unsafe fn parts(&self) -> (*const u8, u32) {
        unsafe { self.0.parts() }
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn as_slice(&self) -> &[u8] {
        Buf::as_slice(&self.0)
    }
}
