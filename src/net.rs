//! Asynchronous networking.
//!
//! To create a new socket ([`AsyncFd`]) use the [`socket`] function, which
//! issues a non-blocking `socket(2)` call.

use std::future::Future;
use std::mem::{size_of, MaybeUninit};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{io, ptr};

use crate::io::{Buf, BufIdx, BufMut, ReadBuf, ReadBufPool};
use crate::op::{op_async_iter, op_future, poll_state, OpState};
use crate::{libc, AsyncFd, SubmissionQueue};

/// Creates a new socket.
///
/// See the `socket(2)` manual for more information.
pub const fn socket(
    sq: SubmissionQueue,
    domain: libc::c_int,
    r#type: libc::c_int,
    protocol: libc::c_int,
    flags: libc::c_int,
) -> Socket {
    Socket {
        sq: Some(sq),
        state: OpState::NotStarted((domain, r#type, protocol, flags)),
    }
}

/// Socket related system calls.
impl AsyncFd {
    /// Initiate a connection on this socket to the specified address.
    pub fn connect<'fd, A>(&'fd self, address: impl Into<Box<A>>) -> Connect<'fd, A>
    where
        A: SocketAddress,
    {
        let address = address.into();
        Connect::new(self, address, ())
    }

    /// Sends data on the socket to a connected peer.
    pub const fn send<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Send<'fd, B>
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
    pub const fn send_zc<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Send<'fd, B>
    where
        B: Buf,
    {
        Send::new(self, buf, (libc::IORING_OP_SEND_ZC as u8, flags))
    }

    /// Receives data on the socket from the remote address to which it is
    /// connected.
    pub const fn recv<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Recv<'fd, B>
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
    /// Be careful when using this as a peer writing a lot data might take up
    /// all your buffers from your pool!
    pub const fn multishot_recv<'fd>(
        &'fd self,
        pool: ReadBufPool,
        flags: libc::c_int,
    ) -> MultishotRecv<'fd> {
        MultishotRecv::new(self, pool, flags)
    }

    /// Shuts down the read, write, or both halves of this connection.
    pub const fn shutdown<'fd>(&'fd self, how: std::net::Shutdown) -> Shutdown<'fd> {
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
    pub fn accept<'fd, A>(&'fd self) -> Accept<'fd, A> {
        self.accept4(libc::SOCK_CLOEXEC)
    }

    /// Accept a new socket stream ([`AsyncFd`]) setting `flags` on the accepted
    /// socket.
    ///
    /// Also see [`AsyncFd::accept`].
    pub fn accept4<'fd, A>(&'fd self, flags: libc::c_int) -> Accept<'fd, A> {
        let address = Box::new((MaybeUninit::uninit(), 0));
        Accept::new(self, address, flags)
    }

    /// Accept multiple socket streams.
    ///
    /// This is not the same as calling [`AsyncFd::accept`] in a loop as this
    /// uses a multishot operation, which means only a single operation is
    /// created kernel side, making this more efficient.
    pub const fn multishot_accept<'fd>(&'fd self) -> MultishotAccept<'fd> {
        self.multishot_accept4(libc::SOCK_CLOEXEC)
    }

    /// Accept a new socket stream ([`AsyncFd`]) setting `flags` on the accepted
    /// socket.
    ///
    /// Also see [`AsyncFd::multishot_accept`].
    pub const fn multishot_accept4<'fd>(&'fd self, flags: libc::c_int) -> MultishotAccept<'fd> {
        MultishotAccept::new(self, flags)
    }
}

/// [`Future`] to create a new [`socket`] asynchronously.
///
/// If you're looking for a socket type, there is none, see [`AsyncFd`].
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct Socket {
    sq: Option<SubmissionQueue>,
    state: OpState<(libc::c_int, libc::c_int, libc::c_int, libc::c_int)>,
}

