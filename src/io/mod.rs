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

use std::mem::ManuallyDrop;
#[cfg(any(target_os = "android", target_os = "linux"))]
use std::os::fd::{AsRawFd, BorrowedFd};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{io, ptr};

use crate::extract::{Extract, Extractor};
#[cfg(any(target_os = "android", target_os = "linux"))]
use crate::new_flag;
#[cfg(any(target_os = "android", target_os = "linux"))]
use crate::op::fd_iter_operation;
use crate::op::{OpState, fd_operation, operation};
use crate::{AsyncFd, man_link, sys};

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

/// Create a function and type to wraps standard {in,out,error}.
macro_rules! stdio {
    (
        $fn: ident () -> $name: ident, $fd: expr
    ) => {
        #[doc = concat!("Create a new [`", stringify!($name), "`].")]
        pub fn $fn(sq: $crate::SubmissionQueue) -> $name {
            unsafe { $name(::std::mem::ManuallyDrop::new($crate::fd::AsyncFd::from_raw($fd, $crate::fd::Kind::File, sq))) }
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
        Read::new(self, buf, NO_OFFSET)
    }

    /// Continuously read data from the fd.
    ///
    /// # Notes
    ///
    /// Is restricted to pollable files and will fall back to single shot if the
    /// file does not support `NOWAIT`.
    ///
    /// This will return `ENOBUFS` if no buffer is available in the `pool` to
    /// read into.
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn multishot_read<'fd>(&'fd self, pool: ReadBufPool) -> MultishotRead<'fd> {
        MultishotRead::new(self, pool, ())
    }

    /// Read at least `n` bytes from this fd into `buf`.
    pub fn read_n<'fd, B>(&'fd self, buf: B, n: usize) -> ReadN<'fd, B>
    where
        B: BufMut,
    {
        let buf = ReadNBuf { buf, last_read: 0 };
        ReadN {
            read: self.read(buf),
            offset: NO_OFFSET,
            left: n,
        }
    }

    /// Read from this fd into `bufs`.
    #[doc = man_link!(readv(2))]
    pub fn read_vectored<'fd, B, const N: usize>(&'fd self, mut bufs: B) -> ReadVectored<'fd, B, N>
    where
        B: BufMutSlice<N>,
    {
        let iovecs = unsafe { bufs.as_iovecs_mut() };
        ReadVectored::new(self, (bufs, iovecs), NO_OFFSET)
    }

    /// Read at least `n` bytes from this fd into `bufs`.
    pub fn read_n_vectored<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
        n: usize,
    ) -> ReadNVectored<'fd, B, N>
    where
        B: BufMutSlice<N>,
    {
        let bufs = ReadNBuf {
            buf: bufs,
            last_read: 0,
        };
        ReadNVectored {
            read: self.read_vectored(bufs),
            offset: NO_OFFSET,
            left: n,
        }
    }

    /// Write `buf` to this fd.
    #[doc = man_link!(write(2))]
    pub fn write<'fd, B>(&'fd self, buf: B) -> Write<'fd, B>
    where
        B: Buf,
    {
        self.write_at(buf, NO_OFFSET)
    }

    /// Write `buf` to this fd at `offset`.
    ///
    /// The current file cursor is not affected by this function.
    pub fn write_at<'fd, B>(&'fd self, buf: B, offset: u64) -> Write<'fd, B>
    where
        B: Buf,
    {
        Write::new(self, buf, offset)
    }

    /// Write all of `buf` to this fd.
    pub fn write_all<'fd, B>(&'fd self, buf: B) -> WriteAll<'fd, B>
    where
        B: Buf,
    {
        self.write_all_at(buf, NO_OFFSET)
    }

    /// Write all of `buf` to this fd at `offset`.
    ///
    /// The current file cursor is not affected by this function.
    pub fn write_all_at<'fd, B>(&'fd self, buf: B, offset: u64) -> WriteAll<'fd, B>
    where
        B: Buf,
    {
        let buf = SkipBuf { buf, skip: 0 };
        WriteAll {
            write: Extractor {
                fut: self.write_at(buf, offset),
            },
            offset,
        }
    }

    /// Write `bufs` to this file.
    #[doc = man_link!(writev(2))]
    pub fn write_vectored<'fd, B, const N: usize>(&'fd self, bufs: B) -> WriteVectored<'fd, B, N>
    where
        B: BufSlice<N>,
    {
        self.write_vectored_at(bufs, NO_OFFSET)
    }

    /// Write `bufs` to this file at `offset`.
    ///
    /// The current file cursor is not affected by this function.
    pub fn write_vectored_at<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
        offset: u64,
    ) -> WriteVectored<'fd, B, N>
    where
        B: BufSlice<N>,
    {
        let iovecs = unsafe { bufs.as_iovecs() };
        WriteVectored::new(self, (bufs, iovecs), offset)
    }

    /// Write all `bufs` to this file.
    pub fn write_all_vectored<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
    ) -> WriteAllVectored<'fd, B, N>
    where
        B: BufSlice<N>,
    {
        self.write_all_vectored_at(bufs, NO_OFFSET)
    }

    /// Write all `bufs` to this file at `offset`.
    ///
    /// The current file cursor is not affected by this function.
    pub fn write_all_vectored_at<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
        offset: u64,
    ) -> WriteAllVectored<'fd, B, N>
    where
        B: BufSlice<N>,
    {
        WriteAllVectored {
            write: self.write_vectored_at(bufs, offset).extract(),
            offset,
            skip: 0,
        }
    }

    /// Splice `length` bytes to `target` fd.
    ///
    /// See the `splice(2)` manual for correct usage.
    #[doc = man_link!(splice(2))]
    #[doc(alias = "splice")]
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn splice_to<'fd>(
        &'fd self,
        target: BorrowedFd<'fd>,
        length: u32,
        flags: Option<SpliceFlag>,
    ) -> Splice<'fd> {
        self.splice_to_at(NO_OFFSET, target, NO_OFFSET, length, flags)
    }

    /// Same as [`AsyncFd::splice_to`], but starts reading data at `offset` from
    /// the file (instead of the current position of the read cursor) and starts
    /// writing at `target_offset` to `target`.
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn splice_to_at<'fd>(
        &'fd self,
        offset: u64,
        target: BorrowedFd<'fd>,
        target_offset: u64,
        length: u32,
        flags: Option<SpliceFlag>,
    ) -> Splice<'fd> {
        self.splice(
            target,
            SpliceDirection::To,
            offset,
            target_offset,
            length,
            flags,
        )
    }

    /// Splice `length` bytes from `target` fd.
    ///
    /// See the `splice(2)` manual for correct usage.
    #[doc(alias = "splice")]
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn splice_from<'fd>(
        &'fd self,
        target: BorrowedFd<'fd>,
        length: u32,
        flags: Option<SpliceFlag>,
    ) -> Splice<'fd> {
        self.splice_from_at(NO_OFFSET, target, NO_OFFSET, length, flags)
    }

    /// Same as [`AsyncFd::splice_from`], but starts reading writing at `offset`
    /// to the file (instead of the current position of the write cursor) and
    /// starts reading at `target_offset` from `target`.
    #[doc(alias = "splice")]
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn splice_from_at<'fd>(
        &'fd self,
        offset: u64,
        target: BorrowedFd<'fd>,
        target_offset: u64,
        length: u32,
        flags: Option<SpliceFlag>,
    ) -> Splice<'fd> {
        self.splice(
            target,
            SpliceDirection::From,
            target_offset,
            offset,
            length,
            flags,
        )
    }

    #[cfg(any(target_os = "android", target_os = "linux"))]
    fn splice<'fd>(
        &'fd self,
        target: BorrowedFd<'fd>,
        direction: SpliceDirection,
        off_in: u64,
        off_out: u64,
        length: u32,
        flags: Option<SpliceFlag>,
    ) -> Splice<'fd> {
        let target_fd = target.as_raw_fd();
        let flags = match flags {
            Some(flags) => flags,
            None => SpliceFlag(0),
        };
        let args = (target_fd, direction, off_in, off_out, length, flags);
        Splice::new(self, (), args)
    }

    /// Explicitly close the file descriptor.
    ///
    /// # Notes
    ///
    /// This happens automatically on drop, this can be used to get a possible
    /// error.
    #[doc = man_link!(close(2))]
    pub fn close(self) -> Close {
        // We deconstruct `self` without dropping it to avoid closing the fd
        // twice.
        let this = ManuallyDrop::new(self);
        // SAFETY: this is safe because we're ensure the pointers are valid and
        // not touching `this` after reading the fields.
        let fd = this.fd();
        let kind = this.kind();
        let sq = unsafe { ptr::read(&raw const this.sq) };
        Close::new(sq, (), (fd, kind))
    }
}

