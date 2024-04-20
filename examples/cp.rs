//! cp - copy files.
//!
//! Only a single file copy is supported.

use std::env::args;
use std::io;

use a10::fs::OpenOptions;
use a10::io::ReadBufPool;
use a10::{AsyncFd, Extract, SubmissionQueue};

mod runtime;

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = a10::Ring::new(1)?;
    // Get our copy of the submission queue.
    let sq = ring.submission_queue().clone();

    // Collect the files we want to concatenate.
    let mut args = args().skip(1);
    let source = match args.next() {
        Some(source) => source,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing source",
            ))
        }
    };
    let destination = match args.next() {
        Some(destination) => destination,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing destination",
            ))
        }
    };

    // Run our cat program.
    runtime::block_on(&mut ring, cp(sq, source, destination))
}

async fn cp(sq: SubmissionQueue, source: String, destination: String) -> io::Result<()> {
    let input: AsyncFd = OpenOptions::new().open(sq.clone(), source.into()).await?;
    let output: AsyncFd = OpenOptions::new()
        .write()
        .create_new()
        .open(sq.clone(), destination.into())
        .await?;

    // Read and write 8 pages at a time.
    let buf_pool = ReadBufPool::new(sq.clone(), 1, 8 * 4096)?;
    let mut buf = buf_pool.get();
    let mut n;
    loop {
        buf.release();
        buf = input.read(buf).await?;
        if buf.is_empty() {
            // Read the entire file.
            return Ok(());
        }

        loop {
            (buf, n) = output.write(buf).extract().await?;
            if n == buf.len() {
                // Written all the bytes, try reading again.
                break;
            } else {
                // Remove the bytes we've already written and try again.
                buf.remove(..n);
            }
        }
    }
}