impl Future for Socket {
    type Output = io::Result<AsyncFd>;

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
            },
        );

        // SAFETY: this is only `None` if `OpState == Done`, which would mean
        // `poll_state!` above would panic.
        let sq = self.sq.as_ref().unwrap();
        match sq.poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok((_, fd)) => Poll::Ready(Ok(AsyncFd {
                        fd,
                        // SAFETY: used it above.
                        sq: self.sq.take().unwrap(),
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
        let (ptr, len) = A::cast_ptr(&mut **address);
        submission.connect(fd.fd, ptr, len);
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
        submission.send(op, fd.fd, ptr, len, flags);
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
        submission.recv(fd.fd, ptr, len, flags);
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
        submission.multishot_recv(this.fd.fd, flags, this.buf_pool.group_id().0);
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

// Shutdown.
op_future! {
    fn AsyncFd::shutdown -> (),
    struct Shutdown<'fd> {
        // Doesn't need any fields.
    },
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (), how| unsafe {
        submission.shutdown(fd.fd, how);
    },
    map_result: |n| Ok(debug_assert!(n == 0)),
}

// Accept.
op_future! {
    fn AsyncFd::accept -> (AsyncFd, A, libc::socklen_t),
    struct Accept<'fd, A: SocketAddress> {
        /// Address for the accepted connection, needs to stay in memory so the
        /// kernel can access it safely.
        address: Box<(MaybeUninit<A>, libc::socklen_t)>,
    },
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (address,), flags| unsafe {
        let (ptr, len) = A::cast_ptr(ptr::addr_of_mut!(address.0).cast());
        let len_ptr = ptr::addr_of_mut!(address.1);
        len_ptr.write(len);
        submission.accept(fd.fd, ptr, len_ptr, flags);
    },
    map_result: |this, (address,), fd| {
        let sq = this.fd.sq.clone();
        let stream = AsyncFd { fd, sq };
        let len = address.1;
        // SAFETY: kernel initialised the memory for us.
        let address = unsafe { address.0.assume_init() };
        Ok((stream, address, len))
    },
}

// MultishotAccept.
op_async_iter! {
    fn AsyncFd::multishot_accept -> AsyncFd,
    struct MultishotAccept<'fd> {
        // No additional state.
    },
    setup_state: flags: libc::c_int,
    setup: |submission, this, flags| unsafe {
        submission.multishot_accept(this.fd.fd, flags);
    },
    map_result: |this, _flags, fd| {
        let sq = this.fd.sq.clone();
        AsyncFd { fd, sq }
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
pub trait SocketAddress {
    // TODO: once we can cast integers during const eval make the size a
    // constant.

    /// Cast the pointer `ptr` to self to an adress storage and it's length.
    ///
    /// # Safety
    ///
    /// Only initialised bytes may be written to the pointer returned. The
    /// pointer *may* point to uninitialised bytes, so reading from the pointer
    /// is UB.
    ///
    /// The implementation must ensure that the pointer is valid, i.e. not null
    /// and pointing to memory owned by the address. Furthermore it must ensure
    /// that the returned length is, in combination with the pointer, valid. In
    /// other words the memory the pointer and length are pointing to must be a
    /// valid memory address and owned by the address.
    ///
    /// Note that the above requirements are only required for implementations
    /// outside of A10. **This trait is unfit for external use!**
    unsafe fn cast_ptr(ptr: *mut Self) -> (*mut libc::sockaddr, libc::socklen_t);
}

/// Socket address.
impl SocketAddress for libc::sockaddr {
    unsafe fn cast_ptr(ptr: *mut Self) -> (*mut libc::sockaddr, libc::socklen_t) {
        (ptr, size_of::<Self>() as _)
    }
}

/// Any kind of address.
impl SocketAddress for libc::sockaddr_storage {
    unsafe fn cast_ptr(ptr: *mut Self) -> (*mut libc::sockaddr, libc::socklen_t) {
        (ptr.cast(), size_of::<Self>() as _)
    }
}

/// IPv4 address.
impl SocketAddress for libc::sockaddr_in {
    unsafe fn cast_ptr(ptr: *mut Self) -> (*mut libc::sockaddr, libc::socklen_t) {
        (ptr.cast(), size_of::<Self>() as _)
    }
}

/// IPv6 address.
impl SocketAddress for libc::sockaddr_in6 {
    unsafe fn cast_ptr(ptr: *mut Self) -> (*mut libc::sockaddr, libc::socklen_t) {
        (ptr.cast(), size_of::<Self>() as _)
    }
}

/// Unix address.
impl SocketAddress for libc::sockaddr_un {
    unsafe fn cast_ptr(ptr: *mut Self) -> (*mut libc::sockaddr, libc::socklen_t) {
        (ptr.cast(), size_of::<Self>() as _)
    }
}
