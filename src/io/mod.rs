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

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::fd::{AsyncFd, Descriptor, File};
use crate::op::{fd_operation, FdOperation};
use crate::{man_link, sys};

mod traits;

pub use traits::{Buf, BufMut, BufMutSlice, BufSlice, IoMutSlice, IoSlice};
#[allow(unused_imports)] // Not used by all OS.
pub(crate) use traits::{BufGroupId, BufId};
// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

/// The io_uring_enter(2) manual says for IORING_OP_READ and IORING_OP_WRITE:
/// > If offs is set to -1, the offset will use (and advance) the file
/// > position, like the read(2) and write(2) system calls.
///
/// `-1` cast as `unsigned long long` in C is the same as as `u64::MAX`.
pub(crate) const NO_OFFSET: u64 = u64::MAX;

/// Create a function and type to wraps standard {in,out,error}.
macro_rules! stdio {
    (
        $fn: ident () -> $name: ident, $fd: expr
    ) => {
        #[doc = concat!("Create a new [`", stringify!($name), "`].")]
        pub fn $fn(sq: $crate::SubmissionQueue) -> $name {
            unsafe { $name(::std::mem::ManuallyDrop::new($crate::fd::AsyncFd::from_raw_fd($fd, sq))) }
        }

        #[doc = concat!(
            "An [`AsyncFd`] for ", stringify!($fn), ".\n\n",
            "Created by calling [`", stringify!($fn), "`].\n\n",
            "# Notes\n\n",
            "This directly writes to the raw file descriptor, which means it's not buffered and will not flush anything buffered by the standard library.\n\n",
            "When this type is dropped it will not close ", stringify!($fn), ".",
        )]
        pub struct $name(::std::mem::ManuallyDrop<$crate::fd::AsyncFd>);

        impl ::std::ops::Deref for $name {
            type Target = $crate::fd::AsyncFd;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl ::std::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(::std::stringify!($name))
                    .field("fd", &*self.0)
                    .finish()
            }
        }

        impl ::std::ops::Drop for $name {
            fn drop(&mut self) {
                // We don't want to close the file descriptor, but we do need to
                // drop our reference to the submission queue.
                // SAFETY: with `ManuallyDrop` we don't drop the `AsyncFd` so
                // it's not dropped twice. Otherwise we get access to it using
                // safe methods.
                unsafe { ::std::ptr::drop_in_place(&mut self.0.sq) };
            }
        }
    };
}

stdio!(stdin() -> Stdin, libc::STDIN_FILENO);
stdio!(stdout() -> Stdout, libc::STDOUT_FILENO);
stdio!(stderr() -> Stderr, libc::STDERR_FILENO);

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
        Read(FdOperation::new(self, buf, offset))
    }

    /// Read at least `n` bytes from this fd into `buf`.
    pub const fn read_n<'fd, B>(&'fd self, buf: B, n: usize) -> ReadN<'fd, B, D>
    where
        B: BufMut,
    {
        self.read_n_at(buf, NO_OFFSET, n)
    }

    /// Read at least `n` bytes from this fd into `buf` starting at `offset`.
    ///
    /// The current file cursor is not affected by this function.
    pub const fn read_n_at<'fd, B>(&'fd self, buf: B, offset: u64, n: usize) -> ReadN<'fd, B, D>
    where
        B: BufMut,
    {
        let buf = ReadNBuf { buf, last_read: 0 };
        ReadN {
            read: self.read_at(buf, offset),
            offset,
            left: n,
        }
    }

    /// Read from this fd into `bufs`.
    #[doc = man_link!(readv(2))]
    pub fn read_vectored<'fd, B, const N: usize>(&'fd self, bufs: B) -> ReadVectored<'fd, B, N, D>
    where
        B: BufMutSlice<N>,
    {
        self.read_vectored_at(bufs, NO_OFFSET)
    }

    /// Read from this fd into `bufs` starting at `offset`.
    ///
    /// The current file cursor is not affected by this function.
    #[doc = man_link!(preadv(2))]
    pub fn read_vectored_at<'fd, B, const N: usize>(
        &'fd self,
        mut bufs: B,
        offset: u64,
    ) -> ReadVectored<'fd, B, N, D>
    where
        B: BufMutSlice<N>,
    {
        let iovecs = unsafe { bufs.as_iovecs_mut() };
        ReadVectored(FdOperation::new(self, (bufs, iovecs), offset))
    }
}

fd_operation!(
    /// [`Future`] behind [`AsyncFd::read`] and [`AsyncFd::read_at`].
    pub struct Read<B: BufMut>(sys::io::ReadOp<B>) -> io::Result<B>;

    /// [`Future`] behind [`AsyncFd::read_vectored`] and [`AsyncFd::read_vectored_at`].
    pub struct ReadVectored<B: BufMutSlice<N>; const N: usize>(sys::io::ReadVectoredOp<B, N>) -> io::Result<B>;
);

/// [`Future`] behind [`AsyncFd::read_n`] and [`AsyncFd::read_n_at`].
#[derive(Debug)]
pub struct ReadN<'fd, B: BufMut, D: Descriptor = File> {
    read: Read<'fd, ReadNBuf<B>, D>,
    offset: u64,
    /// Number of bytes we still need to read to hit our target `N`.
    left: usize,
}

/* TODO(port): add back Cancel support.
impl<'fd, B: BufMut, D: Descriptor> Cancel for ReadN<'fd, B, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.read.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
        self.read.cancel()
    }
}
*/

impl<'fd, B: BufMut, D: Descriptor> Future for ReadN<'fd, B, D> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `self`.
        let this = unsafe { Pin::into_inner_unchecked(self) };
        let mut read = unsafe { Pin::new_unchecked(&mut this.read) };
        match read.as_mut().poll(ctx) {
            Poll::Ready(Ok(buf)) => {
                if buf.last_read == 0 {
                    return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                }

                if buf.last_read >= this.left {
                    // Read the required amount of bytes.
                    return Poll::Ready(Ok(buf.buf));
                }

                this.left -= buf.last_read;
                if this.offset != NO_OFFSET {
                    this.offset += buf.last_read as u64;
                }

                read.set(read.0.fd().read_at(buf, this.offset));
                unsafe { Pin::new_unchecked(this) }.poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Wrapper around a buffer `B` to keep track of the number of bytes written.
#[derive(Debug)]
pub(crate) struct ReadNBuf<B> {
    pub(crate) buf: B,
    pub(crate) last_read: usize,
}

unsafe impl<B: BufMut> BufMut for ReadNBuf<B> {
    unsafe fn parts_mut(&mut self) -> (*mut u8, u32) {
        self.buf.parts_mut()
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.last_read = n;
        self.buf.set_init(n);
    }

    fn buffer_group(&self) -> Option<BufGroupId> {
        self.buf.buffer_group()
    }

    unsafe fn buffer_init(&mut self, id: BufId, n: u32) {
        self.last_read = n as usize;
        self.buf.buffer_init(id, n);
    }
}

unsafe impl<B: BufMutSlice<N>, const N: usize> BufMutSlice<N> for ReadNBuf<B> {
    unsafe fn as_iovecs_mut(&mut self) -> [IoMutSlice; N] {
        self.buf.as_iovecs_mut()
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.last_read = n;
        self.buf.set_init(n);
    }
}
