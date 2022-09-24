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
    pub fn read<'fd>(&'fd self, buf: Vec<u8>) -> result::Result<Read<'fd>, QueueFull> {
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
    pub fn read_at<'fd>(
        &'fd self,
        mut buf: Vec<u8>,
        offset: u64,
    ) -> result::Result<Read<'fd>, QueueFull> {
        self.state.start(|submission| unsafe {
            submission.read_at(self.fd, buf.spare_capacity_mut(), offset);
        })?;

        Ok(Read {
            buf: Some(UnsafeCell::new(buf)),
            fd: self,
        })
    }

    /// Write `buf` to this file.
    pub fn write<'fd>(&'fd self, buf: Vec<u8>) -> result::Result<Write<'fd>, QueueFull> {
        self.write_at(buf, NO_OFFSET)
    }

    /// Write `buf` to this file.
    ///
    /// The current file cursor is not affected by this function.
    pub fn write_at<'fd>(
        &'fd self,
        buf: Vec<u8>,
        offset: u64,
    ) -> result::Result<Write<'fd>, QueueFull> {
        self.state
            .start(|submission| unsafe { submission.write_at(self.fd, &buf, offset) })?;

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
    fn AsyncFd::read -> Vec<u8>,
    struct Read<'fd> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: Vec<u8>, "dropped `a10::io::Read` before completion, leaking buffer",
    },
    |this, n| {
        let mut buf = this.buf.take().unwrap().into_inner();
        unsafe { buf.set_len(buf.len() + n as usize) };
        Ok(buf)
    },
}

// Write.
op_future! {
    fn AsyncFd::write -> usize,
    struct Write<'fd> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: Vec<u8>, "dropped `a10::io::Write` before completion, leaking buffer",
    },
    |n| Ok(n as usize),
    extract: |this, n| -> (Vec<u8>, usize) {
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
