# Generate `sys.rs`

First get a copy of liburing, which holds the function and type definitions,
e.g. from <https://github.com/axboe/liburing>.

Then run `make` and copy all files from `liburing/src/include` into `include`.
