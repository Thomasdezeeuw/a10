use std::thread;
use std::time::Duration;

use a10::io::ReadBufPool;
use a10::{Config, Ring};

use crate::util::{expect_io_errno, init, is_send, is_sync, require_kernel};

const BUF_SIZE: usize = 4096;

#[test]
fn config_is_send_and_sync() {
    is_send::<Config>();
    is_sync::<Config>();
}

#[test]
fn config_disabled() {
    require_kernel!(5, 10);
    init();
    let mut ring = Ring::config(1).disable().build().unwrap();

    // In a disabled state, so we expect an error.
    expect_io_errno(ring.poll(None), libc::EBADFD);

    // Enabling it should allow us to poll.
    ring.enable().unwrap();
    ring.poll(Some(Duration::from_millis(1))).unwrap();
}

#[test]
fn config_single_issuer() {
    require_kernel!(6, 0);
    init();

    let ring = Ring::config(1).single_issuer().build().unwrap();

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
fn config_single_issuer_disabled_ring() {
    require_kernel!(6, 0);
    init();

    let mut ring = Ring::config(1).single_issuer().disable().build().unwrap();

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
fn config_defer_task_run() {
    require_kernel!(6, 1);
    init();

    let mut ring = Ring::config(1)
        .single_issuer()
        .defer_task_run()
        .with_kernel_thread(false)
        .build()
        .unwrap();

    ring.poll(Some(Duration::ZERO)).unwrap();
}
