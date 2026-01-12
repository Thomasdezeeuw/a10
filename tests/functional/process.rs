//! NOTE: see `tests/signals.rs` for testing of signal handling.

use std::pin::Pin;
use std::process::Command;

use a10::process::{
    self, ChildStatus, ReceiveSignal, Signal, SignalInfo, SignalSet, Signals, ToDirect, WaitId,
    WaitOption,
};

use crate::util::{Waker, is_send, is_sync, poll_nop, require_kernel, test_queue};

#[test]
fn signal_is_send_and_sync() {
    is_send::<Signal>();
    is_sync::<Signal>();
}

#[test]
fn signal_set_is_send_and_sync() {
    is_send::<SignalSet>();
    is_sync::<SignalSet>();
}

#[test]
fn signals_is_send_and_sync() {
    is_send::<Signals>();
    is_sync::<Signals>();
}

#[test]
fn signal_info_is_send_and_sync() {
    is_send::<SignalInfo>();
    is_sync::<SignalInfo>();
}

#[test]
fn to_direct_is_send_and_sync() {
    is_send::<ToDirect>();
    is_sync::<ToDirect>();
}

#[test]
fn wait_id_is_send_and_sync() {
    is_send::<WaitId>();
    is_sync::<WaitId>();
}

#[test]
fn receive_signal_is_send_and_sync() {
    is_send::<ReceiveSignal>();
    is_sync::<ReceiveSignal>();
}

#[test]
fn process_wait_on() {
    require_kernel!(6, 7);
    let sq = test_queue();
    let waker = Waker::new();

    let process = Command::new("true").spawn().unwrap();
    let pid = process.id();

    let info = waker
        .block_on(process::wait_on(sq, &process, Some(WaitOption::EXITED)))
        .expect("failed wait");

    assert_eq!(info.signal(), Signal::CHILD);
    assert_eq!(info.code(), ChildStatus::EXITED);
    assert_eq!(info.pid(), pid as i32);
    assert_eq!(info.status().code(), Some(libc::EXIT_SUCCESS));
}

#[test]
fn process_wait_on_drop_before_complete() {
    require_kernel!(6, 7);
    let sq = test_queue();

    let process = Command::new("sleep").arg("1000").spawn().unwrap();

    let mut future = process::wait_on(sq, &process, Some(WaitOption::EXITED));
    let result = poll_nop(Pin::new(&mut future));
    if !result.is_pending() {
        panic!("unexpected result, expected it to return Poll::Pending");
    }
    drop(future);
}
