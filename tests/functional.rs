#![cfg_attr(feature = "nightly", feature(async_iterator))]
// Needed for process::process_wait_on_fd test.
#![feature(linux_pidfd)]

mod util;

#[path = "functional/config.rs"]
mod config;
#[path = "functional/fd.rs"]
mod fd;
#[path = "functional/fs.rs"]
mod fs;
#[path = "functional/fs_notify.rs"]
mod fs_notify;
#[path = "functional/io.rs"]
mod io;
#[path = "functional/mem.rs"]
mod mem;
#[path = "functional/net.rs"]
mod net;
#[path = "functional/net_options.rs"]
mod net_options;
#[path = "functional/pipe.rs"]
mod pipe;
#[path = "functional/process.rs"]
mod process;
#[path = "functional/read_buf.rs"]
mod read_buf;
#[path = "functional/ring.rs"]
mod ring;
