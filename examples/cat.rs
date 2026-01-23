//! cat - concatenate and print files.
//!
//! Run with:
//! $ cargo run --example cat -- src/lib.rs src/fd.rs

use std::env::args;
use std::io;

use a10::fs::OpenOptions;
use a10::io::ReadBufPool;
use a10::{Extract, SubmissionQueue};

mod runtime;

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = a10::Ring::new()?;
    // Get an owned reference to the submission queue.
    let sq = ring.sq();

    // Collect the files we want to concatenate.
    let mut filenames: Vec<String> = args().skip(1).collect();
    // Default to reading from standard in.
    if filenames.is_empty() {
        filenames.push(String::from("-"));
    }

    // Run our cat program.
    runtime::block_on(&mut ring, cat(sq, filenames))
}

async fn cat(sq: SubmissionQueue, filenames: Vec<String>) -> io::Result<()> {
    let stdout = a10::io::stdout(sq.clone());
    // Read and write 8 pages at a time.
    let buf_pool = ReadBufPool::new(sq.clone(), 1, 8 * 4096)?;
    let mut buf = buf_pool.get();
    let mut n;
    for filename in filenames {
        let stdin;
        let file;
        let file = if filename == "-" {
            stdin = a10::io::stdin(sq.clone());
            &*stdin
        } else {
            file = OpenOptions::new().open(sq.clone(), filename.into()).await?;
            &file
        };

        loop {
            buf.release();
            buf = file.read(buf).await?;
            if buf.is_empty() {
                // Read the entire file.
                break;
            }

            loop {
                (buf, n) = stdout.write(buf).extract().await?;
                if n == buf.len() {
                    // Written all the bytes, try reading again.
                    break;
                }
                // Remove the bytes we've already written and try again.
                buf.remove(..n);
            }
        }
    }
    Ok(())
}
