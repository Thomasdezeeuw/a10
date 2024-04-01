# v0.1.8

* Make BufGroupId and BufIdx public, but still hide them from the docs
  <https://github.com/Thomasdezeeuw/a10/commit/c22a6913e37859358b2730d3a92e9e8d6801fa44>.

# v0.1.7

* Update the log crate to use the (now stable) `kv` feature.
  <https://github.com/Thomasdezeeuw/a10/commit/a51723d0491dd129f28604dd0995cb8d0a21fa80>.
* Added `AsyncFd::socket_option`
  <https://github.com/Thomasdezeeuw/a10/commit/2046b60875c273da9d9832cc1a28d118cbf84413>.
* Added `AsyncFd::set_socket_option`
  <https://github.com/Thomasdezeeuw/a10/commit/1e651a24533ca21b7caa4cfa62722639a3301afd>.
* Added `process:wait(_on)`
  <https://github.com/Thomasdezeeuw/a10/commit/45453bfb5e6a7f44d836c30923be0753d70ccc8e>.
* Fix memory leak in ReceiveSignals
  <https://github.com/Thomasdezeeuw/a10/commit/b1c28a95be538b0e35ecfc2cd6924cc706d0975b>.
* Don't create completion event when dropping `AsyncFd`
  <https://github.com/Thomasdezeeuw/a10/commit/b7e73aee7f79225894ffce32731f2107cea4ca0c>.
* Only create one completion event when waking
  <https://github.com/Thomasdezeeuw/a10/commit/f752da0c668bb347b9c65981b3ee7acb06264d7b>.
* Don't request for completion event when canceling an operation
  <https://github.com/Thomasdezeeuw/a10/commit/543d490603a3a61265ac88856f7f976aaed4f970>.
* Don't request for completion event when sending messages
  <https://github.com/Thomasdezeeuw/a10/commit/f6030ea95319e1d158b4f595a05743d66d388984>.
* Cancel all operations it's dropped without completing it
  <https://github.com/Thomasdezeeuw/a10/commit/6298b3aa2056cf751b3ba253fb1e1d0f2599180e>,
  <https://github.com/Thomasdezeeuw/a10/commit/b6c2ed57b763192bd7f359549f4a86ca5f320b01>,
  <https://github.com/Thomasdezeeuw/a10/commit/6b823db80e463529e4f527fca93be14acf437eb7>,
  <https://github.com/Thomasdezeeuw/a10/commit/f7dded7d350e30792d21bf894de018375b14c1f4>,
  <https://github.com/Thomasdezeeuw/a10/commit/cde7c070d112eaa97d4274de67affde077f9bc5d>,
  <https://github.com/Thomasdezeeuw/a10/commit/90f67be0c6559751f1ce1fe5ae0a3121ba2444c1>,
  <https://github.com/Thomasdezeeuw/a10/commit/a54b50c9fd1741d2701f9f69e0b5df5297da2e0b>,
  <https://github.com/Thomasdezeeuw/a10/commit/5efd5c2afb1bd038e23179c0dade054ea359be4e>,
  <https://github.com/Thomasdezeeuw/a10/commit/ba2349f877b306210ce776dc257bab4ab2759af1>.
* Loosen result check in `CancelOp`
  <https://github.com/Thomasdezeeuw/a10/commit/86695d53a3f387b648419ae20b21f4751d52a0c0>.
* Make slot available quicker for stopped multishot operations
  <https://github.com/Thomasdezeeuw/a10/commit/51ad73b8efb43279c07023c2f3f87bce3e27a228>.

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
