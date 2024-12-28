use std::cell::UnsafeCell;
use std::mem;

use crate::process::WaitOn;
use crate::sys::{self, cq, libc, sq};
use crate::SubmissionQueue;

pub(crate) struct WaitIdOp;

impl sys::Op for WaitIdOp {
    type Output = Box<libc::signalfd_siginfo>;
    type Resources = Box<UnsafeCell<libc::signalfd_siginfo>>;
    type Args = (WaitOn, libc::c_int); // options.

    fn fill_submission(
        info: &mut Self::Resources,
        (wait, options): &mut Self::Args,
        submission: &mut sq::Submission,
    ) {
        let (id_type, pid) = match *wait {
            WaitOn::Process(pid) => (libc::P_PID, pid),
            WaitOn::Group(pid) => (libc::P_PGID, pid),
            WaitOn::All => (libc::P_ALL, 0), // NOTE: id is ignored.
        };
        submission.0.opcode = libc::IORING_OP_WAITID as u8;
        submission.0.fd = pid as _;
        submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            addr2: &**info as *const _ as _,
        };
        submission.0.len = id_type;
        submission.0.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            file_index: *options as _,
        };
    }

    fn map_ok(_: &SubmissionQueue, info: Self::Resources, (_, n): cq::OpReturn) -> Self::Output {
        debug_assert!(n == 0);
        // SAFETY: `UnsafeCell` is `repr(transparent)` so this transmute is
        // safe.
        unsafe {
            mem::transmute::<Box<UnsafeCell<libc::signalfd_siginfo>>, Box<libc::signalfd_siginfo>>(
                info,
            )
        }
    }
}