#[derive(Copy, Clone, Debug)]
#[cfg(any(target_os = "android", target_os = "linux"))]
pub(crate) enum SpliceDirection {
    To,
    From,
}

#[cfg(any(target_os = "android", target_os = "linux"))]
new_flag!(
    /// Splice flags.
    ///
    /// See [`AsyncFd::splice_to`] and related function.
    pub struct SpliceFlag(u32) impl BitOr {
        /// Attempt to move pages instead of copying.
        MOVE = libc::SPLICE_F_MOVE,
        /// More data will be coming in a subsequent splice.
        MORE = libc::SPLICE_F_MORE,
    }
);

fd_operation!(
    /// [`Future`] behind [`AsyncFd::read`].
    pub struct Read<B: BufMut>(sys::io::ReadOp<B>) -> io::Result<B>;

    /// [`Future`] behind [`AsyncFd::read_vectored`].
    pub struct ReadVectored<B: BufMutSlice<N>; const N: usize>(sys::io::ReadVectoredOp<B, N>) -> io::Result<B>;

    /// [`Future`] behind [`AsyncFd::write`] and [`AsyncFd::write_at`].
    pub struct Write<B: Buf>(sys::io::WriteOp<B>) -> io::Result<usize>,
      impl Extract -> io::Result<(B, usize)>;

    /// [`Future`] behind [`AsyncFd::write_vectored`] and [`AsyncFd::write_vectored_at`].
    pub struct WriteVectored<B: BufSlice<N>; const N: usize>(sys::io::WriteVectoredOp<B, N>) -> io::Result<usize>,
      impl Extract -> io::Result<(B, usize)>;
);

