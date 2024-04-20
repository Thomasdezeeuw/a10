//! Asynchronous networking.
//!
//! To create a new socket ([`AsyncFd`]) use the [`socket`] function, which
//! issues a non-blocking `socket(2)` call.

// This is not ideal.
// This should only be applied to `SendMsg` and `RecvVectored` `RecvFrom` and
// `RecvFromVectored` as they use `libc::iovec` internally, which is `!Send`,
// while it actually is `Send`.
#![allow(clippy::non_send_fields_in_send_ty)]

use std::future::Future;
use std::marker::PhantomData;
use std::mem::{self, size_of, MaybeUninit};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{io, ptr};

use crate::cancel::{Cancel, CancelOp, CancelResult};
use crate::extract::{Extract, Extractor};
use crate::fd::{AsyncFd, Descriptor, File};
use crate::io::{
    Buf, BufIdx, BufMut, BufMutSlice, BufSlice, ReadBuf, ReadBufPool, ReadNBuf, SkipBuf,
};
use crate::op::{op_async_iter, op_future, poll_state, OpState};
use crate::{libc, SubmissionQueue};

/// Creates a new socket.
///
/// See the `socket(2)` manual for more information.
pub const fn socket<D: Descriptor>(
    sq: SubmissionQueue,
    domain: libc::c_int,
    r#type: libc::c_int,
    protocol: libc::c_int,
    flags: libc::c_int,
) -> Socket<D> {
    Socket {
        sq: Some(sq),
        state: OpState::NotStarted((domain, r#type, protocol, flags)),
        kind: PhantomData,
    }
}

/// Socket related system calls.
impl<D: Descriptor> AsyncFd<D> {
    /// Initiate a connection on this socket to the specified address.
    pub fn connect<'fd, A>(&'fd self, address: impl Into<Box<A>>) -> Connect<'fd, A, D>
    where
        A: SocketAddress,
    {
        let address = address.into();
        Connect::new(self, address, ())
    }

    /// Sends data on the socket to a connected peer.
    pub const fn send<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Send<'fd, B, D>
    where
        B: Buf,
    {
        Send::new(self, buf, (libc::IORING_OP_SEND as u8, flags))
    }

    /// Same as [`AsyncFd::send`], but tries to avoid making intermediate copies
    /// of `buf`.
    ///
    /// # Notes
    ///
    /// Zerocopy execution is not guaranteed and may fall back to copying. The
    /// request may also fail with `EOPNOTSUPP`, when a protocol doesn't support
    /// zerocopy, in which case users are recommended to use [`AsyncFd::send`]
    /// instead.
    ///
    /// The `Future` only returns once it safe for the buffer to be used again,
    /// for TCP for example this means until the data is ACKed by the peer.
    pub const fn send_zc<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Send<'fd, B, D>
    where
        B: Buf,
    {
        Send::new(self, buf, (libc::IORING_OP_SEND_ZC as u8, flags))
    }

    /// Sends all data in `buf` on the socket to a connected peer.
    /// Returns [`io::ErrorKind::WriteZero`] if not all bytes could be written.
    pub const fn send_all<'fd, B>(&'fd self, buf: B) -> SendAll<'fd, B, D>
    where
        B: Buf,
    {
        SendAll::new(self, buf)
    }

    /// Sends data in `bufs` on the socket to a connected peer.
    pub fn send_vectored<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
        flags: libc::c_int,
    ) -> SendMsg<'fd, B, NoAddress, N, D>
    where
        B: BufSlice<N>,
    {
        self.sendmsg(libc::IORING_OP_SENDMSG as u8, bufs, NoAddress, flags)
    }

    /// Same as [`AsyncFd::send_vectored`], but tries to avoid making
    /// intermediate copies of `buf`.
    pub fn send_vectored_zc<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
        flags: libc::c_int,
    ) -> SendMsg<'fd, B, NoAddress, N, D>
    where
        B: BufSlice<N>,
    {
        self.sendmsg(libc::IORING_OP_SENDMSG_ZC as u8, bufs, NoAddress, flags)
    }

    /// Sends all data in `bufs` on the socket to a connected peer, using
    /// vectored I/O.
    /// Returns [`io::ErrorKind::WriteZero`] if not all bytes could be written.
    pub fn send_all_vectored<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
    ) -> SendAllVectored<'fd, B, N, D>
    where
        B: BufSlice<N>,
    {
        SendAllVectored::new(self, bufs)
    }

    /// Sends data on the socket to a connected peer.
    pub const fn sendto<'fd, B, A>(
        &'fd self,
        buf: B,
        address: A,
        flags: libc::c_int,
    ) -> SendTo<'fd, B, A, D>
    where
        B: Buf,
        A: SocketAddress,
    {
        SendTo::new(self, buf, address, (libc::IORING_OP_SEND as u8, flags))
    }

    /// Same as [`AsyncFd::sendto`], but tries to avoid making intermediate copies
    /// of `buf`.
    ///
    /// See [`AsyncFd::send_zc`] for additional notes.
    pub const fn sendto_zc<'fd, B, A>(
        &'fd self,
        buf: B,
        address: A,
        flags: libc::c_int,
    ) -> SendTo<'fd, B, A, D>
    where
        B: Buf,
        A: SocketAddress,
    {
        SendTo::new(self, buf, address, (libc::IORING_OP_SEND_ZC as u8, flags))
    }

    /// Sends data in `bufs` on the socket to a connected peer.
    pub fn sendto_vectored<'fd, B, A, const N: usize>(
        &'fd self,
        bufs: B,
        address: A,
        flags: libc::c_int,
    ) -> SendMsg<'fd, B, A, N, D>
    where
        B: BufSlice<N>,
        A: SocketAddress,
    {
        self.sendmsg(libc::IORING_OP_SENDMSG as u8, bufs, address, flags)
    }

    /// Same as [`AsyncFd::sendto_vectored`], but tries to avoid making
    /// intermediate copies of `buf`.
    pub fn sendto_vectored_zc<'fd, B, A, const N: usize>(
        &'fd self,
        bufs: B,
        address: A,
        flags: libc::c_int,
    ) -> SendMsg<'fd, B, A, N, D>
    where
        B: BufSlice<N>,
        A: SocketAddress,
    {
        self.sendmsg(libc::IORING_OP_SENDMSG_ZC as u8, bufs, address, flags)
    }

    fn sendmsg<'fd, B, A, const N: usize>(
        &'fd self,
        op: u8,
        bufs: B,
        address: A,
        flags: libc::c_int,
    ) -> SendMsg<'fd, B, A, N, D>
    where
        B: BufSlice<N>,
        A: SocketAddress,
    {
        // SAFETY: zeroed `msghdr` is valid.
        let msg = unsafe { mem::zeroed() };
        let iovecs = unsafe { bufs.as_iovecs() };
        SendMsg::new(self, bufs, address, msg, iovecs, (op, flags))
    }

    /// Receives data on the socket from the remote address to which it is
    /// connected.
    pub const fn recv<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Recv<'fd, B, D>
    where
        B: BufMut,
    {
        Recv::new(self, buf, flags)
    }

    /// Continuously receive data on the socket from the remote address to which
    /// it is connected.
    ///
    /// # Notes
    ///
    /// This will return `ENOBUFS` if no buffer is available in the `pool` to
    /// read into.
    ///
    /// Be careful when using this as a peer sending a lot data might take up
    /// all your buffers from your pool!
    pub const fn multishot_recv<'fd>(
        &'fd self,
        pool: ReadBufPool,
        flags: libc::c_int,
    ) -> MultishotRecv<'fd, D> {
        MultishotRecv::new(self, pool, flags)
    }

    /// Receives at least `n` bytes on the socket from the remote address to
    /// which it is connected.
    pub const fn recv_n<'fd, B>(&'fd self, buf: B, n: usize) -> RecvN<'fd, B, D>
    where
        B: BufMut,
    {
        RecvN::new(self, buf, n)
    }

    /// Receives data on the socket from the remote address to which it is
    /// connected, using vectored I/O.
    pub fn recv_vectored<'fd, B, const N: usize>(
        &'fd self,
        mut bufs: B,
        flags: libc::c_int,
    ) -> RecvVectored<'fd, B, N, D>
    where
        B: BufMutSlice<N>,
    {
        // TODO: replace with `Box::new_zeroed` once `new_uninit` is stable.
        // SAFETY: zeroed `msghdr` is valid.
        let msg = unsafe { Box::new(mem::zeroed()) };
        let iovecs = unsafe { bufs.as_iovecs_mut() };
        RecvVectored::new(self, bufs, msg, iovecs, flags)
    }

    /// Receives at least `n` bytes on the socket from the remote address to
    /// which it is connected, using vectored I/O.
    pub fn recv_n_vectored<'fd, B, const N: usize>(
        &'fd self,
        bufs: B,
        n: usize,
    ) -> RecvNVectored<'fd, B, N, D>
    where
        B: BufMutSlice<N>,
    {
        RecvNVectored::new(self, bufs, n)
    }

    /// Receives data on the socket and returns the source address.
    pub fn recvfrom<'fd, B, A>(&'fd self, mut buf: B, flags: libc::c_int) -> RecvFrom<'fd, B, A, D>
    where
        B: BufMut,
        A: SocketAddress,
    {
        // SAFETY: zeroed `msghdr` is valid.
        let msg = unsafe { mem::zeroed() };
        let (buf_ptr, buf_len) = unsafe { buf.parts_mut() };
        let iovec = libc::iovec {
            iov_base: buf_ptr.cast(),
            iov_len: buf_len as _,
        };
        let msg = Box::new((msg, MaybeUninit::uninit()));
        RecvFrom::new(self, buf, msg, iovec, flags)
    }

    /// Receives data on the socket and the source address using vectored I/O.
    pub fn recvfrom_vectored<'fd, B, A, const N: usize>(
        &'fd self,
        mut bufs: B,
        flags: libc::c_int,
    ) -> RecvFromVectored<'fd, B, A, N, D>
    where
        B: BufMutSlice<N>,
        A: SocketAddress,
    {
        // SAFETY: zeroed `msghdr` is valid.
        let msg = unsafe { mem::zeroed() };
        let iovecs = unsafe { bufs.as_iovecs_mut() };
        let msg = Box::new((msg, MaybeUninit::uninit()));
        RecvFromVectored::new(self, bufs, msg, iovecs, flags)
    }

    /// Shuts down the read, write, or both halves of this connection.
    pub const fn shutdown<'fd>(&'fd self, how: std::net::Shutdown) -> Shutdown<'fd, D> {
        let how = match how {
            std::net::Shutdown::Read => libc::SHUT_RD,
            std::net::Shutdown::Write => libc::SHUT_WR,
            std::net::Shutdown::Both => libc::SHUT_RDWR,
        };
        Shutdown::new(self, how)
    }

    /// Accept a new socket stream ([`AsyncFd`]).
    ///
    /// If an accepted stream is returned, the remote address of the peer is
    /// returned along with it.
    pub fn accept<'fd, A>(&'fd self) -> Accept<'fd, A, D> {
        self.accept4(libc::SOCK_CLOEXEC)
    }

    /// Accept a new socket stream ([`AsyncFd`]) setting `flags` on the accepted
    /// socket.
    ///
    /// Also see [`AsyncFd::accept`].
    pub fn accept4<'fd, A>(&'fd self, flags: libc::c_int) -> Accept<'fd, A, D> {
        let address = Box::new((MaybeUninit::uninit(), 0));
        Accept::new(self, address, flags)
    }

    /// Accept multiple socket streams.
    ///
    /// This is not the same as calling [`AsyncFd::accept`] in a loop as this
    /// uses a multishot operation, which means only a single operation is
    /// created kernel side, making this more efficient.
    pub const fn multishot_accept<'fd>(&'fd self) -> MultishotAccept<'fd, D> {
        self.multishot_accept4(libc::SOCK_CLOEXEC)
    }

    /// Accept a new socket stream ([`AsyncFd`]) setting `flags` on the accepted
    /// socket.
    ///
    /// Also see [`AsyncFd::multishot_accept`].
    pub const fn multishot_accept4<'fd>(&'fd self, flags: libc::c_int) -> MultishotAccept<'fd, D> {
        MultishotAccept::new(self, flags)
    }

    /// Get socket option.
    ///
    /// At the time of writing this limited to the `SOL_SOCKET` level.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `T` is the valid type for the option.
    #[doc(alias = "getsockopt")]
    #[allow(clippy::cast_sign_loss)] // No valid negative level or optnames.
    pub fn socket_option<'fd, T>(
        &'fd self,
        level: libc::c_int,
        optname: libc::c_int,
    ) -> SocketOption<'fd, T, D> {
        // TODO: replace with `Box::new_uninit` once `new_uninit` is stable.
        let value = Box::new(MaybeUninit::uninit());
        SocketOption::new(self, value, (level as libc::__u32, optname as libc::__u32))
    }

    /// Set socket option.
    ///
    /// At the time of writing this limited to the `SOL_SOCKET` level.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `T` is the valid type for the option.
    #[doc(alias = "setsockopt")]
    #[allow(clippy::cast_sign_loss)] // No valid negative level or optnames.
    pub fn set_socket_option<'fd, T>(
        &'fd self,
        level: libc::c_int,
        optname: libc::c_int,
        optvalue: T,
    ) -> SetSocketOption<'fd, T, D> {
        let value = Box::new(optvalue);
        SetSocketOption::new(self, value, (level as libc::__u32, optname as libc::__u32))
    }
}

