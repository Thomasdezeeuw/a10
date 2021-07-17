use std::task::Poll;
use std::{env, io, str};

use a10::fs::File;
use a10::Ring;

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = Ring::new(1)?;

    // Asynchronously open a file for reading.
    let path = env::args().nth(1).expect("missing argument");
    let mut open_file = File::open(&ring, path.into())?;

    // Poll the ring and check if the file is opened.
    ring.poll()?;
    let file = match open_file.check() {
        Poll::Ready(Ok(file)) => file,
        Poll::Ready(Err(err)) => return Err(err),
        Poll::Pending => todo!(),
    };

    // Start aysynchronously reading from the file.
    let buf = Vec::with_capacity(4096);
    let mut read = file.read(buf)?;

    // Same pattern as above; poll and check the result.
    ring.poll()?;
    let buf = match read.check() {
        Poll::Ready(Ok(file)) => file,
        Poll::Ready(Err(err)) => return Err(err),
        Poll::Pending => todo!(),
    };

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
