use std::fmt;

use a10::net::{
    Domain, Level, Protocol, SetSocketOption, SetSocketOption2, SocketOpt, SocketOption,
    SocketOption2, Type, option,
};

use crate::util::{Waker, is_send, is_sync, new_socket, require_kernel, test_queue};

#[test]
fn async_fd_socket_option_is_send_and_sync() {
    is_send::<SocketOption<libc::c_int>>();
    is_sync::<SocketOption<libc::c_int>>();
}

#[test]
fn async_fd_set_socket_option_is_send_and_sync() {
    is_send::<SetSocketOption<libc::c_int>>();
    is_sync::<SetSocketOption<libc::c_int>>();
}

#[test]
fn async_fd_socket_option2_is_send_and_sync() {
    is_send::<SocketOption2<option::ReuseAddress>>();
    is_sync::<SocketOption2<option::ReuseAddress>>();
}

#[test]
fn async_fd_set_socket_options_is_send_and_sync() {
    is_send::<SetSocketOption2<option::ReuseAddress>>();
    is_sync::<SetSocketOption2<option::ReuseAddress>>();
}

#[test]
fn socket_option() {
    require_kernel!(6, 7);

    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(new_socket(sq, Domain::IPV4, Type::STREAM, None));

    let got_domain = waker
        .block_on(socket.socket_option(Level::SOCKET, SocketOpt::DOMAIN))
        .unwrap();
    assert_eq!(libc::AF_INET, got_domain);

    let got_type = waker
        .block_on(socket.socket_option(Level::SOCKET, SocketOpt::TYPE))
        .unwrap();
    assert_eq!(libc::SOCK_STREAM, got_type);

    let got_protocol = waker
        .block_on(socket.socket_option(Level::SOCKET, SocketOpt::PROTOCOL))
        .unwrap();
    assert_eq!(libc::IPPROTO_TCP, got_protocol);

    let got_linger = waker
        .block_on(socket.socket_option::<libc::linger>(Level::SOCKET, SocketOpt::LINGER))
        .unwrap();
    assert_eq!(0, got_linger.l_onoff);
    assert_eq!(0, got_linger.l_linger);

    let got_error = waker
        .block_on(socket.socket_option::<libc::c_int>(Level::SOCKET, SocketOpt::ERROR))
        .unwrap();
    assert_eq!(0, got_error);
}

#[test]
fn socket_option_accept() {
    test_socket_option::<option::Accept, _>(|got| assert!(!got));
}

#[test]
fn socket_option_domain() {
    test_socket_option::<option::Domain, _>(|got| assert_eq!(got, Domain::IPV4));
}

#[test]
fn socket_option_error() {
    test_socket_option::<option::Error, _>(|got| assert!(got.is_none()));
}

#[test]
fn socket_option_protocol() {
    test_socket_option::<option::Protocol, _>(|got| assert_eq!(got, Protocol::TCP));
}

fn test_socket_option<T: option::Get, F: FnOnce(T::Output)>(assert: F) {
    require_kernel!(6, 7);

    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(new_socket(
        sq,
        Domain::IPV4,
        Type::STREAM,
        Some(Protocol::TCP),
    ));

    let got = waker
        .block_on(socket.socket_option2::<T>())
        .expect("failed to get socket option");
    assert(got);
}

#[test]
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

fn test_get_set_socket_option<T>(expected_initial: T::Output, set: T::Value, expected: T::Output)
where
    T: option::Get + option::Set,
    T::Output: Eq + fmt::Debug,
{
    require_kernel!(6, 7);

    let sq = test_queue();
    let waker = Waker::new();

    let socket = waker.block_on(new_socket(sq, Domain::IPV4, Type::STREAM, None));

    let got_initial = waker
        .block_on(socket.socket_option2::<T>())
        .expect("failed to get initial socket option");
    assert_eq!(got_initial, expected_initial);

    waker
        .block_on(socket.set_socket_option2::<T>(set))
        .expect("failed to set socket option");

    let got = waker
        .block_on(socket.socket_option2::<T>())
        .expect("failed to get socket option");
    assert_eq!(got, expected);
}