/// [`Future`] to create a new [`socket`] asynchronously.
///
/// If you're looking for a socket type, there is none, see [`AsyncFd`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct Socket<D: Descriptor = File> {
    sq: Option<SubmissionQueue>,
    state: OpState<(libc::c_int, libc::c_int, libc::c_int, libc::c_int)>,
    kind: PhantomData<D>,
}

impl<D: Descriptor + Unpin> Future for Socket<D> {
    type Output = io::Result<AsyncFd<D>>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let op_index = poll_state!(
            Socket,
            self.state,
            // SAFETY: `poll_state!` will panic if `OpState == Done`, if not
            // this unwrap is safe.
            self.sq.as_ref().unwrap(),
            ctx,
            |submission, (domain, r#type, protocol, flags)| unsafe {
                submission.socket(domain, r#type, protocol, flags);
                D::create_flags(submission)
            },
        );

        // SAFETY: this is only `None` if `OpState == Done`, which would mean
        // `poll_state!` above would panic.
        let sq = self.sq.as_ref().unwrap();
        match sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok((_, fd)) => Poll::Ready(Ok(unsafe {
                        // SAFETY: the socket operation ensures that `fd` is valid.
                        // SAFETY: unwrapping `sq` is safe as used it above.
                        AsyncFd::from_raw(fd, self.sq.take().unwrap())
                    })),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

// Connect.
op_future! {
    fn AsyncFd::connect -> (),
    struct Connect<'fd, A: SocketAddress> {
        /// Address needs to stay alive for as long as the kernel is connecting.
        address: Box<A>,
    },
    setup_state: _unused: (),
    setup: |submission, fd, (address,), ()| unsafe {
        let (ptr, len) = SocketAddress::as_ptr(&**address);
        submission.connect(fd.fd(), ptr, len);
    },
    map_result: |result| Ok(debug_assert!(result == 0)),
    extract: |this, (address,), res| -> Box<A> {
        debug_assert!(res == 0);
        Ok(address)
    },
}

// Send.
op_future! {
    fn AsyncFd::send -> usize,
    struct Send<'fd, B: Buf> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
    },
    setup_state: flags: (u8, libc::c_int),
    setup: |submission, fd, (buf,), (op, flags)| unsafe {
        let (ptr, len) = buf.parts();
        submission.send(op, fd.fd(), ptr, len, flags);
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

/// [`Future`] behind [`AsyncFd::send_all`].
#[derive(Debug)]
pub struct SendAll<'fd, B, D: Descriptor = File> {
    send: Extractor<Send<'fd, SkipBuf<B>, D>>,
}

impl<'fd, B: Buf, D: Descriptor> SendAll<'fd, B, D> {
    const fn new(fd: &'fd AsyncFd<D>, buf: B) -> SendAll<'fd, B, D> {
        let buf = SkipBuf { buf, skip: 0 };
        SendAll {
            // TODO: once `Extract` is a constant trait use that.
            send: Extractor {
                fut: fd.send(buf, 0),
            },
        }
    }

    /// Poll implementation used by the [`Future`] implement for the naked type
    /// and the type wrapper in an [`Extractor`].
    fn inner_poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<io::Result<B>> {
        // SAFETY: not moving `Future`.
        let this = unsafe { Pin::into_inner_unchecked(self) };
        let mut send = unsafe { Pin::new_unchecked(&mut this.send) };
        match send.as_mut().poll(ctx) {
            Poll::Ready(Ok((_, 0))) => Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
            Poll::Ready(Ok((mut buf, n))) => {
                buf.skip += n as u32;

                if let (_, 0) = unsafe { buf.parts() } {
                    // Written everything.
                    return Poll::Ready(Ok(buf.buf));
                }

                send.set(send.fut.fd.send(buf, 0).extract());
                unsafe { Pin::new_unchecked(this) }.inner_poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'fd, B, D: Descriptor> Cancel for SendAll<'fd, B, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.send.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
        self.send.cancel()
    }
}

impl<'fd, B: Buf, D: Descriptor> Future for SendAll<'fd, B, D> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.inner_poll(ctx).map_ok(|_| ())
    }
}

impl<'fd, B: Buf, D: Descriptor> Extract for SendAll<'fd, B, D> {}

impl<'fd, B: Buf, D: Descriptor> Future for Extractor<SendAll<'fd, B, D>> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `fut`.
        unsafe { Pin::map_unchecked_mut(self, |s| &mut s.fut) }.inner_poll(ctx)
    }
}

