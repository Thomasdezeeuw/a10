//! Type definitions for I/O functionality.

use std::cell::UnsafeCell;
use std::future::Future;
use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io, ptr, result, slice};

use crate::op::{op_future, SharedOperationState, NO_OFFSET};
use crate::{AsyncFd, QueueFull};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

/// I/O system calls.
impl AsyncFd {
    /// Read from this fd into `buf`.
    ///
    /// # Notes
    ///
    /// This leave the current contents of `buf` untouched and only uses the
    /// spare capacity.
    pub fn read<'fd, B>(&'fd self, buf: B) -> result::Result<Read<'fd, B>, QueueFull>
    where
        B: ReadBuf,
    {
        self.read_at(buf, NO_OFFSET)
    }

    /// Read from this fd into `buf` starting at `offset`.
    ///
    /// The current file cursor is not affected by this function. This means
    /// that a call `read_at(buf, 1024)` with a buffer of 1kb will **not**
    /// continue reading at 2kb in the next call to `read`.
    ///
    /// # Notes
    ///
    /// This leave the current contents of `buf` untouched and only uses the
    /// spare capacity.
    pub fn read_at<'fd, B>(
        &'fd self,
        mut buf: B,
        offset: u64,
    ) -> result::Result<Read<'fd, B>, QueueFull>
    where
        B: ReadBuf,
    {
        self.state.start(|submission| unsafe {
            let (ptr, size) = buf.as_ptr();
            submission.read_at(self.fd, ptr, size, offset);
        })?;

        Ok(Read {
            buf: Some(UnsafeCell::new(buf)),
            fd: self,
        })
    }

    /// Write `buf` to this file.
    pub fn write<'fd, B>(&'fd self, buf: B) -> result::Result<Write<'fd, B>, QueueFull>
    where
        B: WriteBuf,
    {
        self.write_at(buf, NO_OFFSET)
    }

    /// Write `buf` to this file.
    ///
    /// The current file cursor is not affected by this function.
    pub fn write_at<'fd, B>(
        &'fd self,
        buf: B,
        offset: u64,
    ) -> result::Result<Write<'fd, B>, QueueFull>
    where
        B: WriteBuf,
    {
        self.state
            .start(|submission| unsafe { submission.write_at(self.fd, buf.as_slice(), offset) })?;

        Ok(Write {
            buf: Some(UnsafeCell::new(buf)),
            fd: self,
        })
    }

    /// Explicitly close the file descriptor.
    ///
    /// # Notes
    ///
    /// This happens automatically on drop, this can be used to get a possible
    /// error.
    pub fn close(self) -> result::Result<Close, QueueFull> {
        // We deconstruct `self` without dropping it to avoid closing the fd
        // twice.
        let this = ManuallyDrop::new(self);
        // SAFETY: this is safe because we're ensure the pointers are valid and
        // not touching `this` after reading the fields.
        let fd = unsafe { ptr::read(&this.fd) };
        let state = unsafe { ptr::read(&this.state) };
        state.start(|submission| unsafe { submission.close(fd, true) })?;

        Ok(Close { state })
    }
}

// Read.
op_future! {
    fn AsyncFd::read -> B,
    struct Read<'fd, B: ReadBuf> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B, "dropped `a10::io::Read` before completion, leaking buffer",
    },
    |this, n| {
        let mut buf = this.buf.take().unwrap().into_inner();
        unsafe { buf.set_init(n as usize) };
        Ok(buf)
    },
}

// Write.
op_future! {
    fn AsyncFd::write -> usize,
    struct Write<'fd, B: WriteBuf> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B, "dropped `a10::io::Write` before completion, leaking buffer",
    },
    |n| Ok(n as usize),
    extract: |this, n| -> (B, usize) {
        let buf = this.buf.take().unwrap().into_inner();
        Ok((buf, n as usize))
    }
}

/// [`Future`] to close a [`AsyncFd`]
#[derive(Debug)]
pub struct Close {
    state: SharedOperationState,
}

impl Future for Close {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.state.poll(ctx).map_ok(|_| ())
    }
}

/// A version of [`IoSliceMut`] that allows the buffer to be uninitialised.
///
/// [`IoSliceMut`]: std::io::IoSliceMut
#[repr(transparent)]
pub struct MaybeUninitSlice<'a> {
    vec: libc::iovec,
    _lifetime: PhantomData<&'a mut [MaybeUninit<u8>]>,
}

impl<'a> MaybeUninitSlice<'a> {
    /// Creates a new `MaybeUninitSlice` wrapping a byte slice.
    pub const fn new(buf: &'a mut [MaybeUninit<u8>]) -> MaybeUninitSlice<'a> {
        MaybeUninitSlice {
            vec: libc::iovec {
                iov_base: buf.as_mut_ptr().cast(),
                iov_len: buf.len(),
            },
            _lifetime: PhantomData,
        }
    }
}

impl<'a> Deref for MaybeUninitSlice<'a> {
    type Target = [MaybeUninit<u8>];

    fn deref(&self) -> &[MaybeUninit<u8>] {
        unsafe { slice::from_raw_parts(self.vec.iov_base.cast(), self.vec.iov_len) }
    }
}

impl<'a> DerefMut for MaybeUninitSlice<'a> {
    fn deref_mut(&mut self) -> &mut [MaybeUninit<u8>] {
        unsafe { slice::from_raw_parts_mut(self.vec.iov_base.cast(), self.vec.iov_len) }
    }
}

impl<'a> fmt::Debug for MaybeUninitSlice<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.deref().fmt(f)
    }
}

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

impl WriteBuf for &'static [u8] {
    unsafe fn as_slice(&self) -> &[u8] {
        self
    }
}

impl WriteBuf for &'static str {
    unsafe fn as_slice(&self) -> &[u8] {
        self.as_bytes()
    }
}
