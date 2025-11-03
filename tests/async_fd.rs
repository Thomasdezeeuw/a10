//! Tests for [`a10::AsyncFd`].

#![cfg_attr(feature = "nightly", feature(async_iterator))]

use a10::AsyncFd;

mod util;
use util::{is_send, is_sync};

#[path = "async_fd"] // rustfmt can't find the files.
mod async_fd {
    mod direct;
    mod fs;
    mod io;
    mod net;
}

#[test]
fn async_fd_size() {
    assert_eq!(std::mem::size_of::<AsyncFd>(), 16);
    assert_eq!(std::mem::size_of::<Option<AsyncFd>>(), 16);
}

#[test]
fn async_fd_is_send_and_sync() {
    is_send::<AsyncFd>();
    is_sync::<AsyncFd>();
}
