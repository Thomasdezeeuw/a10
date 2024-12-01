//! I/O traits.
//!
//! See [`BufMut`] and [`Buf`], and their vectored counterparts [`BufMutSlice`]
//! and [`BufSlice`].

use std::fmt;
use std::mem::MaybeUninit;

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
        self.set_init(n as usize);
    }
}

/// Id for a [`BufPool`].
#[derive(Copy, Clone, Debug)]
pub(crate) struct BufGroupId(pub(crate) u16);

/// Index for a [`BufPool`].
#[derive(Copy, Clone, Debug)]
pub(crate) struct BufId(pub(crate) u16);

/// The implementation for `Vec<u8>` only uses the unused capacity, so any bytes
/// already in the buffer will be untouched.
// SAFETY: `Vec<u8>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes. When the `Vec`tor is leaked the allocation
// will also be leaked.
unsafe impl BufMut for Vec<u8> {
    unsafe fn parts_mut(&mut self) -> (*mut u8, u32) {
        let slice = self.spare_capacity_mut();
        (slice.as_mut_ptr().cast(), slice.len() as u32)
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.set_len(self.len() + n);
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
}

/// Wrapper around [`libc::iovec`] to perform mutable vectored I/O operations,
/// such as write.
pub struct IoMutSlice(crate::sys::io::IoMutSlice);

impl IoMutSlice {
    fn new<B: BufMut>(buf: &mut B) -> IoMutSlice {
        IoMutSlice(crate::sys::io::IoMutSlice::new(buf))
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
        // TODO: replace with `MaybeUninit::uninit_array` once stable.
        // SAFETY: an uninitialised `MaybeUninit` is valid.
        let mut iovecs =
            unsafe { MaybeUninit::<[MaybeUninit<IoMutSlice>; N]>::uninit().assume_init() };
        for (buf, iovec) in self.iter_mut().zip(iovecs.iter_mut()) {
            /* TODO(port).
            debug_assert!(
                buf.buffer_group().is_none(),
                "can't use a10::ReadBuf as a10::BufMutSlice in vectored I/O"
            );
            */
            iovec.write(IoMutSlice::new(buf));
        }
        // TODO: replace with `MaybeUninit::array_assume_init` once stable.
        // SAFETY: `MaybeUninit<IoMutSlice>` and `IoMutSlice` have the same
        // layout as guaranteed by `MaybeUninit`.
        unsafe { std::mem::transmute_copy(&std::mem::ManuallyDrop::new(iovecs)) }
    }

    unsafe fn set_init(&mut self, n: usize) {
        let mut left = n;
        for buf in self {
            let (_, len) = buf.parts_mut();
            let len = len as usize;
            if len < left {
                // Fully initialised the buffer.
                buf.set_init(len);
                left -= len;
            } else {
                // Partially initialised the buffer.
                buf.set_init(left);
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
}

// SAFETY: `Vec<u8>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes. When the `Vec`tor is leaked the allocation
// will also be leaked.
unsafe impl Buf for Vec<u8> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let slice = self.as_slice();
        (slice.as_ptr().cast(), slice.len() as u32)
    }
}

// SAFETY: `Box<[u8]>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes. When the `Box` is leaked the allocation will
// also be leaked.
unsafe impl Buf for Box<[u8]> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_ptr().cast(), self.len() as u32)
    }
}

// SAFETY: `String` is just a `Vec<u8>`, see it's implementation for the safety
// reasoning.
unsafe impl Buf for String {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let slice = self.as_bytes();
        (slice.as_ptr().cast(), slice.len() as u32)
    }
}

// SAFETY: because the reference has a `'static` lifetime we know the bytes
// can't be deallocated, so it's safe to implement `Buf`.
unsafe impl Buf for &'static [u8] {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_ptr(), self.len() as u32)
    }
}

// SAFETY: because the reference has a `'static` lifetime we know the bytes
// can't be deallocated, so it's safe to implement `Buf`.
unsafe impl Buf for &'static str {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_bytes().as_ptr(), self.len() as u32)
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
}

/// Wrapper around [`libc::iovec`] to perform immutable vectored I/O operations,
/// such as read.
pub struct IoSlice(crate::sys::io::IoSlice);

impl IoSlice {
    fn new<B: Buf>(buf: &B) -> IoSlice {
        IoSlice(crate::sys::io::IoSlice::new(buf))
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
        // TODO: replace with `MaybeUninit::uninit_array` once stable.
        // SAFETY: an uninitialised `MaybeUninit` is valid.
        let mut iovecs =
            unsafe { MaybeUninit::<[MaybeUninit<IoSlice>; N]>::uninit().assume_init() };
        for (buf, iovec) in self.iter().zip(iovecs.iter_mut()) {
            iovec.write(IoSlice::new(buf));
        }
        // TODO: replace with `MaybeUninit::array_assume_init` once stable.
        // SAFETY: `MaybeUninit<IoSlice>` and `IoSlice` have the same layout as
        // guaranteed by `MaybeUninit`.
        unsafe { std::mem::transmute_copy(&std::mem::ManuallyDrop::new(iovecs)) }
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
                        /* TODO(port).
                        debug_assert!(
                            self.$index.buffer_group().is_none(),
                            "can't use a10::ReadBuf as a10::BufMutSlice in vectored I/O"
                        );
                        */
                        IoMutSlice::new(&mut self.$index)
                    }),+
                ]
            }

            unsafe fn set_init(&mut self, n: usize) {
                let mut left = n;
                $({
                    let (_, len) = self.$index.parts_mut();
                    let len = len as usize;
                    if len < left {
                        // Fully initialised the buffer.
                        self.$index.set_init(len);
                        left -= len;
                    } else {
                        // Partially initialised the buffer.
                        self.$index.set_init(left);
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
                        IoSlice::new(&self.$index)
                    }),+
                ]
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
