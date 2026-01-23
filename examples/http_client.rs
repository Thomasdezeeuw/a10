//! Very simple HTTP/1.1 client.
//!
//! Run with:
//! $ cargo run --example http_client -- thomasdezeeuw.nl

use std::net::SocketAddr;
use std::{env, io, str};

use a10::net::{Domain, Type, socket};
use a10::{Ring, SubmissionQueue};

mod runtime;

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = Ring::new()?;

    // Read the host, e.g. thomasdezeeuw.nl, this doesn't accept a scheme, path
    // or anything else.
    let mut host = env::args()
        .nth(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing host"))?;
    if !host.contains(':') {
        // Add port 80 for `ToSocketAddrs`.
        let insert_idx = host.find('/').unwrap_or(host.len());
        host.insert_str(insert_idx, ":80");
    }
    let addr_host = host.split_once('/').map_or(host.as_str(), |(h, _)| h);

    // Get an IPv4 address for the domain (using blocking I/O).
    let address = std::net::ToSocketAddrs::to_socket_addrs(&addr_host)?
        .next()
        .ok_or_else(|| io::Error::other("failed to lookup ip"))?;

    // Create our future that makes the request.
    let request_future = request(ring.sq(), &host, address);

    // Use our fake runtime to poll the future.
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
async fn request(sq: SubmissionQueue, host: &str, address: SocketAddr) -> io::Result<Vec<u8>> {
    // Create a new TCP socket.
    let socket = socket(sq, Domain::for_address(&address), Type::STREAM, None).await?;

    // Connect.
    socket.connect(address).await?;

    // Send a HTTP GET / request to the socket.
    let host = host.split_once(':').map_or(host, |(h, _)| h);
    let version = env!("CARGO_PKG_VERSION");
    let request = format!(
        "GET / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: A10-example/{version}\r\nAccept: */*\r\n\r\n"
    );
    socket.send_all(request).await?;

    // Receiving the response.
    let recv_buf = socket.recv(Vec::with_capacity(8192)).await?;

    // We'll explicitly close the socket, although that happens for us when we
    // drop the socket. In other words, this is not needed.
    socket.close().await?;

    Ok(recv_buf)
}
