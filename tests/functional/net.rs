use std::cell::Cell;
use std::io::{self, Read, Write};
use std::mem::{self, size_of};
use std::net::{
    Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, SocketAddrV6, TcpListener, TcpStream,
    UdpSocket,
};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::sync::{Arc, OnceLock};
use std::{ptr, thread};

#[cfg(any(target_os = "android", target_os = "linux"))]
use a10::io::ReadBufPool;
use a10::net::{
    Accept, Bind, Connect, NoAddress, Recv, RecvN, RecvNVectored, Send, SendAll, SendAllVectored,
    SendTo, Socket, SocketName,
};
#[cfg(any(target_os = "android", target_os = "linux"))]
use a10::net::{Domain, MultishotAccept, MultishotRecv, Type, socket};
use a10::{Extract, SubmissionQueue};

use crate::util::{
    BadBuf, BadBufSlice, BadReadBuf, BadReadBufSlice, Waker, bind_and_listen_ipv4, bind_ipv4,
    expect_io_error_kind, fd, ignore_unsupported, is_send, is_sync, syscall, tcp_ipv4_socket,
    test_queue, udp_ipv4_socket,
};
#[cfg(any(target_os = "android", target_os = "linux"))]
use crate::util::{expect_io_errno, next};

const DATA1: &[u8] = b"Hello, World!";
const DATA2: &[u8] = b"Hello, Mars!";

#[test]
fn socket_is_send_and_sync() {
    is_send::<Socket>();
    is_sync::<Socket>();
}

#[test]
fn bind_is_send_and_sync() {
    is_send::<Bind<SocketAddr>>();
    is_sync::<Bind<SocketAddr>>();
}

#[test]
fn accept() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Accept<SocketAddr>>();
    is_sync::<Accept<SocketAddr>>();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    let local_addr = waker.block_on(bind_and_listen_ipv4(&listener));

    // Accept a connection.
    let mut stream = TcpStream::connect(local_addr).expect("failed to connect");
    let accept = listener.accept::<SocketAddr>();
    let (client, address) = waker.block_on(accept).expect("failed to accept connection");
    assert_eq!(stream.peer_addr().unwrap(), local_addr);
    assert_eq!(stream.local_addr().unwrap(), address.into());

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
fn accept_no_address() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<Accept<SocketAddr>>();
    is_sync::<Accept<SocketAddr>>();

    // Bind a socket.
    let listener = waker.block_on(tcp_ipv4_socket(sq));
    let local_addr = waker.block_on(bind_and_listen_ipv4(&listener));

    // Accept a connection.
    let mut stream = TcpStream::connect(local_addr).expect("failed to connect");
    assert_eq!(stream.peer_addr().unwrap(), local_addr);
    let accept = listener.accept::<NoAddress>();
    let (client, _) = waker.block_on(accept).expect("failed to accept connection");

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
#[cfg(any(target_os = "android", target_os = "linux"))]
fn multishot_accept() {
    test_multishot_accept(0);
    test_multishot_accept(1);
    test_multishot_accept(5);

    fn test_multishot_accept(n: usize) {
        let sq = test_queue();
        let waker = Waker::new();

        is_send::<MultishotAccept>();
        is_sync::<MultishotAccept>();

        // Bind a socket.
        let listener = waker.block_on(tcp_ipv4_socket(sq));
        let local_addr = waker.block_on(bind_and_listen_ipv4(&listener));

        let mut accept_stream = listener.multishot_accept();

        // Create connections and accept them.
        let streams = (0..n)
            .map(|_| {
                let stream = TcpStream::connect(local_addr).expect("failed to connect");
                let addr = stream.local_addr().expect("failed to get address");
                (stream, addr)
            })
            .collect::<Vec<_>>();
        let mut clients = (0..n)
            .map(|_| {
                let client = waker
                    .block_on(next(&mut accept_stream))
                    .expect("missing a connection")
                    .expect("failed to accept connection");
                let addr = peer_addr(fd(&client)).expect("failed to get address");
                (client, addr)
            })
            .collect::<Vec<_>>();

        // Make sure we use the correct stream, client pair.
        let mut tests = Vec::with_capacity(clients.len());
        for (stream, addr) in streams {
            let idx = clients
                .iter()
                .position(|(_, a)| *a == addr)
                .expect("failed to find client");
            let client = clients.remove(idx);
            tests.push((stream, client.0));
        }

        // Test each connection.
        for (mut stream, client) in tests {
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
    }
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn multishot_accept_incorrect_usage() {
    let sq = test_queue();
    let waker = Waker::new();

    // Create a socket, but don't bind it.
    let listener = waker.block_on(tcp_ipv4_socket(sq));

    let mut accept_stream = listener.multishot_accept();

    let res = waker.block_on(next(&mut accept_stream)).unwrap();
    assert!(res.is_err(), "unexpected ok result: {:?}", res);
    assert!(waker.block_on(next(&mut accept_stream)).is_none());
}

#[test]
fn connect() {
    is_send::<Connect<SocketAddr>>();
    is_sync::<Connect<SocketAddr>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");

            let mut buf = vec![0; DATA1.len() + 1];
            let n = client.read(&mut buf).expect("failed to read");
            assert_eq!(&buf[0..n], DATA1);

            client.write_all(DATA2).expect("failed to write");

            let n = client.read(&mut buf).expect("failed to read");
            assert_eq!(n, 0);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Write some data.
            stream.write(DATA1).await.expect("failed to write");

            // Read some data.
            let buf = Vec::with_capacity(DATA2.len() + 1);
            let buf = stream.read(buf).await.expect("failed to read");
            assert_eq!(buf, DATA2);

            // Dropping the stream should close it.
            drop(stream);
        },
    );
}

