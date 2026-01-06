use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{ptr, slice};

use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice};
use crate::io_uring::op::{OpReturn, Singleshot, State};
use crate::io_uring::{self, cq, libc, sq};
use crate::net::{AddressStorage, Domain, NoAddress, OptionStorage, Protocol, SocketAddress, Type};
use crate::{AsyncFd, SubmissionQueue, fd};

pub(crate) use crate::unix::MsgHeader;

pub(crate) struct SocketOp;

impl io_uring::op::Op for SocketOp {
    type Output = AsyncFd;
    type Resources = fd::Kind;
    type Args = (Domain, Type, Protocol);

    #[allow(clippy::cast_sign_loss)]
    fn fill_submission(
        fd_kind: &mut Self::Resources,
        (domain, r#type, protocol): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        submission.0.opcode = libc::IORING_OP_SOCKET as u8;
        submission.0.fd = domain.0;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: r#type.0.into(),
        };
        submission.0.len = protocol.0;
        // Must currently always be set to zero per the manual.
        submission.0.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { rw_flags: 0 };
        /* TODO.
        if let fd::Kind::Direct = *fd_kind {
            io_uring::fd::create_direct_flags(submission);
        }
        */
    }

    #[allow(clippy::cast_possible_wrap)]
    fn map_ok(sq: &SubmissionQueue, fd_kind: Self::Resources, (_, fd): OpReturn) -> Self::Output {
        // SAFETY: kernel ensures that `fd` is valid.
        unsafe { AsyncFd::from_raw(fd as RawFd, fd_kind, sq.clone()) }
    }
}
