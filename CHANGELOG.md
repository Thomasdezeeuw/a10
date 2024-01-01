# v0.1.6

* Added `AsyncFd::try_clone`
  <https://github.com/Thomasdezeeuw/a10/commit/2fc6e63dbdaf8556ca9565ee14e5ab8bf20884a3>.
* Fixs size of tuple SocketAddress implementations
  <https://github.com/Thomasdezeeuw/a10/commit/11781458e176e093852ee7241dde2e2aba033f18>.
* Updates io\_uring definitions, syncing to liburing commit [`7524a6a`](https://github.com/axboe/liburing/commit/7524a6adf4d6720a47bfa617b5cb2fd8d57f16d2)
  <https://github.com/Thomasdezeeuw/a10/commit/2660ccc6ab0dc538a561fcba255d53b0d553ce87>,
  <https://github.com/Thomasdezeeuw/a10/commit/32967b974b859c8bca84feb24950d3dfc0215c46>.

# v0.1.5

* Reduce flag unsupported logs to debug
  <https://github.com/Thomasdezeeuw/a10/commit/061236a9023486b0b02302fc4163acec29b98ac4>.

# v0.1.4

* Fixed dropping of `ReceiveSignals`, it now properly cancels the receiving of
  process signals and ensure the kernel doesn't write into deallocated memory
  <https://github.com/Thomasdezeeuw/a10/pull/81>.

# v0.1.3

* Added `ReceiveSignals`, a type that combines Signals and signals::Receive to
  not have to deal with lifetime of the fd
  <https://github.com/Thomasdezeeuw/a10/pull/79>.

# v0.1.2

* Added support for user space messaging, see `SubmissionQueue::msg_listener`
  and `SubmissionQueue::(try_)send_msg`
  <https://github.com/Thomasdezeeuw/a10/pull/76>.
* Returns more accurate `io::ErrorKind`s for certain errors when `nightly`
  feature is enabled
  <https://github.com/Thomasdezeeuw/a10/pull/77>.

# v0.1.1

* Don't leak `SubmissionQueue` in `Std{in,out,err}` types
  <https://github.com/Thomasdezeeuw/a10/pull/72>.
* Implement fmt::Debug for `Std{in,out,err}` and improve the implemtation for
  `AsyncFd` and `SubmissionQueue`
  <https://github.com/Thomasdezeeuw/a10/pull/73>.

# v0.1.0

Initial release.