// SendTo.
op_future! {
    fn AsyncFd::sendto -> usize,
    struct SendTo<'fd, B: Buf, A: SocketAddress> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
        /// Address to send to.
        address: A,
    },
    setup_state: flags: (u8, libc::c_int),
    setup: |submission, fd, (buf, address), (op, flags)| unsafe {
        let (buf, buf_len) = buf.parts();
        let (addr, addr_len) = SocketAddress::as_ptr(address);
        submission.sendto(op, fd.fd(), buf, buf_len, addr, addr_len, flags);
    },
    map_result: |n| {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok(n as usize)
    },
    extract: |this, (buf, _), n| -> (B, usize) {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok((buf, n as usize))
    },
}

// SendMsg.
op_future! {
    fn AsyncFd::send_vectored -> usize,
    struct SendMsg<'fd, B: BufSlice<N>, A: SocketAddress; const N: usize> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        bufs: B,
        /// Address to send to.
        address: A,
        /// NOTE: we only need `msg` and `iovec` in the submission, we don't
        /// have to keep around during the operation. Because of this we don't
        /// heap allocate it like we for other operations. This leaves a small
        /// duration between the submission of the entry and the submission
        /// being read by the kernel in which this future could be dropped and
        /// the kernel will read memory we don't own. However because we wake
        /// the kernel after submitting the timeout entry it's not really worth
        /// to heap allocation.
        msg: libc::msghdr,
        iovecs: [libc::iovec; N],
    },
    /// `msg` and `iovecs` can't move until the kernel has read the submission.
    impl !Upin,
    setup_state: flags: (u8, libc::c_int),
    setup: |submission, fd, (_, address, msg, iovecs), (op, flags)| unsafe {
        msg.msg_iov = iovecs.as_mut_ptr();
        msg.msg_iovlen = N;
        let (addr, addr_len) = SocketAddress::as_ptr(address);
        msg.msg_name = addr.cast_mut().cast();
        msg.msg_namelen = addr_len;
        submission.sendmsg(op, fd.fd(), &*msg, flags);
    },
    map_result: |n| {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok(n as usize)
    },
    extract: |this, (buf, _, _, _), n| -> (B, usize) {
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        Ok((buf, n as usize))
    },
}

