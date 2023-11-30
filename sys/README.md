# Generate `sys.rs`

First get a copy of liburing, which holds the function and type definitions,
e.g. from <https://github.com/axboe/liburing>.

Then run `make` and copy all files from `liburing/src/include` into `include`.

After the headers are prepared run `cargo build` and copy `src/sys.rs` to
`../src/sys.rs` and run `cargo fmt` at the root.
