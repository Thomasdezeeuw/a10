use std::marker::PhantomData;
use std::os::fd::RawFd;
use std::{io, ptr};

use crate::fd::Kind;
use crate::io_uring::sq::{self, Submission};
use crate::io_uring::{self, cq, libc};
use crate::{asan, fd, msan, AsyncFd, SubmissionQueue};

impl Kind {
    pub(crate) fn create_flags(self, submission: &mut Submission) {
        if let Kind::Direct = self {
            submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
                file_index: libc::IORING_FILE_INDEX_ALLOC as u32,
            };
        }
    }

    pub(crate) fn use_flags(&self, submission: &mut Submission) {
        if let Kind::Direct = self {
            submission.0.flags |= libc::IOSQE_FIXED_FILE;
        }
    }
}
