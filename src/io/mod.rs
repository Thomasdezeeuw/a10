//! Type definitions for I/O functionality.
//!
//! The main types of this module are the [`Buf`] and [`BufMut`] traits, which
//! define the requirements on buffers using the I/O system calls on an file
//! descriptor ([`AsyncFd`]). Additionally the [`BufSlice`] and [`BufMutSlice`]
//! traits existing to define the behaviour of buffers in vectored I/O.
//!
//! A specialised read buffer pool implementation exists in [`ReadBufPool`],
//! which is a buffer pool managed by the kernel when making `read(2)`-like
//! system calls.
//!
//! Finally we have the [`stdin`], [`stdout`] and [`stderr`] functions to create
//! `AsyncFd`s for standard in, out and error respectively.

// This is not ideal.
// This should only be applied to `ReadVectored` and `WriteVectored` as they use
// `libc::iovec` internally, which is `!Send`, while it actually is `Send`.
#![allow(clippy::non_send_fields_in_send_ty)]

use std::future::Future;
use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{io, ptr};

use crate::cancel::{Cancel, CancelOp, CancelResult};
use crate::extract::{Extract, Extractor};
use crate::fd::{AsyncFd, Descriptor, File};
use crate::op::{op_future, poll_state, OpState, NO_OFFSET};
use crate::{libc, SubmissionQueue};

mod read_buf;
#[doc(hidden)]
pub use read_buf::{BufGroupId, BufIdx};
pub use read_buf::{ReadBuf, ReadBufPool};

// Re-export so we don't have to worry about import `std::io` and `crate::io`.
pub(crate) use std::io::*;

macro_rules! stdio {
    (
        $fn: ident () -> $name: ident, $fd: expr
    ) => {
        #[doc = concat!("Create a new `", stringify!($name), "`.\n\n")]
        pub fn $fn(sq: $crate::SubmissionQueue) -> $name {
            unsafe { $name(std::mem::ManuallyDrop::new($crate::AsyncFd::from_raw_fd($fd, sq))) }
        }

        #[doc = concat!(
            "An [`AsyncFd`] for ", stringify!($fn), ".\n\n",
            "# Notes\n\n",
            "This directly writes to the raw file descriptor, which means it's not buffered and will not flush anything buffered by the standard library.\n\n",
            "When this type is dropped it will not close ", stringify!($fn), ".",
        )]
        pub struct $name(std::mem::ManuallyDrop<$crate::AsyncFd>);

        impl std::ops::Deref for $name {
            type Target = $crate::AsyncFd;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name))
                    .field("fd", &*self.0)
                    .finish()
            }
        }

        impl std::ops::Drop for $name {
            fn drop(&mut self) {
                // We don't want to close the file descriptor, but we do need to
                // drop our reference to the submission queue.
                // SAFETY: with `ManuallyDrop` we don't drop the `AsyncFd` so
                // it's not dropped twice. Otherwise we get access to it using
                // safe methods.
                unsafe { std::ptr::drop_in_place(&mut self.0.sq) };
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
    pub const fn read_at<'fd, B>(&'fd self, buf: B, offset: u64) -> Read<'fd, B, D>
    where
        B: BufMut,
    {
        Read::new(self, buf, offset)
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
        ReadN::new(self, buf, offset, n)
    }

    /// Read from this fd into `bufs`.
    pub fn read_vectored<'fd, B, const N: usize>(&'fd self, bufs: B) -> ReadVectored<'fd, B, N, D>
    where
        B: BufMutSlice<N>,
    {
        self.read_vectored_at(bufs, NO_OFFSET)
    }

    /// Read from this fd into `bufs` starting at `offset`.
    ///
    /// The current file cursor is not affected by this function.
    pub fn read_vectored_at<'fd, B, const N: usize>(
        &'fd self,
        mut bufs: B,
        offset: u64,
    ) -> ReadVectored<'fd, B, N, D>
    where
        B: BufMutSlice<N>,
    {
        let iovecs = unsafe { bufs.as_iovecs_mut() };
        ReadVectored::new(self, bufs, iovecs, offset)
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
        ReadNVectored::new(self, bufs, offset, n)
    }

    /// Write `buf` to this fd.
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
        Write::new(self, buf, offset)
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
        WriteAll::new(self, buf, offset)
    }

    /// Write `bufs` to this file.
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
        WriteVectored::new(self, bufs, iovecs, offset)
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
        WriteAllVectored::new(self, bufs, offset)
    }

    /// Splice `length` bytes to `target` fd.
    ///
    /// See the `splice(2)` manual for correct usage.
    #[doc(alias = "splice")]
    pub const fn splice_to<'fd>(
        &'fd self,
        target: RawFd,
        length: u32,
        flags: libc::c_int,
    ) -> Splice<'fd, D> {
        self.splice_to_at(NO_OFFSET, target, NO_OFFSET, length, flags)
    }

    /// Same as [`AsyncFd::splice_to`], but starts reading data at `offset` from
    /// the file (instead of the current position of the read cursor) and starts
    /// writing at `target_offset` to `target`.
    pub const fn splice_to_at<'fd>(
        &'fd self,
        offset: u64,
        target: RawFd,
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
    pub const fn splice_from<'fd>(
        &'fd self,
        target: RawFd,
        length: u32,
        flags: libc::c_int,
    ) -> Splice<'fd, D> {
        self.splice_from_at(NO_OFFSET, target, NO_OFFSET, length, flags)
    }

    /// Same as [`AsyncFd::splice_from`], but starts reading writing at `offset`
    /// to the file (instead of the current position of the write cursor) and
    /// starts reading at `target_offset` from `target`.
    #[doc(alias = "splice")]
    pub const fn splice_from_at<'fd>(
        &'fd self,
        offset: u64,
        target: RawFd,
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

    const fn splice<'fd>(
        &'fd self,
        target: RawFd,
        direction: SpliceDirection,
        off_in: u64,
        off_out: u64,
        length: u32,
        flags: libc::c_int,
    ) -> Splice<'fd, D> {
        Splice::new(self, (target, direction, off_in, off_out, length, flags))
    }

    /// Explicitly close the file descriptor.
    ///
    /// # Notes
    ///
    /// This happens automatically on drop, this can be used to get a possible
    /// error.
    pub fn close(self) -> Close<D> {
        // We deconstruct `self` without dropping it to avoid closing the fd
        // twice.
        let this = ManuallyDrop::new(self);
        // SAFETY: this is safe because we're ensure the pointers are valid and
        // not touching `this` after reading the fields.
        let fd = this.fd();
        let sq = unsafe { ptr::read(&this.sq) };

        Close {
            sq,
            state: OpState::NotStarted(fd),
            kind: PhantomData,
        }
    }
}

