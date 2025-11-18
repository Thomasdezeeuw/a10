#![cfg_attr(feature = "nightly", feature(async_iterator))]

mod util;

#[path = "functional/config.rs"]
mod config;
#[path = "functional/fd.rs"]
mod fd;
#[path = "functional/fs.rs"]
mod fs;
#[path = "functional/io.rs"]
mod io;
#[path = "functional/mem.rs"]
mod mem;
#[path = "functional/msg.rs"]
mod msg;
#[path = "functional/net.rs"]
mod net;
#[path = "functional/poll.rs"]
mod poll;
#[path = "functional/process.rs"]
mod process;
#[path = "functional/read_buf.rs"]
mod read_buf;
#[path = "functional/ring.rs"]
mod ring;
