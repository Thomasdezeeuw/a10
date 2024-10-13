//! Type definitions for I/O functionality.
//!
//! The main types of this module are the [`Buf`] and [`BufMut`] traits, which
//! define the requirements on buffers using the I/O system calls on an file
//! descriptor ([`AsyncFd`]). Additionally the [`BufSlice`] and [`BufMutSlice`]
//! traits existing to define the behaviour of buffers in vectored I/O.
//!
//! A specialised io_uring-only read buffer pool implementation exists in
//! [`ReadBufPool`], which is a buffer pool managed by the kernel when making
//! `read(2)`-like system calls.
//!
//! Finally we have the [`stdin`], [`stdout`] and [`stderr`] functions to create
//! `AsyncFd`s for standard in, out and error respectively.

use crate::fd::{AsyncFd, Descriptor, File};
use crate::op::Operation;
use crate::{man_link, sys};

mod traits;

pub use traits::{Buf, BufMut, BufMutSlice, BufSlice};
// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

/// The io_uring_enter(2) manual says for IORING_OP_READ and IORING_OP_WRITE:
/// > If offs is set to -1, the offset will use (and advance) the file
/// > position, like the read(2) and write(2) system calls.
///
/// `-1` cast as `unsigned long long` in C is the same as as `u64::MAX`.
pub(crate) const NO_OFFSET: u64 = u64::MAX;

/// I/O system calls.
impl<D: Descriptor> AsyncFd<D> {
    /// Read from this fd into `buf`.
    #[doc = man_link!(read(2))]
    pub const fn read<'fd, B>(&'fd self, buf: B) -> Read<'fd, B, D>
    where
        B: BufMut,
    {
        self.read_at(buf, NO_OFFSET)
    }

    /// Read from this fd into `buf` starting at `offset`.
    ///
    /// The current file cursor is not affected by this function. This means
    /// that a call `read_at(buf, 1024)` with a buffer of 1kb will **not**
    /// continue reading at 2kb in the next call to `read`.
    #[doc = man_link!(pread(2))]
    pub const fn read_at<'fd, B>(&'fd self, buf: B, offset: u64) -> Read<'fd, B, D>
    where
        B: BufMut,
    {
        Read::new(self, buf, offset)
    }
}

/// [`Future`] behind [`AsyncFd::read`] and [`AsyncFd::read_at`].
pub struct Read<'fd, B: BufMut, D: Descriptor = File> {
    inner: Operation<'fd, sys::io::Read<B>, D>,
}

impl<'fd, B: BufMut, D: Descriptor> Read<'fd, B, D> {
    const fn new(fd: &'fd AsyncFd<D>, buf: B, offset: u64) -> Read<'fd, B, D> {
        Read {
            inner: Operation::new(fd, buf, offset),
        }
    }
}