/// [`Future`] behind [`AsyncFd::send_all_vectored`].
#[derive(Debug)]
pub struct SendAllVectored<'fd, B, const N: usize, D: Descriptor = File> {
    send: Extractor<SendMsg<'fd, B, NoAddress, N, D>>,
    skip: u64,
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> SendAllVectored<'fd, B, N, D> {
    fn new(fd: &'fd AsyncFd<D>, bufs: B) -> SendAllVectored<'fd, B, N, D> {
        SendAllVectored {
            send: fd.send_vectored(bufs, 0).extract(),
            skip: 0,
        }
    }

    /// Poll implementation used by the [`Future`] implement for the naked type
    /// and the type wrapper in an [`Extractor`].
    fn inner_poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<io::Result<B>> {
        // SAFETY: not moving `Future`.
        let this = unsafe { Pin::into_inner_unchecked(self) };
        let mut send = unsafe { Pin::new_unchecked(&mut this.send) };
        match send.as_mut().poll(ctx) {
            Poll::Ready(Ok((_, 0))) => Poll::Ready(Err(io::ErrorKind::WriteZero.into())),
            Poll::Ready(Ok((bufs, n))) => {
                this.skip += n as u64;

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

                // SAFETY: zeroed `msghdr` is valid.
                let msg = unsafe { mem::zeroed() };
                let op = libc::IORING_OP_SENDMSG as u8;
                send.set(
                    SendMsg::new(send.fut.fd, bufs, NoAddress, msg, iovecs, (op, 0)).extract(),
                );
                unsafe { Pin::new_unchecked(this) }.inner_poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<'fd, B, const N: usize, D: Descriptor> Cancel for SendAllVectored<'fd, B, N, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.send.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
        self.send.cancel()
    }
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> Future for SendAllVectored<'fd, B, N, D> {
    type Output = io::Result<()>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.inner_poll(ctx).map_ok(|_| ())
    }
}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> Extract for SendAllVectored<'fd, B, N, D> {}

impl<'fd, B: BufSlice<N>, const N: usize, D: Descriptor> Future
    for Extractor<SendAllVectored<'fd, B, N, D>>
{
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        unsafe { Pin::map_unchecked_mut(self, |s| &mut s.fut) }.inner_poll(ctx)
    }
}