#[test]
fn socket_name() {
    is_send::<SocketName<SocketAddr>>();
    is_sync::<SocketName<SocketAddr>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let expected_peer_addr = listener.local_addr().unwrap();

    let expected_local_addr = Arc::new(OnceLock::new());
    let e = expected_local_addr.clone();

    conn_test(
        move || {
            let (_client, got) = listener.accept().expect("failed to accept connection");
            e.set(got).unwrap()
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream
                .connect(expected_peer_addr)
                .await
                .expect("failed to connect");

            let got_local_addr: SocketAddr =
                ignore_unsupported!(stream.local_addr().await).expect("failed to get local addr");
            assert_eq!(got_local_addr, *expected_local_addr.wait());

            let got_peer_addr: SocketAddr =
                stream.peer_addr().await.expect("failed to get peer addr");
            assert_eq!(got_peer_addr, expected_peer_addr);
        },
    );
}

#[test]
fn recv() {
    is_send::<Recv<Vec<u8>>>();
    is_sync::<Recv<Vec<u8>>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            client.write_all(DATA1).expect("failed to send data");
            drop(client);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Receive some data.
            let mut buf = stream
                .recv(Vec::with_capacity(DATA1.len() + 1))
                .await
                .expect("failed to receive");
            assert_eq!(&buf, DATA1);

            // We should detect the peer closing the stream.
            buf.clear();
            let buf = stream.recv(buf).await.expect("failed to receive");
            assert!(buf.is_empty());
        },
    );
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn recv_read_buf_pool() {
    const BUF_SIZE: usize = 4096;

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            client.write_all(DATA1).expect("failed to send data");
            drop(client);
        },
        |sq| async move {
            let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Receive some data.
            let recv_future = stream.recv(buf_pool.get());
            let buf = recv_future.await.expect("failed to receive");
            assert_eq!(buf.as_slice(), DATA1);

            // We should detect the peer closing the stream.
            let buf = stream
                .recv(buf_pool.get())
                .await
                .expect("failed to receive");
            assert!(buf.is_empty());
        },
    );
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn recv_read_buf_pool_send_read_buf() {
    const BUF_SIZE: usize = 4096;

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");

            client.write_all(DATA1).expect("failed to send data");

            let mut buf = vec![0; DATA1.len() + 1];
            let n = client.read(&mut buf).expect("failed to read data");
            assert_eq!(n, DATA1.len());
            assert_eq!(&buf[0..n], DATA1);

            drop(client);
        },
        |sq| async move {
            let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Receive some data.
            let buf = stream
                .recv(buf_pool.get())
                .await
                .expect("failed to receive");
            assert_eq!(buf.as_slice(), DATA1);

            // Send the data back.
            let n = stream.send(buf).await.expect("failed to send");
            assert_eq!(n, DATA1.len());

            // We should detect the peer closing the stream.
            let buf = stream
                .recv(buf_pool.get())
                .await
                .expect("failed to receive");
            assert!(buf.is_empty());
        },
    );
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn multishot_recv() {
    const BUF_SIZE: usize = 512;
    const BUFS: usize = 2;

    is_send::<MultishotRecv>();
    is_sync::<MultishotRecv>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");

            client.write_all(DATA1).expect("failed to write");

            client.shutdown(Shutdown::Write).unwrap();
        },
        |sq| async move {
            let buf_pool = ReadBufPool::new(sq.clone(), BUFS as u16, BUF_SIZE as u32).unwrap();

            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            let mut stream_recv = stream.multishot_recv(buf_pool);

            // Write some data and read it back.
            let buf = next(&mut stream_recv)
                .await
                .unwrap()
                .expect("failed to receive");
            assert_eq!(&*buf, DATA1);

            let buf = next(&mut stream_recv)
                .await
                .unwrap()
                .expect("failed to receive");
            assert!(buf.is_empty(), "unexpected buf: {buf:?}");
            let res = next(&mut stream_recv).await;
            assert!(res.is_none(), "unexpected result: {res:?}");
        },
    );
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn multishot_recv_large_send() {
    const BUF_SIZE: usize = 512;
    const BUFS: usize = 2;
    const N: usize = 4;
    const DATA: &[u8] = &[123; N * 4];

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            client.write_all(DATA).expect("failed to write");
            client.shutdown(Shutdown::Write).unwrap();
        },
        |sq| async move {
            let buf_pool = ReadBufPool::new(sq.clone(), BUFS as u16, BUF_SIZE as u32).unwrap();

            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            let mut stream_recv = stream.multishot_recv(buf_pool);

            // Write some data and read it back.
            let mut data_left = DATA;
            while !data_left.is_empty() {
                let buf = next(&mut stream_recv)
                    .await
                    .unwrap()
                    .expect("failed to receive");
                assert_eq!(&*buf, &data_left[..buf.len()]);
                data_left = &data_left[buf.len()..];
            }

            let buf = next(&mut stream_recv)
                .await
                .unwrap()
                .expect("failed to receive");
            assert!(buf.is_empty(), "unexpected buf: {buf:?}");
            let res = next(&mut stream_recv).await;
            assert!(res.is_none(), "unexpected result: {res:?}");
        },
    );
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn multishot_recv_all_buffers_used() {
    const BUF_SIZE: usize = 512;
    const BUFS: usize = 2;
    const N: usize = 2 + 10;
    const DATA: &[u8] = &[255; BUF_SIZE];

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            // Write some much data that all buffers are used.
            for _ in 0..N {
                client.write_all(DATA).expect("failed to write");
            }
            client.shutdown(Shutdown::Write).unwrap();
        },
        |sq| async move {
            let buf_pool = ReadBufPool::new(sq.clone(), BUFS as u16, BUF_SIZE as u32).unwrap();

            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            let mut stream_recv = stream.multishot_recv(buf_pool);

            for i in 0..N {
                let result = next(&mut stream_recv).await.unwrap();
                match result {
                    Ok(buf) => assert_eq!(&*buf, DATA),
                    Err(err) => {
                        // Should have at least read `BUFS` times, after that we should
                        // get a `ENOBUFS` error.
                        // However depending on the timing and internal ordering (with
                        // regards to other operation/processes/etc.) it's possible we
                        // get more than `BUFS` buffers as they are put back into the
                        // pool.
                        assert!(i >= BUFS);
                        expect_io_errno(io::Result::<()>::Err(err), libc::ENOBUFS);
                        break;
                    }
                }
            }
        },
    );
}

