use std::fmt;

use a10::net::{Domain, Protocol, SetSocketOption, SocketOption, Type, option};

use crate::util::{Waker, is_send, is_sync, new_socket, test_queue};

#[test]
fn async_fd_socket_option_is_send_and_sync() {
    is_send::<SocketOption<option::ReuseAddress>>();
    is_sync::<SocketOption<option::ReuseAddress>>();
}

#[test]
fn async_fd_set_socket_options_is_send_and_sync() {
    is_send::<SetSocketOption<option::ReuseAddress>>();
    is_sync::<SetSocketOption<option::ReuseAddress>>();
}

#[test]
#[cfg(any(
    target_os = "android",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd"
))]
fn socket_option_accept() {
    test_socket_option::<option::Accept, _>(|got| assert!(!got));
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn socket_option_domain() {
    test_socket_option::<option::Domain, _>(|got| assert_eq!(got, Domain::IPV4));
}

#[test]
fn socket_option_error() {
    test_socket_option::<option::Error, _>(|got| assert!(got.is_none()));
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn socket_option_protocol() {
    test_socket_option::<option::Protocol, _>(|got| assert_eq!(got, Protocol::TCP));
}

#[test]
fn socket_option_type() {
    test_socket_option::<option::Type, _>(|got| assert_eq!(got, Type::STREAM));
}

fn test_socket_option<T: option::Get, F: FnOnce(T::Output)>(assert: F) {
    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(new_socket(
        sq,
        Domain::IPV4,
        Type::STREAM,
        Some(Protocol::TCP),
    ));

    let got = waker
        .block_on(socket.socket_option::<T>())
        .expect("failed to get socket option");
    assert(got);
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn socket_option_incoming_cpu() {
    test_get_set_socket_option::<option::IncomingCpu>(None, 0, Some(0));
}

#[test]
fn socket_option_reuse_address() {
    test_get_set_socket_option::<option::ReuseAddress>(false, true, true);
}

#[test]
fn socket_option_reuse_port() {
    test_get_set_socket_option::<option::ReusePort>(false, true, true);
}

#[test]
fn socket_option_keep_alive() {
    test_get_set_socket_option::<option::KeepAlive>(false, true, true);
}

#[test]
fn socket_option_linger() {
    test_get_set_socket_option::<option::Linger>(None, Some(10), Some(10));
}

fn test_get_set_socket_option<T>(expected_initial: T::Output, set: T::Value, expected: T::Output)
where
    T: option::Get + option::Set,
    T::Output: Eq + fmt::Debug,
{
    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(new_socket(sq, Domain::IPV4, Type::STREAM, None));

    let got_initial = waker
        .block_on(socket.socket_option::<T>())
        .expect("failed to get initial socket option");
    assert_eq!(got_initial, expected_initial);

    waker
        .block_on(socket.set_socket_option::<T>(set))
        .expect("failed to set socket option");

    let got = waker
        .block_on(socket.socket_option::<T>())
        .expect("failed to get socket option");
    assert_eq!(got, expected);
}
