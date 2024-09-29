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

use crate::io_uring::cancel::{Cancel, CancelOp, CancelResult};
use crate::io_uring::extract::{Extract, Extractor};
use crate::io_uring::fd::{AsyncFd, Descriptor, File};
use crate::io_uring::op::{op_future, poll_state, OpState, NO_OFFSET};
use crate::io_uring::{libc, SubmissionQueue};
use crate::man_link;

mod read_buf;
#[doc(hidden)]
pub use read_buf::{BufGroupId, BufIdx};
pub use read_buf::{ReadBuf, ReadBufPool};

macro_rules! stdio {
    (
        $fn: ident () -> $name: ident, $fd: expr
    ) => {
        #[doc = concat!("Create a new `", stringify!($name), "`.\n\n")]
        pub fn $fn(sq: $crate::io_uring::SubmissionQueue) -> $name {
            unsafe { $name(std::mem::ManuallyDrop::new($crate::io_uring::fd::AsyncFd::from_raw_fd($fd, sq))) }
        }

        #[doc = concat!(
            "An [`AsyncFd`] for ", stringify!($fn), ".\n\n",
            "# Notes\n\n",
            "This directly writes to the raw file descriptor, which means it's not buffered and will not flush anything buffered by the standard library.\n\n",
            "When this type is dropped it will not close ", stringify!($fn), ".",
        )]
        pub struct $name(std::mem::ManuallyDrop<$crate::io_uring::fd::AsyncFd>);

        impl std::ops::Deref for $name {
            type Target = $crate::io_uring::fd::AsyncFd;

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
    #[doc = man_link!(splice(2))]
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
    #[doc = man_link!(close(2))]
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
    drop_using: Box,
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
    drop_using: Box,
    /// `iovecs` can't move until the kernel has read the submission.
    impl !Unpin,
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
    drop_using: Box,
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
    drop_using: Box,
    /// `iovecs` can't move until the kernel has read the submission.
    impl !Unpin,
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
