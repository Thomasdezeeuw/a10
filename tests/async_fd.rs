//! Tests for [`a10::AsyncFd`].

#![cfg_attr(feature = "nightly", feature(async_iterator))]

use a10::fd::{AsyncFd, File};

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
fn async_fd_is_send_and_sync() {
    is_send::<AsyncFd<File>>();
    is_sync::<AsyncFd<File>>();
}

#[test]
#[cfg(any(target_os = "linux"))]
fn async_direct_fd_is_send_and_sync() {
    use a10::fd::Direct;
    is_send::<AsyncFd<Direct>>();
    is_sync::<AsyncFd<Direct>>();
}
