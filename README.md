# A10

The [A10] io\_uring library.

This library is meant as a low-level library safely exposing the io\_uring API.
A10 is expected to be integrated into a `Future` runtime, but it can work as a
stand-alone library.

For simplicity this only has two main types and a number of helper types:
 * `Ring` is a wrapper around io\_uring used to poll for completion events.
 * `AsyncFd` is a wrapper around a file descriptor that provides a safe API to
   schedule operations.

[A10]: https://en.wikipedia.org/wiki/A10_motorway_(Netherlands)

## Linux Required

Currently this requires a fairly new Linux kernel version, everything should
work on Linux v6.1 and up.

## Examples

Examples can be found in the [examples directory] of the source code.

[examples directory]: ./examples
