//! Asynchronous networking.

use std::io;
use std::mem::{size_of, MaybeUninit};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

use crate::op::SharedOperationState;
use crate::{libc, AsyncFd, QueueFull};

/// Socket related system calls.
impl AsyncFd {
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

        self.state.start(|submission| unsafe {
            submission.accept(self.fd, &mut address.0, &mut address.1, flags);
        })?;

        Ok(Accept {
            address: Some(address),
            fd: self,
        })
    }
}

op_future! {
    fn AsyncFd::accept -> (AsyncFd, SocketAddr),
    struct Accept<'fd> {
        /// Address for the accepted connection, needs to stay in memory so the
        /// kernel can access it safely.
        address: Option<Box<(MaybeUninit<libc::sockaddr_storage>, libc::socklen_t)>>, "dropped `a10::net::Accept` before completion, leaking address buffer",
    },
    |this, fd| {
        let sq = this.fd.state.submission_queue();
        let state = SharedOperationState::new(sq);
        let stream = AsyncFd { fd, state };

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
