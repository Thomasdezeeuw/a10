use std::io;
use std::time::{Duration, Instant};

use a10::Ring;

#[test]
fn polling_timeout() -> io::Result<()> {
    const TIMEOUT: Duration = Duration::from_millis(400);
    const MARGIN: Duration = Duration::from_millis(10);

    let mut ring = Ring::new(1)?;

    let start = Instant::now();
    ring.poll(Some(TIMEOUT))?;
    let elapsed = start.elapsed();
    assert!(
        elapsed >= TIMEOUT && elapsed <= (TIMEOUT + MARGIN),
        "polling elapsed: {:?}, expected: {:?}",
        elapsed,
        TIMEOUT
    );
    Ok(())
}
