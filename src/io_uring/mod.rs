//! io_uring implementation.

pub(crate) mod sys;

pub(crate) use sys as libc; // TODO: replace this with definitions from the `libc` crate once available.