// Recv.
op_future! {
    fn AsyncFd::recv -> B,
    struct Recv<'fd, B: BufMut> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
    },
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (buf,), flags| unsafe {
        let (ptr, len) = buf.parts_mut();
        submission.recv(fd.fd(), ptr, len, flags);
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

// MultishotRecv.
op_async_iter! {
    fn AsyncFd::multishot_recv -> ReadBuf,
    struct MultishotRecv<'fd> {
        /// Buffer pool used in the receive operation.
        buf_pool: ReadBufPool,
    },
    setup_state: flags: libc::c_int,
    setup: |submission, this, flags| unsafe {
        submission.multishot_recv(this.fd.fd(), flags, this.buf_pool.group_id().0);
    },
    map_result: |this, buf_idx, n| {
        if n == 0 {
            // Peer closed it's writing half.
            this.state = crate::op::OpState::Done;
        }
        // SAFETY: the kernel initialised the buffers for us as part of the read
        // call.
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        unsafe { this.buf_pool.new_buffer(BufIdx(buf_idx), n as u32) }
    },
}

/// [`Future`] behind [`AsyncFd::recv_n`].
#[derive(Debug)]
pub struct RecvN<'fd, B, D: Descriptor = File> {
    recv: Recv<'fd, ReadNBuf<B>, D>,
    /// Number of bytes we still need to receive to hit our target `N`.
    left: usize,
}

impl<'fd, B: BufMut, D: Descriptor> RecvN<'fd, B, D> {
    const fn new(fd: &'fd AsyncFd<D>, buf: B, n: usize) -> RecvN<'fd, B, D> {
        let buf = ReadNBuf { buf, last_read: 0 };
        RecvN {
            recv: fd.recv(buf, 0),
            left: n,
        }
    }
}

impl<'fd, B, D: Descriptor> Cancel for RecvN<'fd, B, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.recv.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
        self.recv.cancel()
    }
}

