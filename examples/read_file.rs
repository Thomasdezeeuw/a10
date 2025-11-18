//! Read a single file.
//!
//! Run with:
//! $ cargo run --example read_file -- examples/read_file.rs

use std::{env, io, str};

use a10::fs::OpenOptions;
use a10::{Ring, SubmissionQueue};

mod runtime;

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = Ring::new(1)?;

    // Path of the file to read.
    let path = env::args()
        .nth(1)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing argument"))?;

    // Create our future that reads the file.
    let read_file_future = read_file(ring.sq().clone(), path);

    // Use our fake runtime to poll the future.
    let data = runtime::block_on(&mut ring, read_file_future)?;

    // We'll print the response (using ol' fashioned blocking I/O).
    let data = str::from_utf8(&data).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file doesn't contain UTF-8: {err}"),
        )
    })?;
    println!("{data}");

    Ok(())
}

async fn read_file(sq: SubmissionQueue, path: String) -> io::Result<Vec<u8>> {
    // Open a file for reading.
    let file = OpenOptions::new().open(sq, path.into()).await?;

    // Read some bytes from the file.
    let buf = file.read(Vec::with_capacity(32 * 1024)).await?;
    Ok(buf)
}
