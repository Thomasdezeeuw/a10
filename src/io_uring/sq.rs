use std::os::fd::AsRawFd;
use std::sync::atomic::{self, AtomicBool, Ordering};
use std::{fmt, io, ptr};

use crate::sq::{Cancelled, QueueFull};
use crate::sys::{cancel, libc, Shared};
use crate::{OperationId, WAKE_ID};

/// NOTE: all the state is in [`Shared`].
#[derive(Debug)]
pub(crate) struct Submissions {
    // All state is in `Shared`.
}

impl Submissions {
    pub(crate) const fn new() -> Submissions {
        Submissions {}
    }
}

impl crate::sq::Submissions for Submissions {
    type Shared = Shared;
    type Submission = Submission;

    #[allow(clippy::mutex_integer)]
    fn add<F>(
        &self,
        shared: &Self::Shared,
        is_polling: &AtomicBool,
        submit: F,
    ) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Self::Submission),
    {
        // First we need to acquire mutable access to an `Submission` entry in
        // the `entries` array.
        //
        // We do this by increasing `pending_tail` by 1, reserving
        // `entries[pending_tail]` for ourselves, while ensuring we don't go
        // beyond what the kernel has processed by checking `tail - kernel_read`
        // is less then the length of the submission queue.
        let kernel_read = shared.kernel_read();
        let tail = shared
            .pending_tail
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |tail| {
                if tail - kernel_read < shared.entries_len {
                    // Still an entry available.
                    Some(tail.wrapping_add(1))
                } else {
                    None
                }
            });
        let Ok(tail) = tail else {
            // If the kernel thread is not awake we'll need to wake it to make
            // space in the submission queue.
            shared.maybe_wake_kernel_thread();
            return Err(QueueFull);
        };

        // SAFETY: the `ring_mask` ensures we can never get an index larger
        // then the size of the queue. Above we've already ensured that
        // we're the only thread  with mutable access to the entry.
        let submission_index = tail & shared.entries_mask;
        let submission = unsafe { &mut *shared.entries.add(submission_index as usize) };

        // Let the caller fill the `submission`.
        submission.reset();
        submit(submission);
        #[cfg(debug_assertions)]
        debug_assert!(!submission.is_unchanged());

        // Ensure that all writes to the `submission` are done.
        atomic::fence(Ordering::SeqCst);

        // Now that we've written our submission we need add it to the
        // `array` so that the kernel can process it.
        log::trace!(submission:? = submission; "queueing submission");
        {
            // Now that the submission is filled we need to add it to the
            // `shared.array` so that the kernel can read from it.
            //
            // We do this with a lock to avoid a race condition between two
            // threads incrementing `shared.tail` concurrently. Consider the
            // following execution:
            //
            // Thread A                           | Thread B
            // ...                                | ...
            // ...                                | Got `array_index` 0.
            // Got `array_index` 1.               |
            // Writes index to `shared.array[1]`. |
            // `shared.tail.fetch_add` to 1.      |
            // At this point the kernel will/can read `shared.array[0]`, but
            // thread B hasn't filled it yet. So the kernel will read an invalid
            // index!
            //                                    | Writes index to `shared.array[0]`.
            //                                    | `shared.tail.fetch_add` to 2.

            let mut array_index = shared.array_index.lock().unwrap();
            let idx = (*array_index & shared.entries_mask) as usize;
            // SAFETY: `idx` is masked above to be within the correct bounds.
            unsafe { (*shared.array.add(idx)).store(submission_index, Ordering::Release) };
            // SAFETY: we filled the array above.
            let old_tail = unsafe { (*shared.array_tail).fetch_add(1, Ordering::AcqRel) };
            debug_assert!(old_tail == *array_index);
            *array_index += 1;
        }

        // If the kernel thread is not awake we'll need to wake it for it to
        // process our submission.
        shared.maybe_wake_kernel_thread();
        // When we're not using the kernel polling thread we might have to
        // submit the event ourselves to ensure we can make progress while the
        // (user space) polling thread is calling `Ring::poll`.
        shared.maybe_submit_event(is_polling);
        Ok(())
    }

    fn cancel(
        &self,
        shared: &Self::Shared,
        is_polling: &AtomicBool,
        op_id: OperationId,
    ) -> Cancelled {
        let result = self.add(shared, is_polling, |submission| {
            use crate::sq::Submission;
            submission.set_id(op_id);
            // We'll get a canceled completion event if we succeeded, which is
            // sufficient to cleanup the operation.
            submission.no_completion_event();
            cancel::operation(op_id, submission);
        });
        if let Ok(()) = result {
            return Cancelled::Async;
        }

        // We failed to asynchronously cancel the operation, so we'll fallback
        // to doing it synchronously.
        let cancel = libc::io_uring_sync_cancel_reg {
            addr: op_id as u64,
            fd: 0,
            flags: 0,
            // No timeout, saves the kernel from setting up a timer etc.
            timeout: libc::__kernel_timespec {
                tv_sec: libc::__kernel_time64_t::MAX,
                tv_nsec: std::os::raw::c_longlong::MAX,
            },
            opcode: 0,
            pad: [0; 7],
            pad2: [0; 3],
        };
        let arg = ptr::from_ref(&cancel).cast();
        match shared.register(libc::IORING_REGISTER_SYNC_CANCEL, arg, 1) {
            Ok(()) => Cancelled::Immediate,
            Err(err) => match err.raw_os_error() {
                // Operation was already completed.
                Some(libc::ENOENT) => Cancelled::Immediate,
                // Operation is nearly complete, can't be cancelled
                // anymore (EALREADY), or the cancellation failed
                // (ETIME, EINVAL). Either way we'll have to wait until
                // the operation is completed.
                Some(libc::EALREADY | libc::ETIME | libc::EINVAL) => Cancelled::Async,
                _ => {
                    log::error!(id = op_id; "unexpected error cancelling operation: {err}");
                    // Waiting is the safest thing we can do.
                    Cancelled::Async
                }
            },
        }
    }

    fn wake(&self, shared: &Self::Shared) -> io::Result<()> {
        // This is only called if we're not polling, so we can set `is_polling`
        // to false and we ignore the queue full error.
        let is_polling = AtomicBool::new(false);
        let _: Result<(), QueueFull> = self.add(shared, &is_polling, |submission| {
            submission.0.opcode = libc::IORING_OP_MSG_RING as u8;
            submission.0.fd = shared.rfd.as_raw_fd();
            submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
                addr: u64::from(libc::IORING_MSG_DATA),
            };
            submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: WAKE_ID as _ };
            submission.no_completion_event();
        });
        Ok(())
    }
}

