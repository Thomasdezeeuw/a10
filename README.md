# A10

The [A10] io\_uring library.

This library is meant as a low-level library safely exposing the io\_uring API.
For simplicity this only has two main types and a number of helper types:
 * `Ring` is a wrapper around io\_uring used to poll for completion events.
 * `AsyncFd` is a wrapper around a file descriptor that provides a safe API to
   schedule operations.

[A10]: https://en.wikipedia.org/wiki/A10_motorway_(Netherlands)

## Linux Required

Currently this requires a fairly new Linux kernel version, everything should
work on Linux v6.1 and up.

## Examples

A10 is expected to be integrated into a `Future` runtime, but it can work as a
stand-alone library.

```rust
use std::path::PathBuf;
use std::future::Future;
use std::io;

use a10::{Extract, Ring, SubmissionQueue};

fn main() -> io::Result<()> {
    // Create a new I/O uring supporting 8 submission entries.
    let mut ring = Ring::new(8)?;

    // Get access to the submission queue, used to... well queue submissions.
    let sq = ring.submission_queue().clone();
    // A10 makes use of `Future`s to represent the asynchronous nature of
    // io_uring.
    let future = cat(sq, "./src/lib.rs");

    // This `block_on` function would normally be implement by a `Future`
    // runtime, but we show a simple example implementation below.
    block_on(&mut ring, future)
}

/// A "cat" like function, which reads from `filename` and writes it to
/// standard out.
async fn cat(sq: SubmissionQueue, filename: &str) -> io::Result<()> {
    // Because io_uring uses asychronous operation it needs access to the
    // path for the duration the operation is active. To prevent use-after
    // free and similar issues we need ownership of the arguments. In the
    // case of opening a file it means we need ownership of the file name.
    let filename = PathBuf::from(filename);
    // Open a file for reading.
    let file = a10::fs::OpenOptions::new().open(sq.clone(), filename).await?;

    // Next we'll read from the from the file.
    // Here we need ownership of the buffer, same reason as discussed above.
    let buf = file.read(Vec::with_capacity(32 * 1024)).await?;

    // Let's write what we read from the file to standard out.
    let stdout = a10::io::stdout(sq);
    // For writing we also need ownership of the buffer, so we move the
    // buffer into function call. However by default we won't get it back,
    // to match the API you see in the standard libray.
    // But using buffers just once it a bit wasteful, so we can it back
    // using the `Extract` trait (the call to `extract`). It changes the
    // return values (and `Future` type) to return the buffer and the amount
    // of bytes written.
    let (buf, n) = stdout.write(buf).extract().await?;

    // All done.
    Ok(())
}

/// Block on the `future`, expecting polling `ring` to drive it forward.
fn block_on<Fut, T>(ring: &mut Ring, future: Fut) -> Fut::Output
where
    Fut: Future<Output = io::Result<T>>
{
    use std::task::{self, RawWaker, RawWakerVTable, Poll};
    use std::ptr;

    // Pin the future to the stack so we don't move it around.
    let mut future = std::pin::pin!(future);

    // Create a task context to poll the future work.
    let waker = unsafe { task::Waker::from_raw(RawWaker::new(ptr::null(), &WAKER_VTABLE)) };
    let mut ctx = task::Context::from_waker(&waker);

    loop {
        match future.as_mut().poll(&mut ctx) {
            Poll::Ready(result) => return result,
            Poll::Pending => {
                // Poll the `Ring` to get an update on the operation(s).
                //
                // In pratice you would first yield to another future, but
                // in this example we don't have one, so we'll always poll
                // the `Ring`.
                ring.poll(None)?;
            }
        }
    }

    // A waker implementation that does nothing.
    static WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
        |_| RawWaker::new(ptr::null(), &WAKER_VTABLE),
        |_| {},
        |_| {},
        |_| {},
    );
}
```

For more examples see the [examples directory].

[examples directory]: ./examples
