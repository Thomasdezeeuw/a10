use std::env::args;
use std::io;

mod runtime;

fn main() -> io::Result<()> {
    std_logger::Config::logfmt().init();
    let mut ring = a10::Ring::new(1)?;
    let sq = ring.submission_queue().clone();
    let mut filenames: Vec<String> = args().skip(1).collect();
    runtime::block_on(&mut ring, test(sq, filenames))
}

async fn test(sq: a10::SubmissionQueue, filenames: Vec<String>) -> io::Result<()> {
    let mut buf = Vec::with_capacity(64); //(8 * 1024);
    let mut stdout = std::io::stdout();
    for filename in filenames {
        let file = std::fs::File::open(&filename)?;
        let fd = a10::AsyncFd::new(file.into(), sq.clone());

        loop {
            buf.clear();
            buf = fd.read(buf).await?;
            if buf.is_empty() {
                // Read the entire file.
                break;
            }

            let mut n = 0;
            loop {
                use std::io::Write;
                let m = stdout.write(&buf[n..])?;
                n += m;
                if n == buf.len() {
                    // Written all the bytes, try reading again.
                    break;
                }
            }
        }
    }
    Ok(())
}