impl<'fd, B: BufMut> Read<'fd, B> {
    /// Change to a positional read starting at `offset`.
    ///
    /// The current file cursor is not affected by this function. This means
    /// that a call `read(buf).at(1024)` with a buffer of 1kb will **not**
    /// continue reading at 2kb in the next call to `read`.
    #[doc = man_link!(pread(2))]
    pub fn at(mut self, offset: u64) -> Self {
        if let Some(off) = self.state.args_mut() {
            *off = offset;
        }
        self
    }
}

impl<'fd, B: BufMutSlice<N>, const N: usize> ReadVectored<'fd, B, N> {
    /// Change to a positional read starting at `offset`.
    ///
    /// Also see [`Read::at`].
    #[doc = man_link!(preadv(2))]
    pub fn at(mut self, offset: u64) -> Self {
        if let Some(off) = self.state.args_mut() {
            *off = offset;
        }
        self
    }
}

#[cfg(any(target_os = "android", target_os = "linux"))]
fd_operation!(
    /// [`Future`] behind [`AsyncFd::splice_to`], [`AsyncFd::splice_to_at`],
    /// [`AsyncFd::splice_from`] and [`AsyncFd::splice_from_at`].
    pub struct Splice(sys::io::SpliceOp) -> io::Result<usize>;
);

#[cfg(any(target_os = "android", target_os = "linux"))]
fd_iter_operation! {
    /// [`AsyncIterator`] behind [`AsyncFd::multishot_read`].
    pub struct MultishotRead(sys::io::MultishotReadOp) -> io::Result<ReadBuf>;
}