#[test]
fn recv_n() {
    is_send::<RecvN<Vec<u8>>>();
    is_sync::<RecvN<Vec<u8>>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            client.write_all(DATA1).expect("failed to send data");
            drop(client);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Receive some data.
            let buf = BadReadBuf {
                data: Vec::with_capacity(30),
            };
            let mut buf = stream.recv_n(buf, DATA1.len()).await.unwrap();
            assert_eq!(&buf.data, DATA1);

            // We should detect the peer closing the stream.
            buf.data.clear();
            let res = stream.recv_n(buf, 5).await;
            expect_io_error_kind(res, io::ErrorKind::UnexpectedEof);
        },
    );
}

#[test]
fn recv_vectored() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            client.write_all(DATA1).expect("failed to send data");
            drop(client);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Receive some data.
            let bufs = [
                Vec::with_capacity(5),
                Vec::with_capacity(2),
                Vec::with_capacity(7),
            ];
            let (mut bufs, flags) = stream.recv_vectored(bufs).await.expect("failed to receive");
            assert_eq!(&bufs[0], b"Hello");
            assert_eq!(&bufs[1], b", ");
            assert_eq!(&bufs[2], b"World!");
            assert_eq!(flags, 0);

            // We should detect the peer closing the stream.
            for buf in bufs.iter_mut() {
                buf.clear();
            }
            let (bufs, flags) = stream.recv_vectored(bufs).await.expect("failed to receive");
            assert!(bufs[0].is_empty());
            assert!(bufs[1].is_empty());
            assert!(bufs[2].is_empty());
            assert_eq!(flags, 0);
        },
    );
}

