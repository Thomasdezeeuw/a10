//! Tests for [`a10::Ring`].

#![cfg_attr(feature = "nightly", feature(async_iterator))]

use std::pin::Pin;
use std::process::Command;

use a10::process::{self, ChildStatus, Signal, WaitOption};

mod util;
use util::{Waker, cancel, is_send, is_sync, poll_nop, require_kernel, test_queue};

#[test]
fn process_wait_on() {
    require_kernel!(6, 7);
    let sq = test_queue();
    let waker = Waker::new();

    is_send::<process::WaitId>();
    is_sync::<process::WaitId>();

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
fn process_wait_on_cancel() {
    require_kernel!(6, 7);
    let sq = test_queue();
    let waker = Waker::new();

    let mut process = Command::new("sleep").arg("1000").spawn().unwrap();

    let mut future = process::wait_on(sq, &process, Some(WaitOption::EXITED));

    cancel(&waker, &mut future, |future| {
        // NOTE: can't use `start_op` as `siginfo_t` doesn't implemented
        // `fmt::Debug`.
        let result = poll_nop(Pin::new(future));
        if !result.is_pending() {
            panic!("unexpected result, expected it to return Poll::Pending");
        }
    });

    process.kill().unwrap();
    process.wait().unwrap();
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
