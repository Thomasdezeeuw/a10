//! Asynchronous networking.

use std::future::Future;
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::pin::Pin;
use std::task::{self, Poll};

use crate::io::{ReadBuf, WriteBuf};
use crate::op::{op_future, poll_state, OpState};
use crate::{libc, AsyncFd, SubmissionQueue};

/// Creates a new socket.
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
    pub fn connect<'fd, A>(&'fd self, address: A, address_length: libc::socklen_t) -> Connect<'fd>
    where
        A: Into<Box<libc::sockaddr_storage>>,
    {
        let address = address.into();
        Connect::new(self, address, address_length)
    }

    /// Sends data on the socket to a connected peer.
    pub const fn send<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Send<'fd, B>
    where
        B: WriteBuf,
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
    pub fn send_zc<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Send<'fd, B>
    where
        B: WriteBuf,
    {
        Send::new(self, buf, (libc::IORING_OP_SEND_ZC as u8, flags))
    }

    /// Receives data on the socket from the remote address to which it is
    /// connected.
    pub const fn recv<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Recv<'fd, B>
    where
        B: ReadBuf,
    {
        Recv::new(self, buf, flags)
    }

    /// Accept a new socket stream ([`AsyncFd`]).
    ///
    /// If an accepted stream is returned, the remote address of the peer is
    /// returned along with it.
    pub fn accept<'fd>(&'fd self) -> Accept<'fd> {
        self.accept4(libc::SOCK_CLOEXEC)
    }

    /// Accept a new socket stream ([`AsyncFd`]) setting `flags` on the accepted
    /// socket.
    ///
    /// Also see [`AsyncFd::accept`].
    pub fn accept4<'fd>(&'fd self, flags: libc::c_int) -> Accept<'fd> {
        let address: MaybeUninit<libc::sockaddr_storage> = MaybeUninit::uninit();
        let length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let address = Box::new((address, length));
        Accept::new(self, address, flags)
    }
}

/// [`Future`] to create a new [`socket`] asynchronously.
///
/// If you're looking for a socket type, there is none, see [`AsyncFd`].
#[derive(Debug)]
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
                    Ok(fd) => Poll::Ready(Ok(AsyncFd {
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
    struct Connect<'fd> {
        /// Address needs to stay alive for as long as the kernel is connecting.
        address: Box<libc::sockaddr_storage>,
    },
    setup_state: address_length: libc::socklen_t,
    setup: |submission, fd, (address,), address_length| unsafe {
        submission.connect(fd.fd, &mut *address, address_length);
    },
    map_result: |result| Ok(debug_assert!(result == 0)),
    extract: |this, (address,), res| -> Box<libc::sockaddr_storage> {
        debug_assert!(res == 0);
        Ok(address)
    },
}

// Send.
op_future! {
    fn AsyncFd::send -> usize,
    struct Send<'fd, B: WriteBuf> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
    },
    setup_state: flags: (u8, libc::c_int),
    setup: |submission, fd, (buf,), (op, flags)| unsafe {
        let (ptr, len) = buf.parts();
        submission.send(op, fd.fd, ptr, len, flags);
    },
    map_result: |n| Ok(n as usize),
    extract: |this, (buf,), n| -> (B, usize) {
        Ok((buf, n as usize))
    },
}

// Recv.
op_future! {
    fn AsyncFd::recv -> B,
    struct Recv<'fd, B: ReadBuf> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B,
    },
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (buf,), flags| unsafe {
        let (ptr, len) = buf.parts();
        submission.recv(fd.fd, ptr, len, flags);
    },
    map_result: |this, (mut buf,), n| {
        unsafe { buf.set_init(n as usize) };
        Ok(buf)
    },
}

// Accept.
op_future! {
    fn AsyncFd::accept -> (AsyncFd, SocketAddr),
    struct Accept<'fd> {
        /// Address for the accepted connection, needs to stay in memory so the
        /// kernel can access it safely.
        address: Box<(MaybeUninit<libc::sockaddr_storage>, libc::socklen_t)>,
    },
    setup_state: flags: libc::c_int,
    setup: |submission, fd, (address,), flags| unsafe {
        submission.accept(fd.fd, &mut address.0, &mut address.1, flags);
    },
    map_result: |this, (address,), fd| {
        let sq = this.fd.sq.clone();
        let stream = AsyncFd { fd, sq };

        let storage = unsafe { address.0.assume_init_ref() };
        let address_length = address.1 as usize;

        let address = match storage.ss_family as libc::c_int {
            libc::AF_INET => {
                // SAFETY: if the `ss_family` field is `AF_INET` then storage
                // must be a `sockaddr_in`.
                debug_assert!(address_length == size_of::<libc::sockaddr_in>());
                let addr: &libc::sockaddr_in = unsafe { &*(storage as *const _ as *const libc::sockaddr_in) };
                let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
                let port = u16::from_be(addr.sin_port);
                Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
            }
            libc::AF_INET6 => {
                // SAFETY: if the `ss_family` field is `AF_INET6` then storage
                // must be a `sockaddr_in6`.
                debug_assert!(address_length == size_of::<libc::sockaddr_in6>());
                let addr: &libc::sockaddr_in6 = unsafe { &*(storage as *const _ as *const libc::sockaddr_in6) };
                let ip = Ipv6Addr::from(addr.sin6_addr.s6_addr);
                let port = u16::from_be(addr.sin6_port);
                Ok(SocketAddr::V6(SocketAddrV6::new(
                    ip,
                    port,
                    addr.sin6_flowinfo,
                    addr.sin6_scope_id,
                )))
            }
            _ => Err(io::ErrorKind::InvalidInput.into()),
        };

        address.map(|address| (stream, address))
    },
}
