//! Tests for [`a10::AsyncFd`].

#![cfg_attr(feature = "nightly", feature(async_iterator))]

use a10::fd::{AsyncFd, Direct, File};

mod util;
use util::{is_send, is_sync};

#[path = "async_fd"] // rustfmt can't find the files.
mod async_fd {
    mod fixed;
    mod fs;
    mod io;
    mod net;
}

#[test]
fn is_send_and_sync() {
    is_send::<AsyncFd<File>>();
    is_sync::<AsyncFd<File>>();
    is_send::<AsyncFd<Direct>>();
    is_sync::<AsyncFd<Direct>>();
}
