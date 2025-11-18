#![cfg_attr(feature = "nightly", feature(async_iterator))]

mod util;

#[path = "functional/config.rs"]
mod config;
#[path = "functional/mem.rs"]
mod mem;
#[path = "functional/msg.rs"]
mod msg;
#[path = "functional/poll.rs"]
mod poll;
#[path = "functional/process.rs"]
mod process;
#[path = "functional/ring.rs"]
mod ring;