/// Submission event.
///
/// # Safety
///
/// It is up to the caller to ensure any data passed to the kernel outlives the
/// operation.
#[repr(transparent)]
pub(crate) struct Submission(pub(crate) libc::io_uring_sqe);

impl Submission {
    /// Reset the submission.
    #[allow(clippy::assertions_on_constants)]
    fn reset(&mut self) {
        debug_assert!(libc::IORING_OP_NOP == 0);
        unsafe { ptr::addr_of_mut!(self.0).write_bytes(0, 1) };
    }

    /// Don't return a completion event for this submission.
    pub(crate) fn no_completion_event(&mut self) {
        self.0.flags |= libc::IOSQE_CQE_SKIP_SUCCESS;
    }

    /// Returns `true` if the submission is unchanged after a [`reset`].
    ///
    /// [`reset`]: Submission::reset
    #[cfg(debug_assertions)]
    const fn is_unchanged(&self) -> bool {
        self.0.opcode == libc::IORING_OP_NOP as u8
    }

    /// Don't attempt to do the operation non-blocking first, always execute it
    /// in an async manner.
    pub(crate) fn set_async(&mut self) {
        self.0.flags |= libc::IOSQE_ASYNC;
    }
}

impl crate::sq::Submission for Submission {
    fn set_id(&mut self, id: OperationId) {
        self.0.user_data = id as _;
    }
}

impl fmt::Debug for Submission {
    #[allow(clippy::too_many_lines)] // Not beneficial to split this up.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Helper functions with common patterns.
        fn io_op(f: &mut fmt::DebugStruct<'_, '_>, submission: &libc::io_uring_sqe, name: &str) {
            f.field("opcode", &name)
                .field("fd", &submission.fd)
                .field("offset", unsafe { &submission.__bindgen_anon_1.off })
                .field("addr", unsafe { &submission.__bindgen_anon_2.addr })
                .field("len", &submission.len);
        }
        fn net_op(f: &mut fmt::DebugStruct<'_, '_>, submission: &libc::io_uring_sqe, name: &str) {
            // NOTE: can't reference a packed struct's field.
            let buf_group = unsafe { submission.__bindgen_anon_4.buf_group };
            f.field("opcode", &name)
                .field("fd", &submission.fd)
                .field("addr", unsafe { &submission.__bindgen_anon_2.addr })
                .field("len", &submission.len)
                .field("msg_flags", unsafe {
                    &submission.__bindgen_anon_3.msg_flags
                })
                .field("ioprio", &submission.ioprio)
                .field("buf_group", &buf_group);
        }