// Read.
op_future! {
    fn AsyncFd::read -> B,
    struct Read<'fd, B: BufMut> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
    },
    setup_state: offset: u64,
    setup: |submission, fd, (buf,), offset| unsafe {
        let (ptr, len) = buf.parts_mut();
        submission.read_at(fd.fd(), ptr, len, offset);
        if let Some(buf_group) = buf.buffer_group() {
            submission.set_buffer_select(buf_group.0);
        }
    },
    map_result: |this, (mut buf,), buf_idx, n| {
        // SAFETY: the kernel initialised the bytes for us as part of the read
        // call.
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        unsafe { buf.buffer_init(BufIdx(buf_idx), n as u32) };
        Ok(buf)
    },
}

/// [`Future`] behind [`AsyncFd::read_n`].
#[derive(Debug)]
pub struct ReadN<'fd, B, D: Descriptor = File> {
    read: Read<'fd, ReadNBuf<B>, D>,
    offset: u64,
    /// Number of bytes we still need to read to hit our target `N`.
    left: usize,
}

impl<'fd, B: BufMut, D: Descriptor> ReadN<'fd, B, D> {
    const fn new(fd: &'fd AsyncFd<D>, buf: B, offset: u64, n: usize) -> ReadN<'fd, B, D> {
        let buf = ReadNBuf { buf, last_read: 0 };
        ReadN {
            read: fd.read_at(buf, offset),
            offset,
            left: n,
        }
    }
}

impl<'fd, B, D: Descriptor> Cancel for ReadN<'fd, B, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.read.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
        self.read.cancel()
    }
}

impl<'fd, B: BufMut, D: Descriptor> Future for ReadN<'fd, B, D> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `Future`.
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

                read.set(read.fd.read_at(buf, this.offset));
                unsafe { Pin::new_unchecked(this) }.poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

