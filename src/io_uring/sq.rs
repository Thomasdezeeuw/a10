use std::os::fd::RawFd;
use std::sync::Arc;
use std::sync::atomic::{self, Ordering};
use std::{fmt, io, ptr, task};

use crate::io_uring::{Shared, cq, libc, load_kernel_shared};
use crate::{asan, lock};

#[derive(Clone, Debug)]
pub(crate) struct Submissions {
    shared: Arc<Shared>,
}

impl Submissions {
    pub(crate) fn new(shared: Shared) -> Submissions {
        Submissions {
            shared: Arc::new(shared),
        }
    }

    /// Try to submit a new operation.
    pub(crate) fn add<F>(&self, fill_submission: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Submission),
    {
        let shared = &*self.shared;
        let len = shared.submissions_len;
        // Before grabbing a lock, see if there is space in the queue.
        if load_kernel_shared(shared.submissions_tail) - load_kernel_shared(shared.submissions_head)
            >= len
        {
            return Err(QueueFull);
        }

        // Grab the submission lock.
        let submissions_guard = lock(&shared.submissions_lock);

        // NOTE: need to load the tail and head values again as they could have
        // changed since we last loaded them.
        let tail = load_kernel_shared(shared.submissions_tail);
        let head = load_kernel_shared(shared.submissions_head);
        if (tail - head) > len {
            return Err(QueueFull);
        }

        // SAFETY: with the mask we've ensured above that index is valid for the
        // number of submissions. Furthermore while holding the lock we're
        // ensure of unique access to the submission.
        let index = (tail & (len - 1)) as usize;
        let submission = unsafe { &mut *shared.submissions.add(index).as_ptr() };
        asan::unpoison(submission);

        // Reset and fill the submission.
        submission.reset();
        fill_submission(submission);
        #[cfg(debug_assertions)]
        debug_assert!(!submission.is_unchanged());

        // Ensure that all writes to the submission are done.
        atomic::fence(Ordering::SeqCst);

        // SAFETY: submissions_tail is a valid pointer. It's safe to use store
        // here because we're holding the submission lock and thus have unique
        // access to the submissions.
        let new_tail = tail.wrapping_add(1);
        unsafe { (*shared.submissions_tail.as_ptr()).store(new_tail, Ordering::Release) }
        drop(submissions_guard);

        log::trace!(submission:?, index, tail, new_tail; "queueing submission");
        asan::poison(submission);

        Ok(())
    }

    /// Asynchronously cancel an operation.
    pub(super) fn cancel(&self, user_data: u64) -> Result<(), QueueFull> {
        self.add(|submission| {
            submission.0.opcode = libc::IORING_OP_ASYNC_CANCEL as u8;
            submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: user_data };
            // We'll get a canceled completion event if we succeeded, which is
            // sufficient to cleanup the operation.
            submission.no_success_event();
        })
    }

    pub(crate) fn wake(&self) -> io::Result<()> {
        if !self.shared.is_polling.load(Ordering::Acquire) {
            // If we're not polling we don't need to wake up.
            return Ok(());
        }

        _ = self.add(|submission| {
            submission.0.opcode = libc::IORING_OP_MSG_RING as u8;
            submission.0.fd = self.ring_fd();
            submission.0.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
                addr: u64::from(libc::IORING_MSG_DATA),
            };
            submission.0.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
                off: cq::WAKE_USER_DATA,
            };
            submission.no_success_event();
        });

        let (flags, submissions) = if self.shared.kernel_thread {
            let flags = if load_kernel_shared(self.shared.kernel_flags)
                & libc::IORING_SQ_NEED_WAKEUP
                != 0
            {
                libc::IORING_ENTER_SQ_WAKEUP
            } else {
                0
            };
            (flags, 0) // Kernel thread handles the submissions.
        } else {
            (0, self.shared.unsubmitted_submissions())
        };
        log::trace!(submissions; "entering kernel for wake up");
        self.shared.enter(submissions, 0, flags, ptr::null(), 0)?;

        Ok(())
    }

    /// Wait for a submission slot, waking `waker` once one is available.
    pub(crate) fn wait_for_submission(&self, waker: task::Waker) {
        log::trace!(waker:?; "adding future waiting on submission slot");
        let shared = &*self.shared;
        lock(&shared.blocked_futures).push(waker);
    }

    pub(crate) fn shared(&self) -> &Shared {
        &self.shared
    }

    pub(super) fn ring_fd(&self) -> RawFd {
        self.shared.ring_fd()
    }
}

/// Submission queue is full.
pub(crate) struct QueueFull;

