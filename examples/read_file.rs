use std::future::{Future, IntoFuture};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{env, io, str};

use a10::fs::OpenOptions;
use a10::Ring;

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = Ring::new(1)?;

    // Asynchronously open a file for reading.
    let path = env::args().nth(1).expect("missing argument");
    let open_file = OpenOptions::new().open(ring.submission_queue(), path.into())?;

    // Poll the ring and check if the file is opened.
    ring.poll(None)?;
    let file = block_on(open_file)?;

    // Start aysynchronously reading from the file.
    let buf = Vec::with_capacity(4096);
    let read = file.read(buf)?;

    // Same pattern as above; poll and check the result.
    ring.poll(None)?;
    let buf = block_on(read)?;

    // Done reading, we'll print the result (using ol' fashioned blocking I/O).
    let data = str::from_utf8(&buf).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file doesn't contain UTF-8: {}", err),
        )
    })?;
    println!("{}", data);

    Ok(())
}

/// Replace this with your favorite [`Future`] runtime.
fn block_on<Fut>(fut: Fut) -> Fut::Output
where
    Fut: IntoFuture,
    Fut::IntoFuture: Unpin,
{
    let waker = noop_waker();
    let mut ctx = task::Context::from_waker(&waker);
    let mut fut = fut.into_future();
    let mut fut = Pin::new(&mut fut);
    loop {
        if let Poll::Ready(result) = fut.as_mut().poll(&mut ctx) {
            return result;
        }
    }
}

fn noop_waker() -> task::Waker {
    use std::ptr;
    use std::task::{RawWaker, RawWakerVTable};
    static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(ptr::null(), &WAKER_VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    unsafe { task::Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE)) }
}
