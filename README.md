# The A10 I/O library. [^1]

This library is meant as a low-level library safely exposing different OS's
abilities to perform non-blocking I/O.

On Linux A10 uses io\_uring, which is a completion based API. For the BSD family
of OS (FreeBSD, OpenBSD, NetBSD, etc.) and for the Apple family (macOS, iOS,
etc.) this uses kqueue, which is a poll based API.

To support both the completion and poll based API most I/O operations need
ownership of the data, e.g. a buffer, so it can delay deallocation if needed.
[^2] The input data can be retrieved again by using the [`Extract`] trait.

Additional documentation can be found in the [`io_uring(7)`] and [`kqueue(2)`]
manuals.

[`io_uring(7)`]: https://man7.org/linux/man-pages/man7/io_uring.7.html
[`kqueue(2)`]: https://man.freebsd.org/cgi/man.cgi?query=kqueue

## Examples

Examples can be found in the [examples directory] of the source code.

[examples directory]: ./examples

[^1]: The name A10 comes from the [A10 ring road around Amsterdam], which
      relates to the ring buffers that io\_uring uses in its design.
[^2]: Delaying of the deallocation needs to happen for completion based APIs
      where an I/O operation `Future` is dropped before it's complete -- the
      OS will continue to use the resources, which would result in a
      use-after-free bug.

[A10 ring road around Amsterdam]: https://en.wikipedia.org/wiki/A10_motorway_(Netherlands)
