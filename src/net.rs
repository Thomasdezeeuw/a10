//! Asynchronous networking.

use std::cell::UnsafeCell;
use std::future::Future;
use std::io;
use std::mem::{size_of, MaybeUninit};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::pin::Pin;
use std::task::{self, Poll};

use crate::io::{ReadBuf, WriteBuf};
use crate::op::op_future;
use crate::{libc, AsyncFd, OpIndex, QueueFull, SubmissionQueue};

/// Creates a new socket.
pub fn socket(
    sq: SubmissionQueue,
    domain: libc::c_int,
    r#type: libc::c_int,
    protocol: libc::c_int,
    flags: libc::c_int,
) -> Result<Socket, QueueFull> {
    let op_index = sq.add(|submission| unsafe {
        submission.socket(domain, r#type, protocol, flags);
    })?;

    Ok(Socket {
        sq: Some(sq),
        op_index,
    })
}

/// Socket related system calls.
impl AsyncFd {
    /// Initiate a connection on this socket to the specified address.
    pub fn connect<'fd, A>(
        &'fd self,
        address: A,
        address_length: libc::socklen_t,
    ) -> Result<Connect<'fd>, QueueFull>
    where
        A: Into<Box<libc::sockaddr_storage>>,
    {
        let mut address = address.into();
        let op_index = self.sq.add(|submission| unsafe {
            submission.connect(self.fd, &mut address, address_length);
        })?;

        Ok(Connect {
            // Needs to stay alive as long as the kernel is accessing it.
            address: Some(UnsafeCell::new(address)),
            fd: self,
            op_index,
        })
    }

    /// Sends data on the socket to a connected peer.
    pub fn send<'fd, B>(&'fd self, buf: B) -> Result<Send<'fd, B>, QueueFull>
    where
        B: WriteBuf,
    {
        self.send_with_flags(buf, 0)
    }

    /// Identical to [`AsyncFd::send`] but allows for specification of arbitrary
    /// flags to the underlying `send` call.
    pub fn send_with_flags<'fd, B>(
        &'fd self,
        buf: B,
        flags: libc::c_int,
    ) -> Result<Send<'fd, B>, QueueFull>
    where
        B: WriteBuf,
    {
        let op_index = self.sq.add(|submission| unsafe {
            let (ptr, len) = buf.parts();
            submission.send(self.fd, ptr, len, flags);
        })?;

        Ok(Send {
            // Needs to stay alive as long as the kernel is accessing it.
            buf: Some(UnsafeCell::new(buf)),
            fd: self,
            op_index,
        })
    }

    /// Same as [`AsyncFd::send_with_flags`], but tries to avoid making
    /// intermediate copies of `buf`.
    ///
    /// # Notes
    ///
    /// Zerocopy execution is not guaranteed and may fall back to copying. The
    /// request may also fail with `EOPNOTSUPP`, when a protocol doesn't support
    /// zerocopy, in which case users are recommended to use
    /// [`AsyncFd::send_with_flags`] instead.
    ///
    /// The `Future` only returns once it safe for the buffer to be used again,
    /// for TCP for example this means until the data is ACKed by the peer.
    pub fn send_zc<'fd, B>(&'fd self, buf: B, flags: libc::c_int) -> Result<Send<'fd, B>, QueueFull>
    where
        B: WriteBuf,
    {
        let op_index = self.sq.add(|submission| unsafe {
            let (ptr, len) = buf.parts();
            submission.send_zc(self.fd, ptr, len, flags);
        })?;

        Ok(Send {
            // Needs to stay alive as long as the kernel is accessing it.
            buf: Some(UnsafeCell::new(buf)),
            fd: self,
            op_index,
        })
    }

    /// Receives data on the socket from the remote address to which it is
    /// connected.
    ///
    /// # Notes
    ///
    /// This leave the current contents of `buf` untouched and only uses the
    /// spare capacity.
    pub fn recv<'fd, B>(&'fd self, buf: B) -> Result<Recv<'fd, B>, QueueFull>
    where
        B: ReadBuf,
    {
        self.recv_with_flags(buf, 0)
    }

    /// Identical to [`AsyncFd::recv`] but allows for specification of arbitrary
    /// flags to the underlying `recv` call.
    pub fn recv_with_flags<'fd, B>(
        &'fd self,
        mut buf: B,
        flags: libc::c_int,
    ) -> Result<Recv<'fd, B>, QueueFull>
    where
        B: ReadBuf,
    {
        let op_index = self.sq.add(|submission| unsafe {
            let (ptr, len) = buf.parts();
            submission.recv(self.fd, ptr, len, flags);
        })?;

        Ok(Recv {
            // Needs to stay alive as long as the kernel is accessing it.
            buf: Some(UnsafeCell::new(buf)),
            fd: self,
            op_index,
        })
    }

    /// Accept a new socket stream ([`AsyncFd`]).
    ///
    /// If an accepted stream is returned, the remote address of the peer is
    /// returned along with it.
    pub fn accept<'fd>(&'fd self) -> Result<Accept<'fd>, QueueFull> {
        self.accept4(libc::SOCK_CLOEXEC)
    }

    /// Accept a new socket stream ([`AsyncFd`]) setting `flags` on the accepted
    /// socket.
    ///
    /// Also see [`AsyncFd::accept`].
    pub fn accept4<'fd>(&'fd self, flags: libc::c_int) -> Result<Accept<'fd>, QueueFull> {
        let address: MaybeUninit<libc::sockaddr_storage> = MaybeUninit::uninit();
        let length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let mut address = Box::new((address, length));

        let op_index = self.sq.add(|submission| unsafe {
            submission.accept(self.fd, &mut address.0, &mut address.1, flags);
        })?;

        Ok(Accept {
            address: Some(UnsafeCell::new(address)),
            fd: self,
            op_index,
        })
    }
}

