//! Tests for the networking operations.

use std::io::{Read, Write};
use std::mem;
use std::net::{Shutdown, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::pin::Pin;

use a10::io::ReadBufPool;
use a10::{Extract, Ring};

use crate::util::{bind_ipv4, block_on, init, poll_nop, tcp_ipv4_socket, test_queue, Waker};

const DATA1: &[u8] = b"Hello, World!";
const DATA2: &[u8] = b"Hello, Mars!";

#[test]
fn accept() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    let local_addr = bind_ipv4(&listener);

    // Accept a connection.
    let mut stream = TcpStream::connect(local_addr).expect("failed to connect");
    let accept = listener.accept();
    let (client, addr) = waker.block_on(accept).expect("failed to accept connection");
    assert_eq!(stream.peer_addr().unwrap(), local_addr);
    assert_eq!(stream.local_addr().unwrap(), addr);

    // Read some data.
    stream.write(DATA1).expect("failed to write");
    let mut buf = waker
        .block_on(client.read(Vec::with_capacity(DATA1.len() + 1)))
        .expect("failed to read");
    assert_eq!(buf, DATA1);

    // Write some data.
    let n = waker
        .block_on(client.write(DATA2))
        .expect("failed to write");
    assert_eq!(n, DATA2.len());
    buf.resize(DATA2.len() + 1, 0);
    let n = stream.read(&mut buf).expect("failed to read");
    assert_eq!(&buf[..n], DATA2);

    // Closing the client should get a result.
    drop(stream);
    buf.clear();
    let buf = waker.block_on(client.read(buf)).expect("failed to read");
    assert!(buf.is_empty());
}

#[test]
fn connect() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = {
        let mut addr: libc::sockaddr_storage = unsafe { mem::zeroed() };
        let a = unsafe { &mut *(&mut addr as *mut _ as *mut libc::sockaddr_in) };
        a.sin_family = libc::AF_INET as libc::sa_family_t;
        a.sin_port = local_addr.port().to_be();
        a.sin_addr = libc::in_addr {
            s_addr: u32::from_ne_bytes(local_addr.ip().octets()),
        };
        addr
    };
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Write some data.
    waker
        .block_on(stream.write(DATA1))
        .expect("failed to write");
    let mut buf = vec![0; DATA1.len() + 1];
    let n = client.read(&mut buf).expect("failed to read");
    assert_eq!(&buf[0..n], DATA1);

    // Read some data.
    client.write_all(DATA2).expect("failed to write");
    buf.clear();
    buf.reserve(DATA2.len() + 1);
    let mut buf = waker.block_on(stream.read(buf)).expect("failed to read");
    assert_eq!(buf, DATA2);

    // Dropping the stream should closing it.
    drop(stream);
    let n = client.read(&mut buf).expect("failed to read");
    assert_eq!(n, 0);
}

#[test]
fn connect_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len).extract();
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    let _addr = waker.block_on(connect_future).expect("failed to connect");

    // Write some data.
    waker
        .block_on(stream.write(DATA1))
        .expect("failed to write");
    let mut buf = vec![0; DATA1.len() + 1];
    let n = client.read(&mut buf).expect("failed to read");
    assert_eq!(&buf[0..n], DATA1);

    // Read some data.
    client.write_all(DATA2).expect("failed to write");
    buf.clear();
    buf.reserve(DATA2.len() + 1);
    let buf = waker.block_on(stream.read(buf)).expect("failed to read");
    assert_eq!(buf, DATA2);
}

#[test]
fn recv() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Receive some data.
    let recv_future = stream.recv(Vec::with_capacity(DATA1.len() + 1), 0);
    client.write_all(DATA1).expect("failed to send data");
    let mut buf = waker.block_on(recv_future).expect("failed to receive");
    assert_eq!(&buf, DATA1);

    // We should detect the peer closing the stream.
    drop(client);
    buf.clear();
    let buf = waker
        .block_on(stream.recv(buf, 0))
        .expect("failed to receive");
    assert!(buf.is_empty());
}

