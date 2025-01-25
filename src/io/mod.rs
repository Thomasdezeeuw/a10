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
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{io, ptr};

use crate::cancel::{Cancel, CancelOperation, CancelResult};
use crate::extract::{Extract, Extractor};
use crate::fd::{AsyncFd, Descriptor, File};
use crate::op::{fd_operation, operation, FdOperation, Operation};
use crate::{man_link, sys};

mod read_buf;
mod traits;

pub use read_buf::{ReadBuf, ReadBufPool};
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
        let buf = Buffer { buf };
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

    /// Read at least `n` bytes from this fd into `bufs`.
    pub fn read_n_vectored<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
        n: usize,
    ) -> ReadNVectored<'fd, B, N, D>
    where
        B: BufMutSlice<N>,
    {
        self.read_n_vectored_at(bufs, NO_OFFSET, n)
    }

    /// Read at least `n` bytes from this fd into `bufs`.
    ///
    /// The current file cursor is not affected by this function.
    pub fn read_n_vectored_at<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
        offset: u64,
        n: usize,
    ) -> ReadNVectored<'fd, B, N, D>
    where
        B: BufMutSlice<N>,
    {
        let bufs = ReadNBuf {
            buf: bufs,
            last_read: 0,
        };
        ReadNVectored {
            read: self.read_vectored_at(bufs, offset),
            offset,
            left: n,
        }
    }

    /// Write `buf` to this fd.
    #[doc = man_link!(write(2))]
    pub const fn write<'fd, B>(&'fd self, buf: B) -> Write<'fd, B, D>
    where
        B: Buf,
    {
        self.write_at(buf, NO_OFFSET)
    }

    /// Write `buf` to this fd at `offset`.
    ///
    /// The current file cursor is not affected by this function.
    pub const fn write_at<'fd, B>(&'fd self, buf: B, offset: u64) -> Write<'fd, B, D>
    where
        B: Buf,
    {
        let buf = Buffer { buf };
        Write(FdOperation::new(self, buf, offset))
    }

    /// Write all of `buf` to this fd.
    pub const fn write_all<'fd, B>(&'fd self, buf: B) -> WriteAll<'fd, B, D>
    where
        B: Buf,
    {
        self.write_all_at(buf, NO_OFFSET)
    }

    /// Write all of `buf` to this fd at `offset`.
    ///
    /// The current file cursor is not affected by this function.
    pub const fn write_all_at<'fd, B>(&'fd self, buf: B, offset: u64) -> WriteAll<'fd, B, D>
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
    pub fn write_vectored<'fd, B, const N: usize>(&'fd self, bufs: B) -> WriteVectored<'fd, B, N, D>
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
    ) -> WriteVectored<'fd, B, N, D>
    where
        B: BufSlice<N>,
    {
        let iovecs = unsafe { bufs.as_iovecs() };
        WriteVectored(FdOperation::new(self, (bufs, iovecs), offset))
    }

    /// Write all `bufs` to this file.
    pub fn write_all_vectored<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
    ) -> WriteAllVectored<'fd, B, N, D>
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
    ) -> WriteAllVectored<'fd, B, N, D>
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
    pub fn splice_to<'fd>(
        &'fd self,
        target: BorrowedFd<'fd>,
        length: u32,
        flags: libc::c_int,
    ) -> Splice<'fd, D> {
        self.splice_to_at(NO_OFFSET, target, NO_OFFSET, length, flags)
    }

    /// Same as [`AsyncFd::splice_to`], but starts reading data at `offset` from
    /// the file (instead of the current position of the read cursor) and starts
    /// writing at `target_offset` to `target`.
    pub fn splice_to_at<'fd>(
        &'fd self,
        offset: u64,
        target: BorrowedFd<'fd>,
        target_offset: u64,
        length: u32,
        flags: libc::c_int,
    ) -> Splice<'fd, D> {
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
    pub fn splice_from<'fd>(
        &'fd self,
        target: BorrowedFd<'fd>,
        length: u32,
        flags: libc::c_int,
    ) -> Splice<'fd, D> {
        self.splice_from_at(NO_OFFSET, target, NO_OFFSET, length, flags)
    }

    /// Same as [`AsyncFd::splice_from`], but starts reading writing at `offset`
    /// to the file (instead of the current position of the write cursor) and
    /// starts reading at `target_offset` from `target`.
    #[doc(alias = "splice")]
    pub fn splice_from_at<'fd>(
        &'fd self,
        offset: u64,
        target: BorrowedFd<'fd>,
        target_offset: u64,
        length: u32,
        flags: libc::c_int,
    ) -> Splice<'fd, D> {
        self.splice(
            target,
            SpliceDirection::From,
            target_offset,
            offset,
            length,
            flags,
        )
    }

    fn splice<'fd>(
        &'fd self,
        target: BorrowedFd<'fd>,
        direction: SpliceDirection,
        off_in: u64,
        off_out: u64,
        length: u32,
        flags: libc::c_int,
    ) -> Splice<'fd, D> {
        let target_fd = target.as_raw_fd();
        let args = (target_fd, direction, off_in, off_out, length, flags);
        Splice(FdOperation::new(self, (), args))
    }

    /// Explicitly close the file descriptor.
    ///
    /// # Notes
    ///
    /// This happens automatically on drop, this can be used to get a possible
    /// error.
    #[doc = man_link!(close(2))]
    pub fn close(self) -> Close<D> {
        // We deconstruct `self` without dropping it to avoid closing the fd
        // twice.
        let this = ManuallyDrop::new(self);
        // SAFETY: this is safe because we're ensure the pointers are valid and
        // not touching `this` after reading the fields.
        let fd = this.fd();
        let sq = unsafe { ptr::read(&this.sq) };
        Close(Operation::new(sq, (), fd))
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum SpliceDirection {
    To,
    From,
}

fd_operation!(
    /// [`Future`] behind [`AsyncFd::read`] and [`AsyncFd::read_at`].
    pub struct Read<B: BufMut>(sys::io::ReadOp<B>) -> io::Result<B>;

    /// [`Future`] behind [`AsyncFd::read_vectored`] and [`AsyncFd::read_vectored_at`].
    pub struct ReadVectored<B: BufMutSlice<N>; const N: usize>(sys::io::ReadVectoredOp<B, N>) -> io::Result<B>;

    /// [`Future`] behind [`AsyncFd::write`] and [`AsyncFd::write_at`].
    pub struct Write<B: Buf>(sys::io::WriteOp<B>) -> io::Result<usize>,
      impl Extract -> io::Result<(B, usize)>;

    /// [`Future`] behind [`AsyncFd::write_vectored`] and [`AsyncFd::write_vectored_at`].
    pub struct WriteVectored<B: BufSlice<N>; const N: usize>(sys::io::WriteVectoredOp<B, N>) -> io::Result<usize>,
      impl Extract -> io::Result<(B, usize)>;

    /// [`Future`] behind [`AsyncFd::splice_to`], [`AsyncFd::splice_to_at`],
    /// [`AsyncFd::splice_from`] and [`AsyncFd::splice_from_at`].
    pub struct Splice(sys::io::SpliceOp) -> io::Result<usize>;
);

/// [`Future`] behind [`AsyncFd::read_n`] and [`AsyncFd::read_n_at`].
#[derive(Debug)]
pub struct ReadN<'fd, B: BufMut, D: Descriptor = File> {
    read: Read<'fd, ReadNBuf<B>, D>,
    offset: u64,
    /// Number of bytes we still need to read to hit our minimum.
    left: usize,
}