/// [`Future`] behind [`AsyncFd::read_n`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct ReadN<'fd, B: BufMut> {
    read: Read<'fd, ReadNBuf<B>>,
    offset: u64,
    /// Number of bytes we still need to read to hit our minimum.
    left: usize,
}

impl<'fd, B: BufMut> ReadN<'fd, B> {
    /// Change to a positional read starting at `offset`.
    ///
    /// Also see [`Read::at`].
    pub fn at(mut self, offset: u64) -> Self {
        if let Some(off) = self.read.state.args_mut() {
            *off = offset;
            self.offset = offset;
        }
        self
    }
}

impl<'fd, B: BufMut> Future for ReadN<'fd, B> {
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

                read.set(read.fd.read(buf).at(this.offset));
                unsafe { Pin::new_unchecked(this) }.poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// [`Future`] behind [`AsyncFd::read_n_vectored`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct ReadNVectored<'fd, B: BufMutSlice<N>, const N: usize> {
    read: ReadVectored<'fd, ReadNBuf<B>, N>,
    offset: u64,
    /// Number of bytes we still need to read to hit our minimum.
    left: usize,
}

impl<'fd, B: BufMutSlice<N>, const N: usize> ReadNVectored<'fd, B, N> {
    /// Change to a positional read starting at `offset`.
    ///
    /// Also see [`Read::at`].
    pub fn at(mut self, offset: u64) -> Self {
        if let Some(off) = self.read.state.args_mut() {
            *off = offset;
            self.offset = offset;
        }
        self
    }
}

