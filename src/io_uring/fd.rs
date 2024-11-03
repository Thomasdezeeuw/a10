use std::io;
use std::os::fd::RawFd;

use crate::fd::AsyncFd;
use crate::sys::{self, libc};

/// Direct descriptors are io_uring private file descriptors.
///
/// They avoid some of the overhead associated with thread shared file tables
/// and can be used in any io_uring request that takes a file descriptor.
/// However they cannot be used outside of io_uring.
#[derive(Copy, Clone, Debug)]
pub enum Direct {}

impl crate::fd::Descriptor for Direct {}

impl crate::fd::private::Descriptor for Direct {
    fn use_flags(submission: &mut sys::sq::Submission) {
        submission.use_direct_fd();
    }

    /* TODO(port).
    fn create_flags(submission: &mut Submission) {
        submission.create_direct_fd();
    }

    fn cloexec_flag() -> libc::c_int {
        0 // Direct descriptor always have (the equivalant of) `O_CLOEXEC` set.
    }

    fn cancel_flag() -> u32 {
        libc::IORING_ASYNC_CANCEL_FD_FIXED
    }
    */

    fn fmt_dbg() -> &'static str {
        "direct descriptor"
    }

    fn close(fd: RawFd) -> io::Result<()> {
        // TODO: don't leak the the fd.
        log::warn!(fd = fd; "leaking direct descriptor");
        Ok(())
    }
}

pub(crate) fn fill_close_submission<D: crate::fd::Descriptor>(
    fd: &AsyncFd<D>,
    submission: &mut sys::sq::Submission,
) {
    submission.0.opcode = libc::IORING_OP_CLOSE as u8;
    submission.0.fd = fd.fd();
    D::use_flags(submission);
}