impl<'fd, B: BufMut, D: Descriptor> Cancel for ReadN<'fd, B, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.read.try_cancel()
    }

    fn cancel(&mut self) -> CancelOperation {
        self.read.cancel()
    }
}

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

/// [`Future`] behind [`AsyncFd::read_n_vectored`] and [`AsyncFd::read_n_vectored_at`].
#[derive(Debug)]
pub struct ReadNVectored<'fd, B: BufMutSlice<N>, const N: usize, D: Descriptor = File> {
    read: ReadVectored<'fd, ReadNBuf<B>, N, D>,
    offset: u64,
    /// Number of bytes we still need to read to hit our minimum.
    left: usize,
}

impl<'fd, B: BufMutSlice<N>, const N: usize, D: Descriptor> Cancel for ReadNVectored<'fd, B, N, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.read.try_cancel()
    }

    fn cancel(&mut self) -> CancelOperation {
        self.read.cancel()
    }
}

impl<'fd, B: BufMutSlice<N>, const N: usize, D: Descriptor> Future for ReadNVectored<'fd, B, N, D> {
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

                read.set(read.0.fd().read_vectored_at(bufs, this.offset));
                unsafe { Pin::new_unchecked(this) }.poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// [`Future`] behind [`AsyncFd::write_all`] and [`AsyncFd::write_all_at`].
#[derive(Debug)]
pub struct WriteAll<'fd, B: Buf, D: Descriptor = File> {
    write: Extractor<Write<'fd, SkipBuf<B>, D>>,
    offset: u64,
}