impl<'fd, B: BufMutSlice<N>, const N: usize> Future for ReadNVectored<'fd, B, N> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `Future`.
        let this = unsafe { Pin::into_inner_unchecked(self) };
        let mut read = unsafe { Pin::new_unchecked(&mut this.read) };
        match read.as_mut().poll(ctx) {
            Poll::Ready(Ok(bufs)) => {
                if bufs.last_read == 0 {
                    return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                }

                if bufs.last_read >= this.left {
                    // Read the required amount of bytes.
                    return Poll::Ready(Ok(bufs.buf));
                }

                this.left -= bufs.last_read;
                if this.offset != NO_OFFSET {
                    this.offset += bufs.last_read as u64;
                }

                read.set(read.fd.read_vectored(bufs).at(this.offset));
                unsafe { Pin::new_unchecked(this) }.poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// [`Future`] behind [`AsyncFd::write_all`] and [`AsyncFd::write_all_at`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct WriteAll<'fd, B: Buf> {
    write: Extractor<Write<'fd, SkipBuf<B>>>,
    offset: u64,
}

impl<'fd, B: Buf> WriteAll<'fd, B> {
    fn poll_inner(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<io::Result<B>> {
        // SAFETY: not moving `Future`.
        let this = unsafe { Pin::into_inner_unchecked(self) };
        let mut write = unsafe { Pin::new_unchecked(&mut this.write) };
        match write.as_mut().poll(ctx) {
            Poll::Ready(Ok((_, 0))) => Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
            Poll::Ready(Ok((mut buf, n))) => {
                buf.skip += n as u32;
                if this.offset != NO_OFFSET {
                    this.offset += n as u64;
                }

                if let (_, 0) = unsafe { buf.parts() } {
                    // Written everything.
                    return Poll::Ready(Ok(buf.buf));
                }

                write.set(write.fut.fd.write_at(buf, this.offset).extract());
                unsafe { Pin::new_unchecked(this) }.poll_inner(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'fd, B: Buf> Future for WriteAll<'fd, B> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.poll_inner(ctx).map_ok(|_| ())
    }
}

impl<'fd, B: Buf> Extract for WriteAll<'fd, B> {}

impl<'fd, B: Buf> Future for Extractor<WriteAll<'fd, B>> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `self.fut` (`s.fut`), directly called
        // `Future::poll` on it.
        unsafe { Pin::map_unchecked_mut(self, |s| &mut s.fut) }.poll_inner(ctx)
    }
}

/// [`Future`] behind [`AsyncFd::write_all_vectored`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct WriteAllVectored<'fd, B: BufSlice<N>, const N: usize> {
    write: Extractor<WriteVectored<'fd, B, N>>,
    offset: u64,
    skip: u64,
}

impl<'fd, B: BufSlice<N>, const N: usize> WriteAllVectored<'fd, B, N> {
    /// Poll implementation used by the [`Future`] implement for the naked type
    /// and the type wrapper in an [`Extractor`].
    fn poll_inner(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<io::Result<B>> {
        // SAFETY: not moving `Future`.
        let this = unsafe { Pin::into_inner_unchecked(self) };
        let mut write = unsafe { Pin::new_unchecked(&mut this.write) };
        match write.as_mut().poll(ctx) {
            Poll::Ready(Ok((_, 0))) => Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
            Poll::Ready(Ok((bufs, n))) => {
                this.skip += n as u64;
                if this.offset != NO_OFFSET {
                    this.offset += n as u64;
                }

                let mut iovecs = unsafe { bufs.as_iovecs() };
                let mut skip = this.skip;
                for iovec in &mut iovecs {
                    if iovec.len() as u64 <= skip {
                        // Skip entire buf.
                        skip -= iovec.len() as u64;
                        // SAFETY: setting it to zero is always valid.
                        unsafe { iovec.set_len(0) };
                    } else {
                        // SAFETY: checked above that the length > skip.
                        unsafe { iovec.set_len(skip as usize) };
                        break;
                    }
                }

                if iovecs[N - 1].len() == 0 {
                    // Written everything.
                    return Poll::Ready(Ok(bufs));
                }

                write.set(WriteVectored::new(write.fut.fd, (bufs, iovecs), this.offset).extract());
                unsafe { Pin::new_unchecked(this) }.poll_inner(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'fd, B: BufSlice<N>, const N: usize> Future for WriteAllVectored<'fd, B, N> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.poll_inner(ctx).map_ok(|_| ())
    }
}

impl<'fd, B: BufSlice<N>, const N: usize> Extract for WriteAllVectored<'fd, B, N> {}

impl<'fd, B: BufSlice<N>, const N: usize> Future for Extractor<WriteAllVectored<'fd, B, N>> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        unsafe { Pin::map_unchecked_mut(self, |s| &mut s.fut) }.poll_inner(ctx)
    }
}

operation!(
    /// [`Future`] behind [`AsyncFd::close`].
    pub struct Close(sys::io::CloseOp) -> io::Result<()>;
);

/// Wrapper around a buffer `B` to keep track of the number of bytes written.
// Also used in the `net` module.
#[derive(Debug)]
pub(crate) struct ReadNBuf<B> {
    pub(crate) buf: B,
    pub(crate) last_read: usize,
}

unsafe impl<B: BufMut> BufMut for ReadNBuf<B> {
    unsafe fn parts_mut(&mut self) -> (*mut u8, u32) {
        unsafe { self.buf.parts_mut() }
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.last_read = n;
        unsafe { self.buf.set_init(n) };
    }

    fn buffer_group(&self) -> Option<BufGroupId> {
        self.buf.buffer_group()
    }

    unsafe fn buffer_init(&mut self, id: BufId, n: u32) {
        self.last_read = n as usize;
        unsafe { self.buf.buffer_init(id, n) };
    }
}

unsafe impl<B: BufMutSlice<N>, const N: usize> BufMutSlice<N> for ReadNBuf<B> {
    unsafe fn as_iovecs_mut(&mut self) -> [IoMutSlice; N] {
        unsafe { self.buf.as_iovecs_mut() }
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.last_read = n;
        unsafe { self.buf.set_init(n) };
    }
}

/// Wrapper around a buffer `B` to skip a number of bytes.
// Also used in the `net` module.
#[derive(Debug)]
pub(crate) struct SkipBuf<B> {
    pub(crate) buf: B,
    pub(crate) skip: u32,
}

unsafe impl<B: Buf> Buf for SkipBuf<B> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let (ptr, size) = unsafe { self.buf.parts() };
        if self.skip >= size {
            (ptr, 0)
        } else {
            (unsafe { ptr.add(self.skip as usize) }, size - self.skip)
        }
    }
}