// ReadVectored.
op_future! {
    fn AsyncFd::read_vectored -> B,
    struct ReadVectored<'fd, B: BufMutSlice<N>; const N: usize> {
        /// Buffers to write into, needs to stay in memory so the kernel can
        /// access it safely.
        bufs: B,
        /// Buffer references used by the kernel.
        ///
        /// NOTE: we only need these iovecs in the submission, we don't have to
        /// keep around during the operation. Because of this we don't heap
        /// allocate it like we for other operations. This leaves a small
        /// duration between the submission of the entry and the submission
        /// being read by the kernel in which this future could be dropped and
        /// the kernel will read memory we don't own. However because we wake
        /// the kernel after submitting the timeout entry it's not really worth
        /// to heap allocation.
        iovecs: [libc::iovec; N],
    },
    /// `iovecs` can't move until the kernel has read the submission.
    impl !Upin,
    setup_state: offset: u64,
    setup: |submission, fd, (_bufs, iovecs), offset| unsafe {
        submission.read_vectored_at(fd.fd(), iovecs, offset);
    },
    map_result: |this, (mut bufs, _iovecs), _flags, n| {
        // SAFETY: the kernel initialised the bytes for us as part of the read
        // call.
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        unsafe { bufs.set_init(n as usize) };
        Ok(bufs)
    },
}

/// [`Future`] behind [`AsyncFd::read_n_vectored`].
#[derive(Debug)]
pub struct ReadNVectored<'fd, B, const N: usize, D: Descriptor = File> {
    read: ReadVectored<'fd, ReadNBuf<B>, N, D>,
    offset: u64,
    /// Number of bytes we still need to read to hit our target `N`.
    left: usize,
}

impl<'fd, B: BufMutSlice<N>, const N: usize, D: Descriptor> ReadNVectored<'fd, B, N, D> {
    fn new(fd: &'fd AsyncFd<D>, bufs: B, offset: u64, n: usize) -> ReadNVectored<'fd, B, N, D> {
        let bufs = ReadNBuf {
            buf: bufs,
            last_read: 0,
        };
        ReadNVectored {
            read: fd.read_vectored_at(bufs, offset),
            offset,
            left: n,
        }
    }
}

impl<'fd, B, const N: usize, D: Descriptor> Cancel for ReadNVectored<'fd, B, N, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.read.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
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

                read.set(read.fd.read_vectored_at(bufs, this.offset));
                unsafe { Pin::new_unchecked(this) }.poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Wrapper around a buffer `B` to keep track of the number of bytes written,
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
}

unsafe impl<B: BufMutSlice<N>, const N: usize> BufMutSlice<N> for ReadNBuf<B> {
    unsafe fn as_iovecs_mut(&mut self) -> [libc::iovec; N] {
        self.buf.as_iovecs_mut()
    }

    unsafe fn set_init(&mut self, n: usize) {
        self.last_read = n;
        self.buf.set_init(n);
    }
}

// Write.
op_future! {
    fn AsyncFd::write -> usize,
    struct Write<'fd, B: Buf> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
    },
    setup_state: offset: u64,
    setup: |submission, fd, (buf,), offset| unsafe {
        let (ptr, len) = buf.parts();
        submission.write_at(fd.fd(), ptr, len, offset);
    },
    map_result: |n| {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok(n as usize)
    },
    extract: |this, (buf,), n| -> (B, usize) {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok((buf, n as usize))
    },
}

/// [`Future`] behind [`AsyncFd::write_all`].
#[derive(Debug)]
pub struct WriteAll<'fd, B, D: Descriptor = File> {
    write: Extractor<Write<'fd, SkipBuf<B>, D>>,
    offset: u64,
}

impl<'fd, B: Buf, D: Descriptor> WriteAll<'fd, B, D> {
    const fn new(fd: &'fd AsyncFd<D>, buf: B, offset: u64) -> WriteAll<'fd, B, D> {
        let buf = SkipBuf { buf, skip: 0 };
        WriteAll {
            // TODO: once `Extract` is a constant trait use that.
            write: Extractor {
                fut: fd.write_at(buf, offset),
            },
            offset,
        }
    }

    /// Poll implementation used by the [`Future`] implement for the naked type
    /// and the type wrapper in an [`Extractor`].
    fn inner_poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<io::Result<B>> {
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
                unsafe { Pin::new_unchecked(this) }.inner_poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'fd, B, D: Descriptor> Cancel for WriteAll<'fd, B, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.write.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
        self.write.cancel()
    }
}

impl<'fd, B: Buf, D: Descriptor> Future for WriteAll<'fd, B, D> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.inner_poll(ctx).map_ok(|_| ())
    }
}

