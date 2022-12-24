use std::future::Future;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{env, io, ptr, str};

use a10::fs::OpenOptions;
use a10::{Ring, SubmissionQueue};

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = Ring::new(1)?;

    let path = env::args().nth(1).expect("missing argument");

    // Create our future that reads the file.
    let mut request = Box::pin(read_file(ring.submission_queue(), path));

    // This loop will represent our `Future`s runtime, in practice use an actual
    // implementation.
    let data = loop {
        match poll_future(request.as_mut()) {
            Poll::Ready(res) => break res?,
            Poll::Pending => {
                // Poll the `Ring` to get an update on the operation (s).
                //
                // In pratice you would first yield to another future, but in
                // this example we don't have one, so we'll always poll the
                // `Ring`.
                ring.poll(None)?;
            }
        }
    };

    // We'll print the response (using ol' fashioned blocking I/O).
    let data = str::from_utf8(&data).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file doesn't contain UTF-8: {}", err),
        )
    })?;
    println!("{data}");

    Ok(())
}

async fn read_file(sq: SubmissionQueue, path: String) -> io::Result<Vec<u8>> {
    // Open a file for reading.
    let file = OpenOptions::new().open(sq, path.into())?.await?;

    // Read some bytes from the file.
    let buf = file.read(Vec::with_capacity(4096))?.await?;
    Ok(buf)
}

/// Replace this with your favorite [`Future`] runtime.
fn poll_future<Fut>(fut: Pin<&mut Fut>) -> Poll<Fut::Output>
where
    Fut: Future,
{
    use std::task::{RawWaker, RawWakerVTable};
    static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(ptr::null(), &WAKER_VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
    let waker = unsafe { task::Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE)) };
    let mut ctx = task::Context::from_waker(&waker);
    fut.poll(&mut ctx)
}
