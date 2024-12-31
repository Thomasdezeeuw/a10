//! Poll for file descriptor events.
//!
//! To wait for events on a file descriptor use:
//!  * [`oneshot_poll`] a [`Future`] returning a single [`PollEvent`].
//!  * [`multishot_poll`] an [`AsyncIterator`] returning multiple
//!    [`PollEvent`]s.
//!
//! Note that module only supports regular file descriptors, not direct
//! descriptors as it doesn't make much sense to poll a direct descriptor,
//! instead start the I/O operation you want to perform.
//!
//! [`AsyncIterator`]: std::async_iter::AsyncIterator
