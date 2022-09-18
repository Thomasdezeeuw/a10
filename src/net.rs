//! Networking primitives.

use std::future::Future;
use std::io;
use std::mem::{forget as leak, size_of, take, MaybeUninit};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::unix::io::RawFd;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::op::SharedOperationState;
use crate::{libc, QueueFull};

/// A TCP socket server, listening for connections.
#[derive(Debug)]
pub struct TcpListener {
    fd: RawFd,
    state: SharedOperationState,
}

impl TcpListener {
    /// Bind a new TCP listener to the specified `address` to receive new
    /// connections.
    pub fn bind(address: SocketAddr) -> io::Result<TcpListener> {
        todo!("TcpListener::bind({})", address)
    }

    /// Returns the local socket address of this listener.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        todo!("TcpListener::local_addr()")
    }

    /// Accept a new [`TcpStream`].
    ///
    /// If an accepted stream is returned, the remote address of the peer is
    /// returned along with it.
    pub fn accept<'l>(&'l self) -> Result<Accept<'l>, QueueFull> {
        let address: MaybeUninit<libc::sockaddr_storage> = MaybeUninit::uninit();
        let length = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
        let mut address = Box::new((address, length));

        self.state.start(|submission| unsafe {
            submission.accept(self.fd, &mut address.0, &mut address.1, libc::SOCK_CLOEXEC);
        })?;

        Ok(Accept {
            address: Some(address),
            fd: self,
        })
    }
}

impl Drop for TcpListener {
    fn drop(&mut self) {
        let result = self
            .state
            .start(|submission| unsafe { submission.close_fd(self.fd) });
        if let Err(err) = result {
            log::error!("error closing TCP listener: {}", err);
        }
    }
}

op_future! {
    fn TcpListener::accept -> (TcpStream, SocketAddr),
    struct Accept<'l> {
        /// Address for the accepted connection, needs to stay in memory so the
        /// kernel can access it safely.
        address: Option<Box<(MaybeUninit<libc::sockaddr_storage>, libc::socklen_t)>>, "dropped `a10::net::Accept` before completion, leaking address buffer",
    },
    |this, fd| {
        let sq = this.fd.state.submission_queue();
        let state = SharedOperationState::new(sq);
        let stream = TcpStream { fd, state };

        let address = this.address.take().unwrap();
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

/// A TCP stream between a local socket and a remote socket.
pub struct TcpStream {
    fd: RawFd,
    state: SharedOperationState,
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        let result = self
            .state
            .start(|submission| unsafe { submission.close_fd(self.fd) });
        if let Err(err) = result {
            log::error!("error closing TCP stream: {}", err);
        }
    }
}
