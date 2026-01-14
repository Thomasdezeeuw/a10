#[cfg(any(target_os = "android", target_os = "linux"))]
use std::thread;
#[cfg(any(target_os = "android", target_os = "linux"))]
use std::time::Duration;

use a10::Config;
#[cfg(any(target_os = "android", target_os = "linux"))]
use a10::Ring;
#[cfg(any(target_os = "android", target_os = "linux"))]
use a10::io::ReadBufPool;

#[cfg(any(target_os = "android", target_os = "linux"))]
use crate::util::{expect_io_errno, init};
use crate::util::{is_send, is_sync};

#[cfg(any(target_os = "android", target_os = "linux"))]
const BUF_SIZE: usize = 4096;

#[test]
fn config_is_send_and_sync() {
    is_send::<Config>();
    is_sync::<Config>();
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn config_disabled() {
    init();
    let mut ring = Ring::config().disable().build().unwrap();

    // In a disabled state, so we expect an error.
    expect_io_errno(ring.poll(None), libc::EBADFD);

    // Enabling it should allow us to poll.
    ring.enable().unwrap();
    ring.poll(Some(Duration::from_millis(1))).unwrap();
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn config_single_issuer() {
    init();

    let ring = Ring::config().single_issuer().build().unwrap();

    // This is fine.
    let buf_pool = ReadBufPool::new(ring.sq().clone(), 2, BUF_SIZE as u32).unwrap();
    drop(buf_pool);

    thread::spawn(move || {
        // This is not (we're on a different thread).
        let res = ReadBufPool::new(ring.sq().clone(), 2, BUF_SIZE as u32);
        expect_io_errno(res, libc::EEXIST);
    })
    .join()
    .unwrap();
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn config_single_issuer_disabled_ring() {
    init();

    let mut ring = Ring::config().single_issuer().disable().build().unwrap();

    thread::spawn(move || {
        ring.enable().unwrap();

        // Since this thread enabled the ring, this is now fine.
        let buf_pool = ReadBufPool::new(ring.sq().clone(), 2, BUF_SIZE as u32).unwrap();
        drop(buf_pool);
    })
    .join()
    .unwrap();
}

#[test]
#[cfg(any(target_os = "android", target_os = "linux"))]
fn config_defer_task_run() {
    init();

    let mut ring = Ring::config()
        .single_issuer()
        .defer_task_run()
        .build()
        .unwrap();

    ring.poll(Some(Duration::ZERO)).unwrap();
}
