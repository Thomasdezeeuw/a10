use std::io;
use std::time::{Duration, Instant};

use a10::Ring;

#[test]
fn polling_timeout() -> io::Result<()> {
    const TIMEOUT: Duration = Duration::from_millis(400);
    const MARGIN: Duration = Duration::from_millis(10);

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
    let _ = std_logger::Config::logfmt()
        .with_call_location(true)
        .try_init();

    let ring = Ring::new(64).unwrap();
    drop(ring);
}