        let mut f = f.debug_struct("io_uring::Submission");
        match u32::from(self.0.opcode) {
            libc::IORING_OP_NOP => {
                f.field("opcode", &"IORING_OP_NOP");
            }
            libc::IORING_OP_FSYNC => {
                f.field("opcode", &"IORING_OP_FSYNC")
                    .field("fd", &self.0.fd)
                    .field("fsync_flags", unsafe {
                        &self.0.__bindgen_anon_3.fsync_flags
                    });
            }
            libc::IORING_OP_READ => io_op(&mut f, &self.0, "IORING_OP_READ"),
            libc::IORING_OP_READV => io_op(&mut f, &self.0, "IORING_OP_READV"),
            libc::IORING_OP_WRITE => io_op(&mut f, &self.0, "IORING_OP_WRITE"),
            libc::IORING_OP_WRITEV => io_op(&mut f, &self.0, "IORING_OP_WRITEV"),
            libc::IORING_OP_RENAMEAT => {
                f.field("opcode", &"IORING_OP_RENAMEAT")
                    .field("old_fd", &self.0.fd)
                    .field("old_path", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("new_fd", &self.0.len)
                    .field("new_path", unsafe { &self.0.__bindgen_anon_1.off })
                    .field("rename_flags", unsafe {
                        &self.0.__bindgen_anon_3.rename_flags
                    });
            }
            libc::IORING_OP_SOCKET => {
                f.field("opcode", &"IORING_OP_SOCKET")
                    .field("domain", &self.0.fd)
                    .field("type", unsafe { &self.0.__bindgen_anon_1.off })
                    .field("file_index", unsafe { &self.0.__bindgen_anon_5.file_index })
                    .field("protocol", &self.0.len)
                    .field("rw_flags", unsafe { &self.0.__bindgen_anon_3.rw_flags });
            }
            libc::IORING_OP_CONNECT => {
                f.field("opcode", &"IORING_OP_CONNECT")
                    .field("fd", &self.0.fd)
                    .field("addr", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("addr_size", unsafe { &self.0.__bindgen_anon_1.off });
            }
            libc::IORING_OP_SEND => net_op(&mut f, &self.0, "IORING_OP_SEND"),
            libc::IORING_OP_SEND_ZC => net_op(&mut f, &self.0, "IORING_OP_SEND_ZC"),
            libc::IORING_OP_SENDMSG => net_op(&mut f, &self.0, "IORING_OP_SENDMSG"),
            libc::IORING_OP_SENDMSG_ZC => net_op(&mut f, &self.0, "IORING_OP_SENDMSG_ZC"),
            libc::IORING_OP_RECV => net_op(&mut f, &self.0, "IORING_OP_RECV"),
            libc::IORING_OP_RECVMSG => net_op(&mut f, &self.0, "IORING_OP_RECVMSG"),
            libc::IORING_OP_SHUTDOWN => {
                f.field("opcode", &"IORING_OP_SHUTDOWN")
                    .field("fd", &self.0.fd)
                    .field("how", &self.0.len);
            }
            libc::IORING_OP_ACCEPT => {
                f.field("opcode", &"IORING_OP_ACCEPT")
                    .field("fd", &self.0.fd)
                    .field("addr", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("addr_size", unsafe { &self.0.__bindgen_anon_1.off })
                    .field("accept_flags", unsafe {
                        &self.0.__bindgen_anon_3.accept_flags
                    })
                    .field("file_index", unsafe { &self.0.__bindgen_anon_5.file_index })
                    .field("ioprio", &self.0.ioprio);
            }
            libc::IORING_OP_ASYNC_CANCEL => {
                f.field("opcode", &"IORING_OP_ASYNC_CANCEL");
                let cancel_flags = unsafe { self.0.__bindgen_anon_3.cancel_flags };
                #[allow(clippy::if_not_else)]
                if (cancel_flags & libc::IORING_ASYNC_CANCEL_FD) != 0 {
                    f.field("fd", &self.0.fd)
                        .field("cancel_flags", &cancel_flags);
                } else {
                    f.field("addr", unsafe { &self.0.__bindgen_anon_2.addr });
                }
            }
            libc::IORING_OP_OPENAT => {
                f.field("opcode", &"IORING_OP_OPENAT")
                    .field("dirfd", &self.0.fd)
                    .field("pathname", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("mode", &self.0.len)
                    .field("open_flags", unsafe { &self.0.__bindgen_anon_3.open_flags })
                    .field("file_index", unsafe { &self.0.__bindgen_anon_5.file_index });
            }
            libc::IORING_OP_SPLICE => {
                f.field("opcode", &"IORING_OP_SPLICE")
                    .field("fd_in", unsafe { &self.0.__bindgen_anon_5.splice_fd_in })
                    .field("off_in", unsafe { &self.0.__bindgen_anon_2.splice_off_in })
                    .field("fd_out", &self.0.fd)
                    .field("off_out", unsafe { &self.0.__bindgen_anon_1.off })
                    .field("len", &self.0.len)
                    .field("splice_flags", unsafe {
                        &self.0.__bindgen_anon_3.splice_flags
                    });
            }
            libc::IORING_OP_CLOSE => {
                f.field("opcode", &"IORING_OP_CLOSE")
                    .field("fd", &self.0.fd);
            }
            libc::IORING_OP_FILES_UPDATE => {
                f.field("opcode", &"IORING_OP_FILES_UPDATE")
                    .field("fd", &self.0.fd)
                    .field("offset", unsafe { &self.0.__bindgen_anon_1.off })
                    .field("fds", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("len", &self.0.len);
            }
            libc::IORING_OP_STATX => {
                f.field("opcode", &"IORING_OP_STATX")
                    .field("fd", &self.0.fd)
                    .field("pathname", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("statx_flags", unsafe {
                        &self.0.__bindgen_anon_3.statx_flags
                    })
                    .field("mask", &self.0.len)
                    .field("statx", unsafe { &self.0.__bindgen_anon_1.off });
            }
            libc::IORING_OP_FADVISE => {
                f.field("opcode", &"IORING_OP_FADVISE")
                    .field("fd", &self.0.fd)
                    .field("offset", unsafe { &self.0.__bindgen_anon_1.off })
                    .field("len", &self.0.len)
                    .field("advise", unsafe { &self.0.__bindgen_anon_3.fadvise_advice });
            }
            libc::IORING_OP_FALLOCATE => {
                f.field("opcode", &"IORING_OP_FALLOCATE")
                    .field("fd", &self.0.fd)
                    .field("offset", unsafe { &self.0.__bindgen_anon_1.off })
                    .field("len", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("mode", &self.0.len);
            }
            libc::IORING_OP_UNLINKAT => {
                f.field("opcode", &"IORING_OP_UNLINKAT")
                    .field("dirfd", &self.0.fd)
                    .field("path", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("unlink_flags", unsafe {
                        &self.0.__bindgen_anon_3.unlink_flags
                    });
            }
            libc::IORING_OP_MKDIRAT => {
                f.field("opcode", &"IORING_OP_MKDIRAT")
                    .field("dirfd", &self.0.fd)
                    .field("path", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("mode", &self.0.len);
            }
            libc::IORING_OP_POLL_ADD => {
                f.field("opcode", &"IORING_OP_POLL_ADD")
                    .field("fd", &self.0.fd)
                    .field("poll_events", unsafe {
                        &self.0.__bindgen_anon_3.poll32_events
                    })
                    .field("multishot", &(self.0.len == libc::IORING_POLL_ADD_MULTI));
            }
            libc::IORING_OP_POLL_REMOVE => {
                f.field("opcode", &"IORING_OP_POLL_REMOVE")
                    .field("target_user_data", unsafe { &self.0.__bindgen_anon_2.addr });
            }
            libc::IORING_OP_MADVISE => {
                f.field("opcode", &"IORING_OP_MADVISE")
                    .field("address", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("len", &self.0.len)
                    .field("advise", unsafe { &self.0.__bindgen_anon_3.fadvise_advice });
            }
            libc::IORING_OP_MSG_RING => {
                f.field("opcode", &"IORING_OP_MSG_RING")
                    .field("ringfd", &self.0.fd)
                    .field("msg1", &self.0.len)
                    .field("msg2", unsafe { &self.0.__bindgen_anon_1.off });
            }
            libc::IORING_OP_WAITID => {
                f.field("opcode", &"IORING_OP_WAITID")
                    .field("id", &self.0.fd)
                    .field("id_type", &self.0.len)
                    .field("options", unsafe { &self.0.__bindgen_anon_5.file_index })
                    .field("info", unsafe { &self.0.__bindgen_anon_1.addr2 });
            }
            libc::IORING_OP_FIXED_FD_INSTALL => {
                f.field("opcode", &"IORING_OP_FIXED_FD_INSTALL")
                    .field("fd", &self.0.fd)
                    .field("install_fd_flags", unsafe {
                        &self.0.__bindgen_anon_3.install_fd_flags
                    });
            }
            _ => {
                // NOTE: we can't access the unions safely without know what
                // fields to read.
                f.field("opcode", &self.0.opcode)
                    .field("ioprio", &self.0.ioprio)
                    .field("fd", &self.0.fd)
                    .field("len", &self.0.len)
                    .field("personality", &self.0.personality);
            }
        }
        f.field("flags", &self.0.flags)
            .field("user_data", &self.0.user_data)
            .finish()
    }
}