impl<'fd, B: Buf, D: Descriptor> WriteAll<'fd, B, D> {
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

                write.set(write.fut.0.fd().write_at(buf, this.offset).extract());
                unsafe { Pin::new_unchecked(this) }.poll_inner(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'fd, B: Buf, D: Descriptor> Cancel for WriteAll<'fd, B, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.write.try_cancel()
    }

    fn cancel(&mut self) -> CancelOperation {
        self.write.cancel()
    }
}

impl<'fd, B: Buf, D: Descriptor> Future for WriteAll<'fd, B, D> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.poll_inner(ctx).map_ok(|_| ())
    }
}

impl<'fd, B: Buf, D: Descriptor> Extract for WriteAll<'fd, B, D> {}

impl<'fd, B: Buf, D: Descriptor> Future for Extractor<WriteAll<'fd, B, D>> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `self.fut` (`s.fut`), directly called
        // `Future::poll` on it.
        unsafe { Pin::map_unchecked_mut(self, |s| &mut s.fut) }.poll_inner(ctx)
    }
}

/// [`Future`] behind [`AsyncFd::write_all_vectored`].
#[derive(Debug)]
pub struct WriteAllVectored<'fd, B: BufSlice<N>, const N: usize, D: Descriptor = File> {
    write: Extractor<WriteVectored<'fd, B, N, D>>,
    offset: u64,
    skip: u64,
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> WriteAllVectored<'fd, B, N, D> {
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
                        iovec.set_len(0);
                    } else {
                        iovec.set_len(skip as usize);
                        break;
                    }
                }

                if iovecs[N - 1].len() == 0 {
                    // Written everything.
                    return Poll::Ready(Ok(bufs));
                }

                write.set(
                    WriteVectored(FdOperation::new(
                        write.fut.0.fd(),
                        (bufs, iovecs),
                        this.offset,
                    ))
                    .extract(),
                );
                unsafe { Pin::new_unchecked(this) }.poll_inner(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> Cancel for WriteAllVectored<'fd, B, N, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.write.try_cancel()
    }

    fn cancel(&mut self) -> CancelOperation {
        self.write.cancel()
    }
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> Future for WriteAllVectored<'fd, B, N, D> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.poll_inner(ctx).map_ok(|_| ())
    }
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> Extract
    for WriteAllVectored<'fd, B, N, D>
{
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> Future
    for Extractor<WriteAllVectored<'fd, B, N, D>>
{
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        unsafe { Pin::map_unchecked_mut(self, |s| &mut s.fut) }.poll_inner(ctx)
    }
}

operation!(
    /// [`Future`] behind [`AsyncFd::close`].
    pub struct Close<D: Descriptor>(sys::io::CloseOp<D>) -> io::Result<()>;
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

/// Wrapper around a buffer `B` to skip a number of bytes.
#[derive(Debug)]
struct SkipBuf<B> {
    buf: B,
    skip: u32,
}

unsafe impl<B: Buf> Buf for SkipBuf<B> {
    unsafe fn parts(&self) -> (*const u8, u32) {
        let (ptr, size) = self.buf.parts();
        if self.skip >= size {
            (ptr, 0)
        } else {
            (ptr.add(self.skip as usize), size - self.skip)
        }
    }
}

/// Wrapper around a buffer `B` to implement [`DropWake`] on.
///
/// [`DropWake`]: crate::drop_waker::DropWake
#[derive(Debug)]
pub(crate) struct Buffer<B> {
    pub(crate) buf: B,
}