impl<'fd, B: BufMut, D: Descriptor> Future for RecvN<'fd, B, D> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving the `Recv` future.
        let this = unsafe { Pin::into_inner_unchecked(self) };
        let mut recv = unsafe { Pin::new_unchecked(&mut this.recv) };
        match recv.as_mut().poll(ctx) {
            Poll::Ready(Ok(buf)) => {
                if buf.last_read == 0 {
                    return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                }

                if buf.last_read >= this.left {
                    // Received the required amount of bytes.
                    return Poll::Ready(Ok(buf.buf));
                }

                this.left -= buf.last_read;

                recv.set(recv.fd.recv(buf, 0));
                unsafe { Pin::new_unchecked(this) }.poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

// RecvVectored.
op_future! {
    fn AsyncFd::recv_vectored -> (B, libc::c_int),
    struct RecvVectored<'fd, B: BufMutSlice<N>; const N: usize> {
        /// Buffers to read from, needs to stay in memory so the kernel can
        /// access it safely.
        bufs: B,
        /// The kernel will write to `msghdr`, so it needs to stay in memory so
        /// the kernel can access it safely.
        msg: Box<libc::msghdr>,
        /// NOTE: we only need `iovec` in the submission, we don't have to keep
        /// around during the operation. Because of this we don't heap allocate
        /// it like we for other operations. This leaves a small duration
        /// between the submission of the entry and the submission being read by
        /// the kernel in which this future could be dropped and the kernel will
        /// read memory we don't own. However because we wake the kernel after
        /// submitting the timeout entry it's not really worth to heap
        /// allocation.
        iovecs: [libc::iovec; N],
    },
    /// `iovecs` can't move until the kernel has read the submission.
    impl !Upin,
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (_, msg, iovecs), flags| unsafe {
        msg.msg_iov = iovecs.as_mut_ptr();
        msg.msg_iovlen = N;
        submission.recvmsg(fd.fd(), &**msg, flags);
    },
    map_result: |this, (mut bufs, msg, _), n| {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        unsafe { bufs.set_init(n as usize) };
        Ok((bufs, msg.msg_flags))
    },
}

/// [`Future`] behind [`AsyncFd::recv_n_vectored`].
#[derive(Debug)]
pub struct RecvNVectored<'fd, B, const N: usize, D: Descriptor = File> {
    recv: RecvVectored<'fd, ReadNBuf<B>, N, D>,
    /// Number of bytes we still need to receive to hit our target `N`.
    left: usize,
}

impl<'fd, B: BufMutSlice<N>, const N: usize, D: Descriptor> RecvNVectored<'fd, B, N, D> {
    fn new(fd: &'fd AsyncFd<D>, buf: B, n: usize) -> RecvNVectored<'fd, B, N, D> {
        let bufs = ReadNBuf { buf, last_read: 0 };
        RecvNVectored {
            recv: fd.recv_vectored(bufs, 0),
            left: n,
        }
    }
}

impl<'fd, B, const N: usize, D: Descriptor> Cancel for RecvNVectored<'fd, B, N, D> {
    fn try_cancel(&mut self) -> CancelResult {
        self.recv.try_cancel()
    }

    fn cancel(&mut self) -> CancelOp {
        self.recv.cancel()
    }
}

impl<'fd, B: BufMutSlice<N>, const N: usize, D: Descriptor> Future for RecvNVectored<'fd, B, N, D> {
    type Output = io::Result<B>;

