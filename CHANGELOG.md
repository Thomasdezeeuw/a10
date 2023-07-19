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