#[test]
fn recv_vectored_truncated() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind socket");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));
    waker
        .block_on(socket.connect(local_addr))
        .expect("failed to connect");

    let socket_addr = sock_addr(fd(&socket)).expect("failed to get local address");
    listener
        .send_to(DATA1, socket_addr)
        .expect("failed to send data");

    // Receive some data.
    let bufs = [Vec::with_capacity(5), Vec::with_capacity(2)];
    let (bufs, flags) = waker
        .block_on(socket.recv_vectored(bufs))
        .expect("failed to receive");
    assert_eq!(&bufs[0], b"Hello");
    assert_eq!(&bufs[1], b", ");
    assert_eq!(flags, libc::MSG_TRUNC);
}

#[test]
fn recv_n_vectored() {
    const DATA: &[u8] = b"Hello marsBooo!! Hi. How are you?";

    is_send::<RecvNVectored<[Vec<u8>; 2], 2>>();
    is_sync::<RecvNVectored<[Vec<u8>; 2], 2>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            client.write_all(DATA).expect("failed to send data");
            drop(client);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Receive some data.
            let bufs = BadReadBufSlice {
                data: [Vec::with_capacity(15), Vec::with_capacity(20)],
            };
            let mut bufs = stream.recv_n_vectored(bufs, DATA.len()).await.unwrap();
            assert_eq!(&bufs.data[0], b"Hello mars! Hi.");
            assert_eq!(&bufs.data[1], b"Booo! How are you?");

            // We should detect the peer closing the stream.
            for buf in bufs.data.iter_mut() {
                buf.clear();
            }
            let res = stream.recv_n_vectored(bufs, 5).await;
            expect_io_error_kind(res, io::ErrorKind::UnexpectedEof);
        },
    );
}

#[test]
fn recv_from() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind socket");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));
    waker.block_on(bind_ipv4(&socket));
    let socket_addr = sock_addr(fd(&socket)).expect("failed to get local address");

    listener
        .send_to(DATA1, socket_addr)
        .expect("failed to send data");

    // Receive some data.
    let (buf, address, flags): (_, SocketAddr, _) = waker
        .block_on(socket.recv_from(Vec::with_capacity(DATA1.len() + 1)))
        .expect("failed to receive");
    assert_eq!(buf, DATA1);
    assert_eq!(address, local_addr);
    assert_eq!(flags, 0);
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn recv_from_read_buf_pool() {
    const BUF_SIZE: usize = 4096;

    let sq = test_queue();
    let waker = Waker::new();

    let buf_pool = ReadBufPool::new(sq.clone(), 2, BUF_SIZE as u32).unwrap();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind socket");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));
    waker.block_on(bind_ipv4(&socket));
    let socket_addr = sock_addr(fd(&socket)).expect("failed to get local address");

    listener
        .send_to(DATA1, socket_addr)
        .expect("failed to send data");

    // Receive some data.
    let (buf, address, flags): (_, SocketAddr, _) = waker
        .block_on(socket.recv_from(buf_pool.get()))
        .expect("failed to receive");
    assert_eq!(&*buf, DATA1);
    assert_eq!(address, local_addr);
    assert_eq!(flags, 0);
}

