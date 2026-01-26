//! fs_notify - watch for file system changes.
//!
//! Run with:
//! $ cargo run --example fs_notify -- -r examples/ src/
//! $ touch src/lib.rs examples/fs_notify.rs

use std::env::args;
use std::future::poll_fn;
use std::io;
use std::path::PathBuf;
use std::pin::pin;

use a10::fs;
use a10::fs::notify::{Interest, Recursive};

mod runtime;

fn main() -> io::Result<()> {
    // Create a new I/O uring.
    let mut ring = a10::Ring::new()?;
    // Get an owned reference to the submission queue.
    let sq = ring.sq();

    // Create a new file system watcher.
    let mut watcher = fs::notify::Watcher::new(sq)?;

    // Add all the files we want to watch.
    let mut recursive = Recursive::No;
    for arg in args().skip(1) {
        if arg == "-r" || arg == "--recursive" {
            recursive = Recursive::All;
            continue;
        }
        watcher.watch(PathBuf::from(arg), Interest::ALL, recursive)?;
    }

    // Run our watch program.
    runtime::block_on(&mut ring, watch(watcher))
}

async fn watch(mut watcher: fs::notify::Watcher) -> io::Result<()> {
    let mut events = pin!(watcher.events());
    // Poll for file system events (the ergonomics for this  should be improved
    // once the `AsyncIterator` trait is stabilised).
    while let Some(result) = poll_fn(|ctx| events.as_mut().poll_next(ctx)).await {
        let event = result?;
        let path = events.path_for(&event);
        println!(
            "Got a file system event for '{}': {event:?}",
            path.display()
        );
    }
    Ok(())
}