impl<'fd, B: Buf, D: Descriptor> Extract for WriteAll<'fd, B, D> {}

impl<'fd, B: Buf, D: Descriptor> Future for Extractor<WriteAll<'fd, B, D>> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        unsafe { Pin::map_unchecked_mut(self, |s| &mut s.fut) }.inner_poll(ctx)
    }
}

/// Wrapper around a buffer `B` to skip a number of bytes.
#[derive(Debug)]
pub(crate) struct SkipBuf<B> {
    pub(crate) buf: B,
    pub(crate) skip: u32,
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

// WriteVectored.
op_future! {
    fn AsyncFd::write_vectored -> usize,
    struct WriteVectored<'fd, B: BufSlice<N>; const N: usize> {
        /// Buffers to read from, needs to stay in memory so the kernel can
        /// access it safely.
        bufs: B,
        /// Buffer references used by the kernel.
        ///
        /// NOTE: we only need these iovecs in the submission, we don't have to
        /// keep around during the operation. Because of this we don't heap
        /// allocate it like we for other operations. This leaves a small
        /// duration between the submission of the entry and the submission
        /// being read by the kernel in which this future could be dropped and
        /// the kernel will read memory we don't own. However because we wake
        /// the kernel after submitting the timeout entry it's not really worth
        /// to heap allocation.
        iovecs: [libc::iovec; N],
    },
    /// `iovecs` can't move until the kernel has read the submission.
    impl !Upin,
    setup_state: offset: u64,
    setup: |submission, fd, (_bufs, iovecs), offset| unsafe {
        submission.write_vectored_at(fd.fd(), iovecs, offset);
    },
    map_result: |n| {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok(n as usize)
    },
    extract: |this, (buf, _iovecs), n| -> (B, usize) {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok((buf, n as usize))
    },
}

/// [`Future`] behind [`AsyncFd::write_all_vectored`].
#[derive(Debug)]
pub struct WriteAllVectored<'fd, B, const N: usize, D: Descriptor = File> {
    write: Extractor<WriteVectored<'fd, B, N, D>>,
    offset: u64,
    skip: u64,
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> WriteAllVectored<'fd, B, N, D> {
    fn new(fd: &'fd AsyncFd<D>, buf: B, offset: u64) -> WriteAllVectored<'fd, B, N, D> {
        WriteAllVectored {
            write: fd.write_vectored_at(buf, offset).extract(),
            offset,
            skip: 0,
        }
    }

    /// Poll implementation used by the [`Future`] implement for the naked type
    /// and the type wrapper in an [`Extractor`].
    fn inner_poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<io::Result<B>> {
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
                    if iovec.iov_len as u64 <= skip {
                        // Skip entire buf.
                        skip -= iovec.iov_len as u64;
                        iovec.iov_len = 0;
                    } else {
                        iovec.iov_len -= skip as usize;
                        break;
                    }
                }

                if iovecs[N - 1].iov_len == 0 {
                    // Written everything.
                    return Poll::Ready(Ok(bufs));
                }

                write.set(WriteVectored::new(write.fut.fd, bufs, iovecs, this.offset).extract());
                unsafe { Pin::new_unchecked(this) }.inner_poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'fd, B, const N: usize, D: Descriptor> Cancel for WriteAllVectored<'fd, B, N, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.write.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
        self.write.cancel()
    }
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> Future for WriteAllVectored<'fd, B, N, D> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.inner_poll(ctx).map_ok(|_| ())
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
        unsafe { Pin::map_unchecked_mut(self, |s| &mut s.fut) }.inner_poll(ctx)
    }
}

// Splice.
op_future! {
    fn AsyncFd::splice_to -> usize,
    struct Splice<'fd> {
        // Doesn't need any fields.
    },
    setup_state: flags: (RawFd, SpliceDirection, u64, u64, u32, libc::c_int),
    setup: |submission, fd, (), (target, direction, off_in, off_out, len, flags)| unsafe {
        let (fd_in, fd_out) = match direction {
            SpliceDirection::To => (fd.fd(), target),
            SpliceDirection::From => (target, fd.fd()),
        };
        submission.splice(fd_in, off_in, fd_out, off_out, len, flags);
    },
    map_result: |n| {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok(n as usize)
    },
}

#[derive(Copy, Clone, Debug)]
enum SpliceDirection {
    To,
    From,
}

/// [`Future`] behind [`AsyncFd::close`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct Close<D: Descriptor = File> {
    sq: SubmissionQueue,
    state: OpState<RawFd>,
    kind: PhantomData<D>,
}

