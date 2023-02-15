//! Tests for [`a10::AsyncFd`].

#![feature(async_iterator, once_cell)]

mod util;

#[path = "async_fd"] // rustfmt can't find the files.
mod async_fd {
    mod fs;
    mod io;
    mod net;
}
