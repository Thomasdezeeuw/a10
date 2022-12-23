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
            let (ptr, len) = buf.parts();
            submission.read_at(self.fd, ptr, len, offset);
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
        self.state.start(|submission| unsafe {
            let (ptr, len) = buf.parts();
            submission.write_at(self.fd, ptr, len, offset);
        })?;

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
///
/// # Safety
///
/// Unlike normal buffers the buffer implementations for A10 have additional
/// requirements.
///
/// If the operation (that uses this buffer) is not polled to completion, i.e.
/// the `Future` is dropped before it returns `Poll::Ready` the kernel still has
/// access to the buffer and will still attempt to write into it. This means
/// that we must ensure that we can leak the buffer in such a way that the
/// kernel will not write into memory we don't have access to any more. This
/// makes, for example, stack based buffers unfit to implement `ReadBuf`.
/// Because if they were to be leaked the kernel will overwrite part of your
/// stack (where the buffer used to be)!
pub unsafe trait ReadBuf: 'static {
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
    unsafe fn parts(&mut self) -> (*mut u8, u32);

    /// Mark `n` bytes as initialised.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `n` bytes, starting at the pointer returned
    /// by [`ReadBuf::parts`], are initialised.
    unsafe fn set_init(&mut self, n: usize);
}

/// The implementation for `Vec<u8>` only uses the unused capacity, so any bytes
/// already in the buffer will be untouched.
// SAFETY: `Vec<u8>` manages the allocation of the bytes, so as long as it's
// alive, so is the slice of bytes. When the `Vec`tor is leaked the allocation
// will also be leaked.
unsafe impl ReadBuf for Vec<u8> {
    unsafe fn parts(&mut self) -> (*mut u8, u32) {
        let slice = self.spare_capacity_mut();
        (slice.as_mut_ptr().cast(), slice.len() as u32)
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.set_len(self.len() + n);
    }
}

/// Trait that defines the behaviour of buffers used in writing.
///
/// # Safety
///
/// Unlike normal buffers the buffer implementations for A10 have additional
/// requirements.
///
/// If the operation (that uses this buffer) is not polled to completion, i.e.
/// the `Future` is dropped before it returns `Poll::Ready` the kernel still has
/// access to the buffer and will still attempt to read from it. This means that
/// we must ensure that we can leak the buffer in such a way that the kernel
/// will not read memory we don't have access to any more. This makes, for
/// example, stack based buffers unfit to implement `WriteBuf`. Because if they
/// were to be leaked the kernel will read part of your stack (where the buffer
/// used to be)! This would be a huge security risk.
pub unsafe trait WriteBuf {
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
unsafe impl WriteBuf for Vec<u8> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let slice = self.as_slice();
        (slice.as_ptr().cast(), slice.len() as u32)
    }
}

// SAFETY: because the reference has a `'static` lifetime we know the bytes
// can't be deallocated, so it's safe to implement `WriteBuf`.
unsafe impl WriteBuf for &'static [u8] {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_ptr(), self.len() as u32)
    }
}

// SAFETY: because the reference has a `'static` lifetime we know the bytes
// can't be deallocated, so it's safe to implement `WriteBuf`.
unsafe impl WriteBuf for &'static str {
    unsafe fn parts(&self) -> (*const u8, u32) {
        (self.as_bytes().as_ptr(), self.len() as u32)
    }
}
