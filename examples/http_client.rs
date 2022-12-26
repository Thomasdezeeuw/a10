use std::net::{SocketAddr, SocketAddrV4};
use std::{env, io, mem, ptr, str};

use a10::net::socket;
use a10::{Ring, SubmissionQueue};

mod runtime;

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = Ring::new(2)?;

    let mut host = env::args()
        .nth(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing host"))?;
    if !host.contains(':') {
        // Add port 80 for `ToSocketAddrs`.
        let insert_idx = host.find('/').unwrap_or(host.len());
        host.insert_str(insert_idx, ":80");
    }
    let addr_host = host.split_once('/').map(|(h, _)| h).unwrap_or(&host);

    // Get an IPv4 address for the domain (using blocking I/O).
    let address = std::net::ToSocketAddrs::to_socket_addrs(&addr_host)?
        .filter(SocketAddr::is_ipv4)
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "failed to lookup ip"))?;
    let address = match address {
        SocketAddr::V4(address) => address,
        SocketAddr::V6(_) => unreachable!(),
    };

    // Create our future that makes the request.
    let request_future = request(ring.submission_queue().clone(), &host, address);

    // Use our fake runtime to poll the future, this basically polls the future
    // and the `a10::Ring` in a loop.
    let response = runtime::block_on(&mut ring, request_future)?;

    // We'll print the response (using ol' fashioned blocking I/O).
    let response = str::from_utf8(&response).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("response doesn't contain UTF-8: {err}"),
        )
    })?;
    println!("{response}");

    Ok(())
}

/// Make a HTTP GET request to `address`.
async fn request(sq: SubmissionQueue, host: &str, address: SocketAddrV4) -> io::Result<Vec<u8>> {
    // Create a new TCP, IPv4 socket.
    let domain = libc::AF_INET;
    let r#type = libc::SOCK_STREAM | libc::SOCK_CLOEXEC;
    let protocol = 0;
    let flags = 0;
    let socket = socket(sq, domain, r#type, protocol, flags)?.await?;

    // Connect.
    let (addr, addr_len) = to_sockaddr_storage(address);
    socket.connect(addr, addr_len)?.await?;

    // Send a HTTP GET / request to the socket.
    let host = host.split_once(':').map(|(h, _)| h).unwrap_or(host);
    let version = env!("CARGO_PKG_VERSION");
    let request = format!("GET / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: A10-example/{version}\r\nAccept: */*\r\n\r\n");
    socket.send(request)?.await?;

    // Receiving the response.
    let recv_buf = socket.recv(Vec::with_capacity(8192))?.await?;

    // We'll explicitly close the socket, although that happens for us when we
    // drop the socket. In other words, this is not needed.
    socket.close()?.await?;

    Ok(recv_buf)
}

fn to_sockaddr_storage(addr: SocketAddrV4) -> (libc::sockaddr_storage, libc::socklen_t) {
    // SAFETY: a `sockaddr_storage` of all zeros is valid.
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let len = {
        let storage: &mut libc::sockaddr_in = unsafe { &mut *ptr::addr_of_mut!(storage).cast() };
        storage.sin_family = libc::AF_INET as _;
        storage.sin_port = addr.port().to_be();
        storage.sin_addr = libc::in_addr {
            s_addr: u32::from_ne_bytes(addr.ip().octets()),
        };
        storage.sin_zero = Default::default();
        mem::size_of::<libc::sockaddr_in>() as _
    };
    (storage, len)
}