#[test]
fn recv_from_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind socket");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));
    waker.block_on(bind_ipv4(&socket));
    let socket_addr = sock_addr(fd(&socket)).expect("failed to get local address");

    listener
        .send_to(DATA1, socket_addr)
        .expect("failed to send data");

    // Receive some data.
    let bufs = [
        Vec::with_capacity(5),
        Vec::with_capacity(2),
        Vec::with_capacity(7),
    ];
    let (bufs, address, flags): (_, SocketAddr, _) = waker
        .block_on(socket.recv_from_vectored(bufs))
        .expect("failed to receive");
    assert_eq!(&bufs[0], b"Hello");
    assert_eq!(&bufs[1], b", ");
    assert_eq!(&bufs[2], b"World!");
    assert_eq!(address, local_addr);
    assert_eq!(flags, 0);
}

#[test]
fn send() {
    is_send::<Send<Vec<u8>>>();
    is_sync::<Send<Vec<u8>>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; DATA2.len() + 2];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(&buf[0..n], DATA2);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send some data.
            let n = stream.send(DATA2).await.expect("failed to send");
            assert_eq!(n, DATA2.len());
        },
    );
}

#[test]
fn send_zc() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; DATA2.len() + 2];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(&buf[0..n], DATA2);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send some data.
            let n = stream.send(DATA2).zc().await.expect("failed to send");
            assert_eq!(n, DATA2.len());
        },
    );
}

#[test]
fn send_extractor() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; DATA2.len() + 2];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(&buf[0..n], DATA2);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send some data.
            let (buf, n) = stream.send(DATA2).extract().await.expect("failed to send");
            assert_eq!(buf, DATA2);
            assert_eq!(n, DATA2.len());
        },
    );
}

#[test]
fn send_zc_extractor() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; DATA2.len() + 2];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(&buf[0..n], DATA2);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send some data.
            let (buf, n) = stream
                .send(DATA2)
                .zc()
                .extract()
                .await
                .expect("failed to send");
            assert_eq!(buf, DATA2);
            assert_eq!(n, DATA2.len());
        },
    );
}

#[test]
fn send_all() {
    is_send::<SendAll<Vec<u8>>>();
    is_sync::<SendAll<Vec<u8>>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = Vec::with_capacity(31);
            let n = client.read_to_end(&mut buf).unwrap();
            assert_eq!(n, BadBuf::DATA.len());
            buf.resize(n, 0);
            assert_eq!(buf, BadBuf::DATA);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send all data.
            stream
                .send_all(BadBuf::new())
                .await
                .expect("failed to send");
        },
    );
}

#[test]
fn send_all_extract() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = Vec::with_capacity(BadBuf::DATA.len() + 1);
            let n = client.read_to_end(&mut buf).unwrap();
            assert_eq!(n, BadBuf::DATA.len());
            buf.resize(n, 0);
            assert_eq!(buf, BadBuf::DATA);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send all data.
            stream
                .send_all(BadBuf::new())
                .extract()
                .await
                .expect("failed to send");
        },
    );
}

#[test]
fn send_vectored() {
    is_send::<Send<Vec<u8>>>();
    is_sync::<Send<Vec<u8>>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; DATA1.len() + 2];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(&buf[0..n], DATA1);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send some data.
            let bufs = ["Hello", ", ", "World!"];
            let n = stream.send_vectored(bufs).await.expect("failed to send");
            assert_eq!(n, DATA1.len());
        },
    );
}

#[test]
fn send_vectored_zc() {
    is_send::<Send<Vec<u8>>>();
    is_sync::<Send<Vec<u8>>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; DATA1.len() + 2];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(&buf[0..n], DATA1);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send some data.
            let bufs = ["Hello", ", ", "World!"];
            let n = stream
                .send_vectored(bufs)
                .zc()
                .await
                .expect("failed to send");
            assert_eq!(n, DATA1.len());
        },
    );
}

#[test]
fn send_vectored_extractor() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; DATA2.len() + 2];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(&buf[0..n], DATA2);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send some data.
            let bufs = ["Hello", ", ", "Mars!"];
            let (bufs, n) = stream
                .send_vectored(bufs)
                .extract()
                .await
                .expect("failed to send");
            assert_eq!(bufs[0], "Hello");
            assert_eq!(n, DATA2.len());
        },
    );
}