impl<D: Descriptor + Unpin> Future for Close<D> {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let op_index = poll_state!(Close, self.state, self.sq, ctx, |submission, fd| unsafe {
            submission.close(fd);
            D::use_flags(submission);
        });

        match self.sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok(_) => Poll::Ready(Ok(())),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

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
    fn buffer_group(&self) -> Option<BufGroupId> {
        None
    }

    /// Mark `n` bytes as initialised in buffer with `idx`.
    ///
    /// Don't implement this.
    #[doc(hidden)]
    unsafe fn buffer_init(&mut self, idx: BufIdx, n: u32) {
        debug_assert!(idx.0 == 0);
        self.set_init(n as usize);
    }
}

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
    /// Returns the writable buffers as `iovec` structures.
    ///
    /// # Safety
    ///
    /// This has the same safety requirements as [`BufMut::parts_mut`], but then
    /// for all buffers used.
    unsafe fn as_iovecs_mut(&mut self) -> [libc::iovec; N];

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

// SAFETY: `BufMutSlice` has the same safety requirements as `BufMut` and since
// `B` implements `BufMut` it's safe to implement `BufMutSlice` for an array of
// `B`.
unsafe impl<B: BufMut, const N: usize> BufMutSlice<N> for [B; N] {
    unsafe fn as_iovecs_mut(&mut self) -> [libc::iovec; N] {
        // TODO: replace with `MaybeUninit::uninit_array` once stable.
        // SAFETY: an uninitialised `MaybeUninit` is valid.
        let mut iovecs =
            unsafe { MaybeUninit::<[MaybeUninit<libc::iovec>; N]>::uninit().assume_init() };
        for (buf, iovec) in self.iter_mut().zip(iovecs.iter_mut()) {
            debug_assert!(
                buf.buffer_group().is_none(),
                "can't use a10::ReadBuf as a10::BufMutSlice in vectored I/O"
            );
            let (ptr, len) = buf.parts_mut();
            iovec.write(libc::iovec {
                iov_base: ptr.cast(),
                iov_len: len as _,
            });
        }
        // TODO: replace with `MaybeUninit::array_assume_init` once stable.
        // SAFETY: `MaybeUninit<libc::iovec>` and `iovec` have the same layout
        // as guaranteed by `MaybeUninit`.
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
    /// Returns the reabable buffer as `iovec` structures.
    ///
    /// # Safety
    ///
    /// This has the same safety requirements as [`Buf::parts`], but then for
    /// all buffers used.
    unsafe fn as_iovecs(&self) -> [libc::iovec; N];
}

// SAFETY: `BufSlice` has the same safety requirements as `Buf` and since `B`
// implements `Buf` it's safe to implement `BufSlice` for an array of `B`.
unsafe impl<B: Buf, const N: usize> BufSlice<N> for [B; N] {
    unsafe fn as_iovecs(&self) -> [libc::iovec; N] {
        // TODO: replace with `MaybeUninit::uninit_array` once stable.
        // SAFETY: an uninitialised `MaybeUninit` is valid.
        let mut iovecs =
            unsafe { MaybeUninit::<[MaybeUninit<libc::iovec>; N]>::uninit().assume_init() };
        for (buf, iovec) in self.iter().zip(iovecs.iter_mut()) {
            let (ptr, len) = buf.parts();
            iovec.write(libc::iovec {
                iov_base: ptr as _,
                iov_len: len as _,
            });
        }
        // TODO: replace with `MaybeUninit::array_assume_init` once stable.
        // SAFETY: `MaybeUninit<libc::iovec>` and `iovec` have the same layout
        // as guaranteed by `MaybeUninit`.
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
            unsafe fn as_iovecs_mut(&mut self) -> [libc::iovec; $N] {
                [
                    $({
                        debug_assert!(
                            self.$index.buffer_group().is_none(),
                            "can't use a10::ReadBuf as a10::BufMutSlice in vectored I/O"
                        );
                        let (ptr, len) = self.$index.parts_mut();
                        libc::iovec {
                            iov_base: ptr.cast(),
                            iov_len: len as _,
                        }
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
            unsafe fn as_iovecs(&self) -> [libc::iovec; $N] {
                [
                    $({
                        let (ptr, len) = self.$index.parts();
                        libc::iovec {
                            iov_base: ptr as _,
                            iov_len: len as _,
                        }
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
