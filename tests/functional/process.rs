//! NOTE: see `tests/signals.rs` for testing of signal handling.

use std::pin::pin;
use std::process::Command;

#[cfg(any(target_os = "android", target_os = "linux"))]
use a10::process::ToDirect;
use a10::process::{
    self, ChildStatus, ReceiveSignal, Signal, SignalInfo, SignalSet, Signals, To, WaitId,
    WaitOption,
};

use crate::util::{Waker, ensure_submitted, is_send, is_sync, poll_nop, start_op, test_queue};

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
#[cfg(any(target_os = "android", target_os = "linux"))]
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
    let sq = test_queue();
    let waker = Waker::new();

    let process = Command::new("true").spawn().unwrap();
    let pid = process.id();

    let info = waker
        .block_on(process::wait_on(sq, &process).flags(WaitOption::EXITED))
        .expect("failed wait");

    assert_eq!(info.signal(), Signal::CHILD);
    assert_eq!(info.code(), ChildStatus::EXITED);
    assert_eq!(info.pid(), pid as i32);
    assert_eq!(info.status().code(), Some(libc::EXIT_SUCCESS));
}

#[test]
fn process_wait_on_process_that_does_not_stop() {
    let sq = test_queue();

    let process = Command::new("sleep").arg("10000").spawn().unwrap();

    let mut fut = pin!(process::wait_on(sq, &process).flags(WaitOption::EXITED));
    for _ in 0..=3 {
        let res = poll_nop(fut.as_mut());
        assert!(res.is_pending(), "unexpected poll result: {res:?}");
    }
}

#[test]
fn process_wait_on_drop_before_complete() {
    let sq = test_queue();

    let process = Command::new("sleep").arg("1000").spawn().unwrap();

    let mut future = process::wait_on(sq, &process).flags(WaitOption::EXITED);
    start_op(&mut future);
    drop(future);

    // Ensure the cancelation (on drop) is completed.
    ensure_submitted();
}

#[test]
fn send_signal() {
    let sq = test_queue();
    let waker = Waker::new();

    let process = Command::new("/bin/sleep").arg("10000").spawn().unwrap();
    let pid = process.id();

    let mut wait_on = pin!(process::wait_on(sq, &process).flags(WaitOption::EXITED));
    start_op(&mut *wait_on);

    process::send_signal(To::child(&process), Signal::TERMINATION).unwrap();

    let info = waker.block_on(wait_on).expect("failed wait");

    assert_eq!(info.signal(), Signal::CHILD);
    assert_eq!(info.code(), ChildStatus::KILLED);
    assert_eq!(info.pid(), pid as i32);
}