/// [`Future`] to create a new [`socket`] asynchronously.
///
/// If you're looking for a socket type, there is none, see [`AsyncFd`].
#[derive(Debug)]
pub struct Socket {
    sq: Option<SubmissionQueue>,
    op_index: OpIndex,
}

impl Future for Socket {
    type Output = io::Result<AsyncFd>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.sq
            .as_ref()
            .unwrap()
            .poll_op(ctx, self.op_index)
            .map_ok(|fd| AsyncFd {
                fd,
                sq: self.sq.take().unwrap(),
            })
    }
}

// Connect.
op_future! {
    fn AsyncFd::connect -> (),
    struct Connect<'fd> {
        /// Address needs to stay alive for as long as the kernel is connecting.
        address: Box<libc::sockaddr_storage>, "dropped `a10::net::Connect` before completion, leaking address buffer",
    },
    |this, res| {
        drop(this.address.take());
        Ok(debug_assert!(res == 0))
    },
    extract: |this, res| -> Box<libc::sockaddr_storage> {
        debug_assert!(res == 0);
        Ok(this.address.take().unwrap().into_inner())
    },
}

// Send.
op_future! {
    fn AsyncFd::send -> usize,
    struct Send<'fd, B: WriteBuf> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B, "dropped `a10::net::Send` before completion, leaking buffer",
    },
    |this, n| {
        drop(this.buf.take());
        Ok(n as usize)
    },
    extract: |this, n| -> (B, usize) {
        let buf = this.buf.take().unwrap().into_inner();
        Ok((buf, n as usize))
    },
}

// Recv.
op_future! {
    fn AsyncFd::recv -> B,
    struct Recv<'fd, B: ReadBuf> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: B, "dropped `a10::net::Recv` before completion, leaking buffer",
    },
    |this, n| {
        let mut buf = this.buf.take().unwrap().into_inner();
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
        address: Box<(MaybeUninit<libc::sockaddr_storage>, libc::socklen_t)>, "dropped `a10::net::Accept` before completion, leaking address buffer",
    },
    |this, fd| {
        let sq = this.fd.sq.clone();
        let stream = AsyncFd { fd, sq };

        let address = this.address.take().unwrap().into_inner();
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