impl From<QueueFull> for io::Error {
    fn from(_: QueueFull) -> io::Error {
        #[cfg(not(feature = "nightly"))]
        let kind = io::ErrorKind::Other;
        #[cfg(feature = "nightly")]
        let kind = io::ErrorKind::ResourceBusy;
        io::Error::new(kind, "submission queue is full")
    }
}

impl fmt::Debug for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueueFull").finish()
    }
}

impl fmt::Display for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("`a10::Ring` submission queue is full")
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
        unsafe { ptr::from_mut(&mut self.0).write_bytes(0, 1) };
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
    pub(super) fn set_async(&mut self) {
        self.0.flags |= libc::IOSQE_ASYNC;
    }

    /// Don't return a completion event for this submission if it succeeds.
    pub(super) fn no_success_event(&mut self) {
        self.0.flags |= libc::IOSQE_CQE_SKIP_SUCCESS;
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
            libc::IORING_OP_READ_MULTISHOT => {
                let buf_group = unsafe { self.0.__bindgen_anon_4.buf_group };
                f.field("opcode", &"IORING_OP_READ_MULTISHOT")
                    .field("fd", &self.0.fd)
                    .field("flags", &self.0.flags)
                    .field("buf_group", &buf_group);
            }
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
                    .field("domain", &crate::net::Domain(self.0.fd))
                    .field("type", &unsafe {
                        crate::net::Type(self.0.__bindgen_anon_1.off as u32)
                    })
                    .field("file_index", unsafe { &self.0.__bindgen_anon_5.file_index })
                    .field("protocol", &crate::net::Protocol(self.0.len))
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
            libc::IORING_OP_BIND => {
                f.field("opcode", &"IORING_OP_BIND")
                    .field("fd", &self.0.fd)
                    .field("addr", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("addr_size", unsafe { &self.0.__bindgen_anon_1.addr2 });
            }
            libc::IORING_OP_LISTEN => {
                f.field("opcode", &"IORING_OP_LISTEN")
                    .field("fd", &self.0.fd)
                    .field("backlog", &self.0.len);
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
            libc::IORING_OP_URING_CMD => {
                fn sockopt(
                    f: &mut fmt::DebugStruct<'_, '_>,
                    submission: &libc::io_uring_sqe,
                    name: &str,
                ) {
                    let level = crate::net::Level(unsafe {
                        submission.__bindgen_anon_2.__bindgen_anon_1.level
                    });
                    let opt = unsafe { submission.__bindgen_anon_2.__bindgen_anon_1.optname };
                    f.field("cmd_op", &name).field("level", &level);
                    match level {
                        // NOTE: Unix also uses `Level::SOCKET`, so
                        // can't use `UnixOpt`.
                        crate::net::Level::SOCKET => f.field("name", &crate::net::SocketOpt(opt)),
                        crate::net::Level::IPV4 => f.field("name", &crate::net::IPv4Opt(opt)),
                        crate::net::Level::IPV6 => f.field("name", &crate::net::IPv6Opt(opt)),
                        crate::net::Level::TCP => f.field("name", &crate::net::TcpOpt(opt)),
                        crate::net::Level::UDP => f.field("name", &crate::net::UdpOpt(opt)),
                        _ => f.field("name", &crate::net::Opt(opt)),
                    };
                    f.field("len", unsafe { &submission.__bindgen_anon_5.optlen });
                }

                f.field("opcode", &"IORING_OP_URING_CMD")
                    .field("fd", &self.0.fd);
                match unsafe { self.0.__bindgen_anon_1.__bindgen_anon_1.cmd_op } {
                    libc::SOCKET_URING_OP_GETSOCKOPT => {
                        sockopt(&mut f, &self.0, "SOCKET_URING_OP_GETSOCKOPT");
                    }
                    libc::SOCKET_URING_OP_SETSOCKOPT => {
                        sockopt(&mut f, &self.0, "SOCKET_URING_OP_SETSOCKOPT");
                        f.field("value", unsafe { &*self.0.__bindgen_anon_6.optval });
                    }
                    op => {
                        f.field("cmd_op", &op);
                    }
                }
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
                    .field("fd", &self.0.fd)
                    .field("file_index", unsafe { &self.0.__bindgen_anon_5.file_index });
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
            libc::IORING_OP_FTRUNCATE => {
                f.field("opcode", &"IORING_OP_FTRUNCATE")
                    .field("fd", &self.0.fd)
                    .field("len", unsafe { &self.0.__bindgen_anon_1.off });
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
            libc::IORING_OP_PIPE => {
                f.field("opcode", &"IORING_OP_PIPE")
                    .field("fds", unsafe { &self.0.__bindgen_anon_2.addr })
                    .field("pipe_flags", unsafe { &self.0.__bindgen_anon_3.pipe_flags });
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
            .field("user_data", &(self.0.user_data as *const ()))
            .finish()
    }
}
