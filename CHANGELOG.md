# v0.3.0 (unreleased)

* Complete rewrite of the internals in an attempt to make it easier to port.
* The methods on `Config` are now implementation specific.
* Removed the `AsFd` implementation for `Ring` as that might not always be
  possible to provide.
* The `net::SocketAddress` is now implemented for the socket address types for
  in the standard library, not in libc.
* `net::Connect` no longer implements `Extract` as all socket address types in
  the library are `Copy`.
* The `BufSlice` and `BufMutSlice` types now use `IoSlice` and `IoMutSlice` as
  wrapper around `libc::iovec`.
* The `msg` module now uses the `MsgData` type as type for message data, instead
  of `u32` (though `MsgData` is also `u32`).
* `process::ReceiveSignals` is now a proper `AsyncIter`.

# v0.2.2

* Fix possible overflow in ReadBuf::release
  <https://github.com/Thomasdezeeuw/a10/commit/79078cb9a2f4222eb184588ab1b05c37ad5f5507>.
* Don't close direct descriptor using close(2)
  <https://github.com/Thomasdezeeuw/a10/commit/a51984404c00beb6cecf4419842860fe41d23154>.

# v0.2.1

* Added `AsyncFd::truncate`
  <https://github.com/Thomasdezeeuw/a10/commit/6cd74479264cbd230e47b9d2a572a75aab4d83b1>.
* Synchronously close fd if it can't be done asynchronously due to a full queue
  <https://github.com/Thomasdezeeuw/a10/commit/42565a387be884921e5300cf3fc4f833e178906d>.
* Exports `Cancel` at the root of the crate
  <https://github.com/Thomasdezeeuw/a10/commit/4b91d3f1fcd8502076150bfb12853e9f08b1bdc4>.
* Delay allocation of drop waker when possible. This is used when a `Future` is
  dropped before it's operation is complete.
  <https://github.com/Thomasdezeeuw/a10/commit/629c25e3883d2eedffe77eda4937cc50842f4c32>,
  <https://github.com/Thomasdezeeuw/a10/commit/f1ff3e4bc374136fef3b0d71f5e9ba7e77cea3b0>,
  <https://github.com/Thomasdezeeuw/a10/commit/fbc6d8478cb89a179a6c72a1dab9fb1406dfa123>.
* Use wrapping add in determining the submission queue index (no more overflow
  after 1 << 32 operations!)
  <https://github.com/Thomasdezeeuw/a10/commit/39289c237453a7f57e3fa604171625a9ef2aed23>.

# v0.2.0

This release adds support for direct descriptors, which are io\_uring specific
file descriptors. Direct descriptor have lower overhead, but an only be used in
io\_uring operations, not regular system calls. It is possible to convert a
direct descriptor into a file descriptor and vica versa.

* `AsyncFd` now has a generic parameter `D` that supports either a regular file
  descriptor (`fd::File`) or a direct desctiptor (`fd::Direct`).
  <https://github.com/Thomasdezeeuw/a10/pull/102>,
  <https://github.com/Thomasdezeeuw/a10/commit/03fe635399f3e453b7b707e080ba239e498e5416>,
  <https://github.com/Thomasdezeeuw/a10/commit/2682bbb6b4e4d18490bf954abc1342ddca003860>,
  <https://github.com/Thomasdezeeuw/a10/commit/b7aafeaa9f615324c4168bc377625b95a14766de>,
  <https://github.com/Thomasdezeeuw/a10/commit/1684af7b2dc3880a0db57dd3dc75184aa66057b8>,
  <https://github.com/Thomasdezeeuw/a10/commit/ab5ae276e0ca70bf960fcc1d9fa03adce778f729>,
  <https://github.com/Thomasdezeeuw/a10/commit/f423cd569aaf18e77e5e392dbf5054b3a35c6145>,
  <https://github.com/Thomasdezeeuw/a10/commit/5d489afdde0a44fb9585970f6d386e42aace87ae>,
  <https://github.com/Thomasdezeeuw/a10/commit/1eea10736d305c62edccffc53066a2ed228da6fb>,
  <https://github.com/Thomasdezeeuw/a10/commit/632fc39963e02f396f26882abf8ca0eb33660ae8>,
  <https://github.com/Thomasdezeeuw/a10/commit/5a8022f6f2d4f6d48a4080987bf9a4af517108ef>,
  <https://github.com/Thomasdezeeuw/a10/commit/10a66cbd7fb40d3dcc6deac2dae2d96e0a0605e2>,
  <https://github.com/Thomasdezeeuw/a10/commit/518dd0901cd80d8f11c96a6aa50a5ea73239ad5e>,
  <https://github.com/Thomasdezeeuw/a10/commit/b6ecb740e9eab2027ffb8d70a76d2285b7b42f83>,
  <https://github.com/Thomasdezeeuw/a10/commit/43d4fe1d085c6a5d1eb26044437ae0b99e74e68b>,
  <https://github.com/Thomasdezeeuw/a10/commit/0763f2c3244956d183a86f3656be28e8899a683c>,
  <https://github.com/Thomasdezeeuw/a10/commit/9f58db14578ef8d8fee2c419417b369d772e2249>,
  <https://github.com/Thomasdezeeuw/a10/commit/b7f2b7fe737c9c5aeee0da2c388497aab9388b2a>,
  <https://github.com/Thomasdezeeuw/a10/commit/c762ea2fced692e3d8b00285de0b720b68a53409>.
