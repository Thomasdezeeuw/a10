//! Type definitions for I/O functionality.
//!
//! # Working with Buffers
//!
//! For working with buffers A10 defines two plus two traits. For "regular",
//! i.e. single buffer I/O, we have the following two traits:
//!  * [`Buf`] is used in writing/sending.
//!  * [`BufMut`] is used in reading/receiving.
//!
//! The basic design of both traits is the same and is fairly simple. Usage
//! starts with a call to [`parts`]/[`parts_mut`], which returns a pointer to
//! the bytes in the bufer to read from or write into. For `BufMut` the caller
//! writes into the buffer and updates the length using [`set_init`], though
//! normally this is done by an I/O operation.
//!
//! For vectored I/O we have the same two traits as above, but suffixed with
//! `Slice`:
//!  * [`BufSlice`] is used in vectored writing/sending.
//!  * [`BufMutSlice`] is used in vectored reading/receiving.
//!
//! Neither of these traits can be implemented outside of the crate, but it's
//! already implemented for tuples and arrays.
//!
//! [`parts`]: Buf::parts
//! [`parts_mut`]: BufMut::parts_mut
//! [`set_init`]: BufMut::set_init
//!
//! ## Specialised Buffer Pool
//!
//! [`ReadBufPool`] is a specialised read buffer pool that can only be used in
//! read operations done by the kernel, i.e. no in-memory operations. In return
//! for this limited functionality we can do things such as [multishot reads],
//! which can greatly improve the performance by batching multiple reads into a
//! single system call.
//!
//! [multishot reads]: crate::fd::AsyncFd::multishot_read
//!
//! # Working with Standard I/O Streams
//!
//! The [`stdin`], [`stdout`] and [`stderr`] functions provide handles to
//! standard I/O streams of all Unix processes. All I/O performed using these
//! handles will non-blocking I/O.
//!
//! Note that these handles are **not** buffered, unlike the ones found in the
//! standard library (e.g. [`std::io::stdout`]). Furthermore these handle do not
//! flush the buffer used by the standard library, so it's not advised to use
//! both the handle from standard library and Heph simultaneously.

use std::io;

use crate::op::fd_operation;
use crate::{man_link, sys, AsyncFd};

#[cfg(any(target_os = "android", target_os = "linux"))]
mod read_buf;
mod traits;

#[cfg(any(target_os = "android", target_os = "linux"))]
pub use read_buf::{ReadBuf, ReadBufPool};
#[allow(unused_imports)] // Not used by all OS.
pub(crate) use traits::{BufGroupId, BufId};
// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

#[doc(inline)]
pub use traits::{Buf, BufMut, BufMutSlice, BufSlice, IoMutSlice, IoSlice, StaticBuf};

/// The io_uring_enter(2) manual says for IORING_OP_READ and IORING_OP_WRITE:
/// > If offs is set to -1, the offset will use (and advance) the file
/// > position, like the read(2) and write(2) system calls.
///
/// `-1` cast as `unsigned long long` in C is the same as as `u64::MAX`.
pub(crate) const NO_OFFSET: u64 = u64::MAX;

/// I/O system calls.
impl AsyncFd {
    /// Read from this fd into `buf`.
    #[doc = man_link!(read(2))]
    pub fn read<'fd, B>(&'fd self, buf: B) -> Read<'fd, B>
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
    pub fn read_at<'fd, B>(&'fd self, buf: B, offset: u64) -> Read<'fd, B>
    where
        B: BufMut,
    {
        Read::new(self, buf, offset)
    }
}

fd_operation!(
    /// [`Future`] behind [`AsyncFd::read`] and [`AsyncFd::read_at`].
    pub struct Read<B: BufMut>(sys::io::ReadOp<B>) -> io::Result<B>;
);