#[test]
fn send_vectored_zc_extractor() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; DATA2.len() + 2];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(&buf[0..n], DATA2);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send some data.
            let bufs = ["Hello", ", ", "Mars!"];
            let (bufs, n) = stream
                .send_vectored(bufs)
                .zc()
                .extract()
                .await
                .expect("failed to send");
            assert_eq!(bufs[0], "Hello");
            assert_eq!(n, DATA2.len());
        },
    );
}

#[test]
fn send_all_vectored() {
    is_send::<SendAllVectored<[Vec<u8>; 2], 2>>();
    is_sync::<SendAllVectored<[Vec<u8>; 2], 2>>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = Vec::with_capacity(31);
            let n = client.read_to_end(&mut buf).unwrap();
            assert_eq!(n, 30);
            buf.resize(n, 0);
            assert_eq!(&buf[..10], BadBufSlice::DATA1);
            assert_eq!(&buf[10..20], BadBufSlice::DATA2);
            assert_eq!(&buf[20..], BadBufSlice::DATA3);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send all data.
            let bufs = BadBufSlice {
                calls: Cell::new(0),
            };
            stream.send_all_vectored(bufs).await.unwrap();
        },
    );
}

#[test]
fn send_all_vectored_extract() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = Vec::with_capacity(31);
            let n = client.read_to_end(&mut buf).unwrap();
            assert_eq!(n, 30);
            buf.resize(n, 0);
            assert_eq!(&buf[..10], BadBufSlice::DATA1);
            assert_eq!(&buf[10..20], BadBufSlice::DATA2);
            assert_eq!(&buf[20..], BadBufSlice::DATA3);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            // Send all data.
            let bufs = BadBufSlice {
                calls: Cell::new(0),
            };
            let bufs = stream.send_all_vectored(bufs).extract().await.unwrap();
            assert_eq!(bufs.calls.get(), 3);
        },
    );
}

#[test]
fn send_to() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<SendTo<Vec<u8>, SocketAddr>>();
    is_sync::<SendTo<Vec<u8>, SocketAddr>>();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let n = waker
        .block_on(socket.send_to(DATA1, local_addr))
        .expect("failed to send_to");
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn send_to_zc() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let n = waker
        .block_on(socket.send_to(DATA1, local_addr).zc())
        .expect("failed to send_to");
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn send_to_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let (buf, n) = waker
        .block_on(socket.send_to(DATA1, local_addr).extract())
        .expect("failed to send_to");
    assert!(buf == DATA1);
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn send_to_zc_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let (buf, n) = waker
        .block_on(socket.send_to(DATA1, local_addr).zc().extract())
        .expect("failed to send_to");
    assert!(buf == DATA1);
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn send_to_vectored() {
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<SendTo<Vec<u8>, SocketAddr>>();
    is_sync::<SendTo<Vec<u8>, SocketAddr>>();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let bufs = ["Hello", ", ", "World!"];
    let n = waker
        .block_on(socket.send_to_vectored(bufs, local_addr))
        .expect("failed to send_to");
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn send_to_vectored_zc() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let bufs = ["Hello", ", ", "World!"];
    let n = waker
        .block_on(socket.send_to_vectored(bufs, local_addr).zc())
        .expect("failed to send_to");
    assert_eq!(n, DATA1.len());

    let mut buf = vec![0; DATA1.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA1);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn send_to_vectored_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let bufs = ["Hello", ", ", "Mars!"];
    let (buf, n) = waker
        .block_on(socket.send_to_vectored(bufs, local_addr).extract())
        .expect("failed to send_to");
    assert!(buf[2] == "Mars!");
    assert_eq!(n, DATA2.len());

    let mut buf = vec![0; DATA2.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA2);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn send_to_vectored_zc_extractor() {
    let sq = test_queue();
    let waker = Waker::new();

    // Bind a socket.
    let listener = UdpSocket::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    let socket = waker.block_on(udp_ipv4_socket(sq));

    // Send some data.
    let bufs = ["Hello", ", ", "Mars!"];
    let (bufs, n) = waker
        .block_on(socket.send_to_vectored(bufs, local_addr).zc().extract())
        .expect("failed to send_to");
    assert!(bufs[0] == "Hello");
    assert_eq!(n, DATA2.len());

    let mut buf = vec![0; DATA2.len() + 2];
    let (n, from_address) = listener.recv_from(&mut buf).expect("failed to recv data");
    assert_eq!(&buf[0..n], DATA2);
    assert!(from_address.ip().is_loopback());
}

#[test]
fn shutdown() {
    is_send::<Shutdown>();
    is_sync::<Shutdown>();

    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; 10];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(n, 0);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = tcp_ipv4_socket(sq).await;
            stream.connect(local_addr).await.expect("failed to connect");

            stream
                .shutdown(Shutdown::Write)
                .await
                .expect("failed to shutdown");
        },
    );
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn direct_fd() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind listener");
    let local_addr = listener.local_addr().unwrap();

    conn_test(
        move || {
            let (mut client, _) = listener.accept().expect("failed to accept connection");
            let mut buf = vec![0; DATA2.len() + 2];
            let n = client.read(&mut buf).expect("failed to send data");
            assert_eq!(&buf[0..n], DATA2);

            client.write_all(DATA1).expect("failed to send data");

            drop(client);
        },
        |sq| async move {
            // Create a socket and connect the listener.
            let stream = socket(sq, Domain::IPV4, Type::STREAM, None)
                .kind(a10::fd::Kind::Direct)
                .await
                .expect("failed to create socket");
            stream.connect(local_addr).await.expect("failed to connect");

            // Send some data.
            let n = stream.send(DATA2).await.expect("failed to send");
            assert_eq!(n, DATA2.len());

            // Receive some data.
            let mut buf = stream
                .recv(Vec::with_capacity(DATA1.len() + 1))
                .await
                .expect("failed to receive");
            assert_eq!(&buf, DATA1);

            // We should detect the peer closing the stream.
            buf.clear();
            let buf = stream.recv(buf).await.expect("failed to receive");
            assert!(buf.is_empty());
        },
    );
}