    fn poll(self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        // SAFETY: not moving `Future`.
        let this = unsafe { Pin::into_inner_unchecked(self) };
        let mut recv = unsafe { Pin::new_unchecked(&mut this.recv) };
        match recv.as_mut().poll(ctx) {
            Poll::Ready(Ok((bufs, _))) => {
                if bufs.last_read == 0 {
                    return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                }

                if bufs.last_read >= this.left {
                    // Read the required amount of bytes.
                    return Poll::Ready(Ok(bufs.buf));
                }

                this.left -= bufs.last_read;

                recv.set(recv.fd.recv_vectored(bufs, 0));
                unsafe { Pin::new_unchecked(this) }.poll(ctx)
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

// RecvFrom.
op_future! {
    fn AsyncFd::recvfrom -> (B, A, libc::c_int),
    struct RecvFrom<'fd, B: BufMut, A: SocketAddress> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
        /// The kernel will write to `msghdr` and the address, so both need to
        /// stay in memory so the kernel can access it safely.
        msg: Box<(libc::msghdr, MaybeUninit<A>)>,
        /// NOTE: we only need `iovec` in the submission, we don't have to keep
        /// around during the operation. Because of this we don't heap allocate
        /// it like we for other operations. This leaves a small duration
        /// between the submission of the entry and the submission being read by
        /// the kernel in which this future could be dropped and the kernel will
        /// read memory we don't own. However because we wake the kernel after
        /// submitting the timeout entry it's not really worth to heap
        /// allocation.
        iovec: libc::iovec,
    },
    /// `iovec` can't move until the kernel has read the submission.
    impl !Upin,
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (buf, msg, iovec), flags| unsafe {
        let address = &mut msg.1;
        let msg = &mut msg.0;
        msg.msg_iov = &mut *iovec;
        msg.msg_iovlen = 1;
        let (addr, addr_len) = SocketAddress::as_mut_ptr(address);
        msg.msg_name = addr.cast();
        msg.msg_namelen = addr_len;
        submission.recvmsg(fd.fd(), &*msg, flags);
        if let Some(buf_group) = buf.buffer_group() {
            submission.set_buffer_select(buf_group.0);
        }
    },
    map_result: |this, (mut buf, msg, _), buf_idx, n| {
        // SAFETY: the kernel initialised the bytes for us as part of the
        // recvmsg call.
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        unsafe { buf.buffer_init(BufIdx(buf_idx), n as u32) };
        // SAFETY: kernel initialised the address for us.
        let address = unsafe { SocketAddress::init(msg.1, msg.0.msg_namelen) };
        Ok((buf, address, msg.0.msg_flags))
    },
}

// RecvFromVectored.
op_future! {
    fn AsyncFd::recvfrom_vectored -> (B, A, libc::c_int),
    struct RecvFromVectored<'fd, B: BufMutSlice<N>, A: SocketAddress; const N: usize> {
        /// Buffers to read from, needs to stay in memory so the kernel can
        /// access it safely.
        bufs: B,
        /// The kernel will write to `msghdr` and the address, so both need to
        /// stay in memory so the kernel can access it safely.
        msg: Box<(libc::msghdr, MaybeUninit<A>)>,
        /// NOTE: we only need `iovec` in the submission, we don't have to keep
        /// around during the operation. Because of this we don't heap allocate
        /// it like we for other operations. This leaves a small duration
        /// between the submission of the entry and the submission being read by
        /// the kernel in which this future could be dropped and the kernel will
        /// read memory we don't own. However because we wake the kernel after
        /// submitting the timeout entry it's not really worth to heap
        /// allocation.
        iovecs: [libc::iovec; N],
    },
    /// `iovecs` can't move until the kernel has read the submission.
    impl !Upin,
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (_, msg, iovecs), flags| unsafe {
        let address = &mut msg.1;
        let msg = &mut msg.0;
        msg.msg_iov = iovecs.as_mut_ptr();
        msg.msg_iovlen = N;
        let (addr, addr_len) = SocketAddress::as_mut_ptr(address);
        msg.msg_name = addr.cast();
        msg.msg_namelen = addr_len;
        submission.recvmsg(fd.fd(), &*msg, flags);
    },
    map_result: |this, (mut bufs, msg, _), n| {
        // SAFETY: the kernel initialised the buffers for us as part of the
        // recvmsg call.
        #[allow(clippy::cast_sign_loss)] // Negative values are mapped to errors.
        unsafe { bufs.set_init(n as usize) };
        // SAFETY: kernel initialised the address for us.
        let address = unsafe { SocketAddress::init(msg.1, msg.0.msg_namelen) };
        Ok((bufs, address, msg.0.msg_flags))
    },
}

// Shutdown.
op_future! {
    fn AsyncFd::shutdown -> (),
    struct Shutdown<'fd> {
        // Doesn't need any fields.
    },
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (), how| unsafe {
        submission.shutdown(fd.fd(), how);
    },
    map_result: |n| Ok(debug_assert!(n == 0)),
}

// Accept.
op_future! {
    fn AsyncFd::accept -> (AsyncFd<D>, A),
    struct Accept<'fd, A: SocketAddress> {
        /// Address for the accepted connection, needs to stay in memory so the
        /// kernel can access it safely.
        address: Box<(MaybeUninit<A>, libc::socklen_t)>,
    },
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (address,), flags| unsafe {
        let (ptr, len) = SocketAddress::as_mut_ptr(&mut address.0);
        address.1 = len;
        submission.accept(fd.fd(), ptr, &mut address.1, flags);
        D::create_flags(submission);
    },
    map_result: |this, (address,), fd| {
        let sq = this.fd.sq.clone();
        // SAFETY: the accept operation ensures that `fd` is valid.
        let stream = unsafe { AsyncFd::from_raw(fd, sq) };
        let len = address.1;
        // SAFETY: kernel initialised the memory for us.
        let address = unsafe { SocketAddress::init(address.0, len) };
        Ok((stream, address))
    },
}

// MultishotAccept.
op_async_iter! {
    fn AsyncFd::multishot_accept -> AsyncFd<D>,
    struct MultishotAccept<'fd> {
        // No additional state.
    },
    setup_state: flags: libc::c_int,
    setup: |submission, this, flags| unsafe {
        submission.multishot_accept(this.fd.fd(), flags);
        D::create_flags(submission);
    },
    map_result: |this, _flags, fd| {
        let sq = this.fd.sq.clone();
        // SAFETY: the accept operation ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd, sq) }
    },
}

// SocketOption.
op_future! {
    fn AsyncFd::socket_option -> T,
    struct SocketOption<'fd, T> {
        /// Value for the socket option, needs to stay in memory so the kernel
        /// can access it safely.
        value: Box<MaybeUninit<T>>,
    },
    setup_state: flags: (libc::__u32, libc::__u32),
    setup: |submission, fd, (value,), (level, optname)| unsafe {
        let optvalue = ptr::addr_of_mut!(**value).cast();
        let optlen = size_of::<T>() as u32;
        submission.uring_command(libc::SOCKET_URING_OP_GETSOCKOPT, fd.fd(), level, optname, optvalue, optlen);
    },
    map_result: |this, (value,), optlen| {
        debug_assert!(optlen == (size_of::<T>() as i32));
        // SAFETY: the kernel initialised the value for us as part of the
        // getsockopt call.
        Ok(unsafe { MaybeUninit::assume_init(*value) })
    },
}