* Add `Config::with_direct_descriptors`, enableing the use of direct descriptors
  <https://github.com/Thomasdezeeuw/a10/commit/162ff632c0de2e6d8e85d7c12f433af1f3904450>.
* Adds `AsyncFd::to_file_descriptor` to convert a direct descriptor to a file
  descriptor.
  <https://github.com/Thomasdezeeuw/a10/commit/a913377d7be511a430430235d6b7b6b073c9a4a8>.
* Adds `AsyncFd::to_direct_descriptor` to convert a file descriptor to a direct
  descriptor.
  <https://github.com/Thomasdezeeuw/a10/commit/d611b7866b9b6aeb157a3e954a62803927d93a99>.
* Adds `Signals::to_direct_descriptor` to use direct description for `Signals`
  <https://github.com/Thomasdezeeuw/a10/commit/029f084733c39d26ea9a5bdd923f1c02d5f17c0a>.
* Adds `ReceiveSignals::into_inner`, returns the underlying `Signals`
  <https://github.com/Thomasdezeeuw/a10/commit/fcafbd44dd9b19ec3167c536335dba9e39df2d66>.
* Moves `SubmissionQueue::oneshot_poll` to `poll` module
  <https://github.com/Thomasdezeeuw/a10/commit/5f4b863a806a920d78880b2f525822c0969b80e2>.
* Moves `SubmissionQueue::multishot_poll` to `poll` module
  <https://github.com/Thomasdezeeuw/a10/commit/8787da1ca4a5c09699882a00cc73a1536a417618>.
* Moves `SubmissionQueue::msg_listener` to `msg` module
  <https://github.com/Thomasdezeeuw/a10/commit/0a9ff3d9816702a29caf8f5e62f2d006769599c0>.
* Moves `SubmissionQueue::(try_)send_msg` to `msg` module
  <https://github.com/Thomasdezeeuw/a10/commit/d620de603b5da0bfab6300873d1bfc976515b16e>.
* Moves `signals` module into the `process` module, renames `signal::Receive` to
  `process::ReceiveSignal`, other types are simply moved
  <https://github.com/Thomasdezeeuw/a10/commit/5c011bc07d4bf5596401399ab2b86559d28d2c16>.
* Removes `signals` module
  <https://github.com/Thomasdezeeuw/a10/commit/4305a97adf3b5e3427b80f046a31790f719affa7>.
* Removes
  `SubmissionQueue::{msg_listener,try_send_msg,send_msg,oneshot_poll,multishot_poll}` functions
  <https://github.com/Thomasdezeeuw/a10/commit/4305a97adf3b5e3427b80f046a31790f719affa7>.
* `AsyncFd` now lives in it's own `fd` module, still exported at the root of the
  crate
  <https://github.com/Thomasdezeeuw/a10/commit/ecda8164a32062f7091d862de5c7798d26002d59>.
* Set `IOSQE_ASYNC` for some operations
  <https://github.com/Thomasdezeeuw/a10/commit/a1e25956b6e04e9c678293318967db3f2e4b905a>.

# v0.1.9

* Added `Config::disable` which enables `IORING_SETUP_R_DISABLED`
  <https://github.com/Thomasdezeeuw/a10/commit/08078e34596e31f3a3a706b583d7a2f5f9f8ac8b>.
* Added `Ring::enable` which enables a disable ring
  <https://github.com/Thomasdezeeuw/a10/commit/3817e4f0aa6ac51e131be0b8076e87fa963c4be0>.
* Added `Config::single_issuer` which enables `IORING_SETUP_SINGLE_ISSUER`,
  which is no longer set by default
  <https://github.com/Thomasdezeeuw/a10/commit/e18c618b31bc83564cc23388d8913ebb93e1b310>.
* Added `Config::defer_task_run` which enables `IORING_SETUP_DEFER_TASKRUN`
  <https://github.com/Thomasdezeeuw/a10/commit/0f4d4847392bdccda346b018d7572d41a2a49853>.

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