/// Run a test for a connection.
/// `f` is called on a separate thread, while `a` is run on the current thread.
pub(crate) fn conn_test<Fut: Future<Output = ()>>(
    f: impl FnOnce() + std::marker::Send + 'static,
    a: impl FnOnce(SubmissionQueue) -> Fut,
) {
    let handle = thread::spawn(f);
    Waker::new().block_on(a(test_queue()));
    handle.join().unwrap()
}

#[cfg(any(target_os = "android", target_os = "linux"))]
fn peer_addr(fd: BorrowedFd) -> io::Result<SocketAddr> {
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let mut len = size_of::<libc::sockaddr_storage>() as u32;
    syscall!(getpeername(
        fd.as_raw_fd(),
        ptr::addr_of_mut!(storage).cast::<libc::sockaddr>(),
        &mut len
    ))?;
    Ok(convert_address(storage, len))
}

fn sock_addr(fd: BorrowedFd) -> io::Result<SocketAddr> {
    let mut storage: libc::sockaddr_storage = unsafe { mem::zeroed() };
    let mut len = size_of::<libc::sockaddr_storage>() as u32;
    syscall!(getsockname(
        fd.as_raw_fd(),
        ptr::addr_of_mut!(storage).cast::<libc::sockaddr>(),
        &mut len
    ))?;
    Ok(convert_address(storage, len))
}

fn convert_address(storage: libc::sockaddr_storage, len: libc::socklen_t) -> SocketAddr {
    if storage.ss_family == libc::AF_INET as libc::sa_family_t {
        assert!(len == size_of::<libc::sockaddr_in>() as libc::socklen_t);
        let storage = unsafe { &*ptr::addr_of!(storage).cast::<libc::sockaddr_in>() };
        let addr = Ipv4Addr::from(storage.sin_addr.s_addr.to_ne_bytes());
        let port = storage.sin_port.to_be();
        SocketAddr::V4(SocketAddrV4::new(addr, port))
    } else if storage.ss_family == libc::AF_INET6 as libc::sa_family_t {
        assert!(len == size_of::<libc::sockaddr_in6>() as libc::socklen_t);
        let storage = unsafe { &*ptr::addr_of!(storage).cast::<libc::sockaddr_in6>() };
        let addr = Ipv6Addr::from(storage.sin6_addr.s6_addr);
        let port = storage.sin6_port.to_be();
        let flowinfo = storage.sin6_flowinfo;
        let scope_id = storage.sin6_scope_id;
        SocketAddr::V6(SocketAddrV6::new(addr, port, flowinfo, scope_id))
    } else {
        panic!("invalid socket storage type: {}", storage.ss_family)
    }
}
