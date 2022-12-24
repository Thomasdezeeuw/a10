//! Tests for the I/O operations.

#![feature(once_cell)]

use std::io;

mod util;
use util::{bind_ipv4, expect_io_errno, expect_io_error_kind, tcp_ipv4_socket, test_queue, Waker};

#[test]
fn cancel_previous_accept() {
    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_ipv4(&listener);

    let accept = listener.accept().unwrap();

    waker
        .block_on(listener.cancel_previous().unwrap())
        .expect("failed to cancel accept call");

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_all_accept() {
    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_ipv4(&listener);

    let accept = listener.accept().unwrap();

    waker
        .block_on(listener.cancel_all().unwrap())
        .expect("failed to cancel all calls");

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_previous_accept_twice() {
    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_ipv4(&listener);

    let accept = listener.accept().unwrap();

    waker
        .block_on(listener.cancel_previous().unwrap())
        .expect("failed to cancel accept call");
    // And again.
    expect_io_error_kind(
        waker.block_on(listener.cancel_previous().unwrap()),
        io::ErrorKind::NotFound,
    );

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_all_accept_twice() {
    let sq = test_queue();
    let waker = Waker::new();

    let listener = waker.block_on(tcp_ipv4_socket(sq));
    bind_ipv4(&listener);

    let accept = listener.accept().unwrap();

    waker
        .block_on(listener.cancel_all().unwrap())
        .expect("failed to cancel all calls");
    // And again. Unlike `cancel_previous`, this should return ok.
    waker
        .block_on(listener.cancel_all().unwrap())
        .expect("failed to cancel all calls");

    expect_io_errno(waker.block_on(accept), libc::ECANCELED);
}

#[test]
fn cancel_previous_no_operation_in_progress() {
    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(tcp_ipv4_socket(sq));

    let cancel = socket.cancel_previous().unwrap();
    expect_io_error_kind(waker.block_on(cancel), io::ErrorKind::NotFound);
}

#[test]
fn cancel_all_no_operation_in_progress() {
    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(tcp_ipv4_socket(sq));

    let cancel = socket.cancel_all().unwrap();
    waker.block_on(cancel).expect("failed to cancel");
}