#[test]
fn recv_read_buf_pool() {
    const BUF_SIZE: usize = 4096;

    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = block_on(&mut ring, tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    block_on(&mut ring, connect_future).expect("failed to connect");

    // Receive some data.
    let recv_future = stream.recv(buf_pool.get(), 0);
    client.write_all(DATA1).expect("failed to send data");
    let buf = block_on(&mut ring, recv_future).expect("failed to receive");
    assert_eq!(buf.as_slice(), DATA1);

    // We should detect the peer closing the stream.
    drop(client);
    let buf = block_on(&mut ring, stream.recv(buf_pool.get(), 0)).expect("failed to receive");
    assert!(buf.is_empty());
}

#[test]
fn recv_read_buf_pool_send_read_buf() {
    const BUF_SIZE: usize = 4096;

    init();
    let mut ring = Ring::new(2).expect("failed to create test ring");
    let sq = ring.submission_queue().clone();
    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = block_on(&mut ring, tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    block_on(&mut ring, connect_future).expect("failed to connect");

    // Receive some data.
    let recv_future = stream.recv(buf_pool.get(), 0);
    client.write_all(DATA1).expect("failed to send data");
    let buf = block_on(&mut ring, recv_future).expect("failed to receive");
    assert_eq!(buf.as_slice(), DATA1);

    // Send the data back.
    let n = block_on(&mut ring, stream.send(buf, 0)).expect("failed to send");
    assert_eq!(n, DATA1.len());
    let mut buf = vec![0; DATA1.len() + 1];
    let n = client.read(&mut buf).expect("failed to read data");
    assert_eq!(n, DATA1.len());
    assert_eq!(&buf[0..n], DATA1);

    // We should detect the peer closing the stream.
    drop(client);
    let buf = block_on(&mut ring, stream.recv(buf_pool.get(), 0)).expect("failed to receive");
    assert!(buf.is_empty());
}

#[test]
fn send() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let n = waker
        .block_on(stream.send(DATA2, 0))
        .expect("failed to send");
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn send_zc() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let n = waker
        .block_on(stream.send_zc(DATA2, 0))
        .expect("failed to send");
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn send_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let (buf, n) = waker
        .block_on(stream.send(DATA2, 0).extract())
        .expect("failed to send");
    assert_eq!(buf, DATA2);
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn send_zc_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    // Send some data.
    let (buf, n) = waker
        .block_on(stream.send_zc(DATA2, 0).extract())
        .expect("failed to send");
    assert_eq!(buf, DATA2);
    assert_eq!(n, DATA2.len());
    let mut buf = vec![0; DATA2.len() + 2];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(&buf[0..n], DATA2);
}

#[test]
fn shutdown() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = match listener.local_addr().unwrap() {
        SocketAddr::V4(addr) => addr,
        _ => unreachable!(),
    };

    // Create a socket and connect the listener.
    let stream = waker.block_on(tcp_ipv4_socket(sq));
    let addr = addr_storage(&local_addr);
    let addr_len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
    let mut connect_future = stream.connect(addr, addr_len);
    // Poll the future to schedule the operation.
    assert!(poll_nop(Pin::new(&mut connect_future)).is_pending());

    let (mut client, _) = listener.accept().expect("failed to accept connection");

    waker.block_on(connect_future).expect("failed to connect");

    waker
        .block_on(stream.shutdown(Shutdown::Write))
        .expect("failed to shutdown");
    let mut buf = vec![0; 10];
    let n = client.read(&mut buf).expect("failed to send data");
    assert_eq!(n, 0);
}

fn addr_storage(addres: &SocketAddrV4) -> libc::sockaddr_storage {
    // SAFETY: zeroed out `sockaddr_storage` is valid.
    let mut addr: libc::sockaddr_storage = unsafe { mem::zeroed() };
    addr.ss_family = libc::AF_INET as libc::sa_family_t;
    // SAFETY: `sockaddr_in` is a valid variant size we se `AF_INET` above.
    let a = unsafe { &mut *(&mut addr as *mut _ as *mut libc::sockaddr_in) };
    a.sin_port = addres.port().to_be();
    a.sin_addr = libc::in_addr {
        s_addr: u32::from_ne_bytes(addres.ip().octets()),
    };
    addr
}