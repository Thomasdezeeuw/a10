#![feature(once_cell)]

use std::pin::Pin;
use std::task::Poll;
use std::time::{Duration, Instant};
use std::{io, thread};

use a10::fs::OpenOptions;
use a10::{Ring, Waker};

mod util;
use util::{init, poll_nop};

#[test]
fn polling_timeout() -> io::Result<()> {
    const TIMEOUT: Duration = Duration::from_millis(400);
    const MARGIN: Duration = Duration::from_millis(10);

    init();
    let mut ring = Ring::new(1).unwrap();

    let start = Instant::now();
    ring.poll(Some(TIMEOUT)).unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed >= TIMEOUT && elapsed <= (TIMEOUT + MARGIN),
        "polling elapsed: {elapsed:?}, expected: {TIMEOUT:?}",
    );
    Ok(())
}

#[test]
fn dropping_unmaps_queues() {
    init();
    let ring = Ring::new(64).unwrap();
    drop(ring);
}

#[test]
fn submission_queue_full_is_handle_internally() {
    const PATH: &str = "tests/data/lorem_ipsum_50.txt";
    const EXPECTED: &[u8] = include_bytes!("data/lorem_ipsum_50.txt");
    const SIZE: usize = 31396;
    const N: usize = (usize::BITS as usize) + 10;
    const BUF_SIZE: usize = SIZE / N;

    init();
    let mut ring = Ring::new(2).unwrap();
    let sq = ring.submission_queue();

    let mut future = OpenOptions::new().open(sq.clone(), PATH.into());
    let file = loop {
        match poll_nop(Pin::new(&mut future)) {
            Poll::Ready(result) => break result.unwrap(),
            Poll::Pending => ring.poll(None).unwrap(),
        }
    };

    let mut futures = (0..N)
        .into_iter()
        .map(|i| Some(file.read_at(Vec::with_capacity(BUF_SIZE), (i * BUF_SIZE) as u64)))
        .collect::<Vec<_>>();
    loop {
        let mut needs_poll = false;
        for (i, fut) in futures.iter_mut().enumerate() {
            if let Some(future) = fut {
                match poll_nop(Pin::new(future)) {
                    Poll::Ready(result) => {
                        *fut = None;
                        let read_buf = result.unwrap();
                        assert_eq!(read_buf, &EXPECTED[i * BUF_SIZE..(i + 1) * BUF_SIZE]);
                    }
                    Poll::Pending => needs_poll = true,
                }
            }
        }
        if needs_poll {
            ring.poll(None).unwrap();
        } else {
            break;
        }
    }

    assert!(futures.iter().all(Option::is_none));
}

#[test]
fn waker_wake() {
    init();
    let mut ring = Ring::new(2).unwrap();

    let waker = Waker::new(&ring);
    let handle = thread::spawn(move || {
        // NOTE: this sleep ensures that the "submission queue polling kernel
        // thread" (the one that reads our submissions) goes to sleep, this way
        // we can test that we wake that as well.
        thread::sleep(Duration::from_secs(1));
        waker.wake();
    });

    // Should be awoken by the wake call above.
    ring.poll(None).unwrap();
    handle.join().unwrap();
}

#[test]
fn waker_wake_before_poll_nop() {
    init();
    let mut ring = Ring::new(2).unwrap();

    Waker::new(&ring).wake();

    // Should be awoken by the wake call above.
    ring.poll(None).unwrap();
}

#[test]
fn waker_wake_after_ring_dropped() {
    init();
    let ring = Ring::new(2).unwrap();

    let waker = Waker::new(&ring);

    drop(ring);
    waker.wake();
}