// SetSocketOption.
op_future! {
    fn AsyncFd::set_socket_option -> (),
    struct SetSocketOption<'fd, T> {
        /// Value for the socket option, needs to stay in memory so the kernel
        /// can access it safely.
        value: Box<T>,
    },
    setup_state: flags: (libc::__u32, libc::__u32),
    setup: |submission, fd, (value,), (level, optname)| unsafe {
        let optvalue = ptr::addr_of_mut!(**value).cast();
        let optlen = size_of::<T>() as u32;
        submission.uring_command(libc::SOCKET_URING_OP_SETSOCKOPT, fd.fd(), level, optname, optvalue, optlen);
    },
    map_result: |result| Ok(debug_assert!(result == 0)),
    extract: |this, (value,), res| -> Box<T> {
        debug_assert!(res == 0);
        Ok(value)
    },
}

/// Trait that defines the behaviour of socket addresses.
///
/// Linux (Unix) uses different address types for different sockets, to support
/// all of them A10 uses a trait to define the behaviour.
///
/// Current implementations include
///  * IPv4 addresses: [`libc::sockaddr_in`],
///  * IPv6 addresses: [`libc::sockaddr_in6`],
///  * Unix addresses: [`libc::sockaddr_un`],
///  * Storage of any address [`libc::sockaddr_storage`] kind.
///
/// For the last two types we need to keep track of the length of the address,
/// for which it uses a tuple `(addr, `[`libc::socklen_t`]`)`.
pub trait SocketAddress: Sized {
    /// Returns itself as raw pointer and length.
    ///
    /// # Safety
    ///
    /// The pointer must be valid to read up to length bytes from.
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointing to memory owned by the address. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the address.
    ///
    /// Note that the above requirements are only required for implementations
    /// outside of A10. **This trait is unfit for external use!**
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t);

    /// Returns `address` as an adress storage and it's length.
    ///
    /// # Safety
    ///
    /// Only initialised bytes may be written to the pointer returned.
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointing to memory owned by the address. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the address.
    ///
    /// Note that the above requirements are only required for implementations
    /// outside of A10. **This trait is unfit for external use!**
    unsafe fn as_mut_ptr(address: &mut MaybeUninit<Self>)
        -> (*mut libc::sockaddr, libc::socklen_t);

    /// Initialise `address` to which at least `length` bytes have been written
    /// (by the kernel).
    ///
    /// # Safety
    ///
    /// Caller must ensure that at least `length` bytes have been written to
    /// `address`.
    unsafe fn init(address: MaybeUninit<Self>, length: libc::socklen_t) -> Self;
}

/// Socket address.
impl SocketAddress for (libc::sockaddr, libc::socklen_t) {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (ptr::addr_of!(self.0).cast(), self.1)
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            ptr::addr_of_mut!((*this.as_mut_ptr()).0).cast(),
            size_of::<libc::sockaddr>() as _,
        )
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length >= size_of::<libc::sa_family_t>() as _);
        // SAFETY: caller must initialise the address.
        let mut this = this.assume_init();
        this.1 = length;
        this
    }
}

/// Any kind of address.
impl SocketAddress for (libc::sockaddr_storage, libc::socklen_t) {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (ptr::addr_of!(self.0).cast(), self.1)
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            ptr::addr_of_mut!((*this.as_mut_ptr()).0).cast(),
            size_of::<libc::sockaddr_storage>() as _,
        )
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length >= size_of::<libc::sa_family_t>() as _);
        // SAFETY: caller must initialise the address.
        let mut this = this.assume_init();
        this.1 = length;
        this
    }
}

/// IPv4 address.
impl SocketAddress for libc::sockaddr_in {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (
            (self as *const libc::sockaddr_in).cast(),
            size_of::<Self>() as _,
        )
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (this.as_mut_ptr().cast(), size_of::<Self>() as _)
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self>() as _);
        // SAFETY: caller must initialise the address.
        this.assume_init()
    }
}

/// IPv6 address.
impl SocketAddress for libc::sockaddr_in6 {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (
            (self as *const libc::sockaddr_in6).cast(),
            size_of::<Self>() as _,
        )
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (this.as_mut_ptr().cast(), size_of::<Self>() as _)
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self>() as _);
        // SAFETY: caller must initialise the address.
        this.assume_init()
    }
}

/// Unix address.
impl SocketAddress for (libc::sockaddr_un, libc::socklen_t) {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        (ptr::addr_of!(self.0).cast(), self.1)
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            ptr::addr_of_mut!((*this.as_mut_ptr()).0).cast(),
            size_of::<libc::sockaddr_un>() as _,
        )
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length >= size_of::<libc::sa_family_t>() as _);
        // SAFETY: caller must initialise the address.
        let mut this = this.assume_init();
        this.1 = length;
        this
    }
}

/// When [`accept`]ing connections we're not interested in the address.
///
/// This is not acceptable in calls to [`connect`].
///
/// [`accept`]: AsyncFd::accept
/// [`connect`]: AsyncFd::connect
#[derive(Debug)]
pub struct NoAddress;

impl SocketAddress for NoAddress {
    unsafe fn as_ptr(&self) -> (*const libc::sockaddr, libc::socklen_t) {
        // NOTE: this goes against the requirements of `cast_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn as_mut_ptr(this: &mut MaybeUninit<Self>) -> (*mut libc::sockaddr, libc::socklen_t) {
        _ = this;
        // NOTE: this goes against the requirements of `as_mut_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn init(this: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length == 0);
        this.assume_init()
    }
}
