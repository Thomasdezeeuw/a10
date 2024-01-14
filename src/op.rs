//! Code related to executing an asynchronous operations.

use std::mem::replace;
use std::os::fd::RawFd;
use std::task::{self, Poll};
use std::{fmt, io, ptr};

use crate::cancel::{CancelOp, CancelResult};
use crate::{libc, Completion, OpIndex, QueueFull, SubmissionQueue};

/// State of a queued operation.
#[derive(Debug)]
pub(crate) struct QueuedOperation {
    /// Operation kind.
    kind: QueuedOperationKind,
    /// True if the connected `Future`/`AsyncIterator` is dropped and thus no
    /// longer will retrieve the result.
    dropped: bool,
    /// Boolean used by operations that result in multiple completion events.
    /// For example zero copy: one completion to report the result another to
    /// indicate the resources are no longer used.
    /// For multishot this will be true if no more completion events are coming,
    /// for example in case a previous event returned an error.
    done: bool,
    /// Waker to wake when the operation is done.
    waker: Option<task::Waker>,
}

impl QueuedOperation {
    /// Create a queued operation.
    pub(crate) const fn new() -> QueuedOperation {
        QueuedOperation::_new(QueuedOperationKind::Single { result: None })
    }

    /// Create a queued multishot operation.
    pub(crate) const fn new_multishot() -> QueuedOperation {
        QueuedOperation::_new(QueuedOperationKind::Multishot {
            results: Vec::new(),
        })
    }

    const fn _new(kind: QueuedOperationKind) -> QueuedOperation {
        QueuedOperation {
            kind,
            done: false,
            dropped: false,
            waker: None,
        }
    }

    /// Update the operation based on a completion `event`.
    pub(crate) fn update(&mut self, event: &Completion) -> bool {
        let completion = CompletionResult {
            result: event.result(),
            flags: event.operation_flags(),
        };
        match &mut self.kind {
            QueuedOperationKind::Single { result } => {
                if event.is_notification() {
                    // Zero copy completed, we can now mark ourselves as done.
                    self.done = true;
                } else {
                    let old = replace(result, Some(completion));
                    debug_assert!(old.is_none());
                    // For zero copy this may be false, in which case we get a
                    // notification (see above) in a future completion event.
                    self.done = !event.is_in_progress();
                }

                if self.done {
                    // NOTE: if `dropped` is true this drops the operations's
                    // resources (e.g. buffers).
                    if let Some(waker) = self.waker.take() {
                        waker.wake();
                    }
                }
            }
            QueuedOperationKind::Multishot { results } => {
                results.push(completion);
                if let Some(waker) = self.waker.take() {
                    waker.wake();
                }
            }
        }
        self.dropped
    }

    /// Poll the operation check if it's ready.
    ///
    /// Returns the `flags` and the `result` (always positive).
    ///
    /// For multishot operations: if this returns `Poll::Pending` the caller
    /// should check `is_done` to determine if the previous result was the last
    /// one.
    #[allow(clippy::needless_pass_by_ref_mut)] // Match `Future` API.
    pub(crate) fn poll(&mut self, ctx: &mut task::Context<'_>) -> Poll<io::Result<(u16, i32)>> {
        match &mut self.kind {
            QueuedOperationKind::Single { result } => {
                if let (true, Some(result)) = (self.done, result.as_ref()) {
                    return Poll::Ready(result.as_result());
                }
            }
            QueuedOperationKind::Multishot { results } => {
                if !results.is_empty() {
                    let completion = results.remove(0);
                    if completion.result.is_negative() {
                        // If we get an error the multishot operation is done.
                        self.done = true;
                    }
                    return Poll::Ready(completion.as_result());
                }
            }
        }

        if !self.done {
            // Still in progress.
            let waker = ctx.waker();
            if !matches!(&self.waker, Some(w) if w.will_wake(waker)) {
                self.waker = Some(waker.clone());
            }
        }
        // NOTE: we can get here multishot operations (see note in docs) or if
        // the `Future` is used in an invalid way (e.g. poll after completion).
        // In either case we don't to set the waker.
        Poll::Pending
    }

    /// Poll the operation for a message.
    ///
    /// Returns the `flags` and the `result` (always positive).
    #[allow(clippy::needless_pass_by_ref_mut)] // Match `Future` API.
    pub(crate) fn poll_msg(&mut self, ctx: &mut task::Context<'_>) -> Poll<(u16, u32)> {
        match &mut self.kind {
            QueuedOperationKind::Multishot { results } => {
                if !results.is_empty() {
                    let completion = results.remove(0);
                    let data = u32::from_ne_bytes(completion.result.to_ne_bytes());
                    return Poll::Ready((completion.flags, data));
                }
            }
            QueuedOperationKind::Single { .. } => {
                panic!("QueuedOperation::poll_msg called incorrectly")
            }
        }

        // Still in progress.
        let waker = ctx.waker();
        if !matches!(&self.waker, Some(w) if w.will_wake(waker)) {
            self.waker = Some(waker.clone());
        }
        Poll::Pending
    }

    /// Returns true if the operation is done.
    pub(crate) const fn is_done(&self) -> bool {
        self.done
    }

    /// Set the state of the operation as dropped, but still in progress kernel
    /// side. This set the waker to `waker` and make `set_result` return `true`.
    pub(crate) fn set_dropped(&mut self, waker: Option<task::Waker>) {
        self.dropped = true;
        self.waker = waker;
    }
}

/// [`QueuedOperation`] kind.
#[derive(Debug)]
enum QueuedOperationKind {
    /// Single result operation.
    Single {
        /// Result of the operation.
        result: Option<CompletionResult>,
    },
    /// Multishot operation, which expects multiple results for the same
    /// operation.
    Multishot {
        /// Results for the operation.
        results: Vec<CompletionResult>,
    },
}

/// Completed result of an operation.
#[derive(Copy, Clone, Debug)]
struct CompletionResult {
    /// The 16 upper bits of `io_uring_cqe::flags`, e.g. the index of a buffer
    /// in a buffer pool.
    flags: u16,
    /// The result of an operation; negative is a (negative) errno, positive a
    /// successful result. The meaning is depended on the operation itself.
    result: i32,
}

impl CompletionResult {
    fn as_result(self) -> io::Result<(u16, i32)> {
        if self.result.is_negative() {
            // TODO: handle `-EBUSY` on operations.
            // TODO: handle io_uring specific errors here, read CQE
            // ERRORS in the manual.
            Err(io::Error::from_raw_os_error(-self.result))
        } else {
            Ok((self.flags, self.result))
        }
    }
}

/// Submission event.
///
/// # Safety
///
/// It is up to the caller to ensure any data passed to the kernel outlives the
/// operation.
#[repr(transparent)]
pub(crate) struct Submission {
    inner: libc::io_uring_sqe,
}

/// The manual says:
/// > If offs is set to -1, the offset will use (and advance) the file
/// > position, like the read(2) and write(2) system calls.
///
/// `-1` cast as `unsigned long long` in C is the same as as `u64::MAX`.
pub(crate) const NO_OFFSET: u64 = u64::MAX;

// Can't do much about this, flags are defined as signed, but io_uring mostly
// uses unsigned.
#[allow(clippy::cast_sign_loss)]
impl Submission {
    /// Reset the submission.
    #[allow(clippy::assertions_on_constants)]
    pub(crate) fn reset(&mut self) {
        debug_assert!(libc::IORING_OP_NOP == 0);
        unsafe { ptr::addr_of_mut!(self.inner).write_bytes(0, 1) };
    }

    /// Set the user data to `user_data`.
    pub(crate) fn set_user_data(&mut self, user_data: u64) {
        self.inner.user_data = user_data;
    }

    /// Mark the submission as using `IOSQE_BUFFER_SELECT`.
    pub(crate) fn set_buffer_select(&mut self, buf_group: u16) {
        self.inner.__bindgen_anon_4.buf_group = buf_group;
        self.inner.flags |= libc::IOSQE_BUFFER_SELECT;
    }

    /// Don't return a completion event for this submission.
    pub(crate) fn no_completion_event(&mut self) {
        self.inner.flags |= libc::IOSQE_CQE_SKIP_SUCCESS;
    }

    /// Returns `true` if the submission is unchanged after a [`reset`].
    ///
    /// [`reset`]: Submission::reset
    #[cfg(debug_assertions)]
    pub(crate) const fn is_unchanged(&self) -> bool {
        self.inner.opcode == libc::IORING_OP_NOP as u8
    }

    /// Sync the `fd` with `fsync_flags`.
    pub(crate) unsafe fn fsync(&mut self, fd: RawFd, fsync_flags: libc::__u32) {
        self.inner.opcode = libc::IORING_OP_FSYNC as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { fsync_flags };
    }

    /// Create a read submission starting at `offset`.
    ///
    /// Avaialable since Linux kernel 5.6.
    pub(crate) unsafe fn read_at(&mut self, fd: RawFd, ptr: *mut u8, len: u32, offset: u64) {
        self.inner.opcode = libc::IORING_OP_READ as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
        self.inner.len = len;
    }

    /// Create a read vectored submission starting at `offset`.
    #[allow(clippy::needless_pass_by_ref_mut)] // Needed for `bufs`.
    pub(crate) unsafe fn read_vectored_at(
        &mut self,
        fd: RawFd,
        bufs: &mut [libc::iovec],
        offset: u64,
    ) {
        self.inner.opcode = libc::IORING_OP_READV as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: bufs.as_ptr() as _,
        };
        self.inner.len = bufs.len() as _;
    }

    /// Create a write submission starting at `offset`.
    ///
    /// Avaialable since Linux kernel 5.6.
    pub(crate) unsafe fn write_at(&mut self, fd: RawFd, ptr: *const u8, len: u32, offset: u64) {
        self.inner.opcode = libc::IORING_OP_WRITE as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as u64 };
        self.inner.len = len;
    }

    /// Create a write vectored submission starting at `offset`.
    pub(crate) unsafe fn write_vectored_at(
        &mut self,
        fd: RawFd,
        bufs: &[libc::iovec],
        offset: u64,
    ) {
        self.inner.opcode = libc::IORING_OP_WRITEV as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: bufs.as_ptr() as _,
        };
        self.inner.len = bufs.len() as _;
    }

    pub(crate) unsafe fn mkdirat(
        &mut self,
        dirfd: RawFd,
        path: *const libc::c_char,
        mode: libc::mode_t,
    ) {
        self.inner.opcode = libc::IORING_OP_MKDIRAT as u8;
        self.inner.fd = dirfd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: path as _ };
        self.inner.len = mode;
    }

    pub(crate) unsafe fn rename(
        &mut self,
        old_fd: RawFd,
        old_path: *const libc::c_char,
        new_fd: RawFd,
        new_path: *const libc::c_char,
        flags: libc::c_int,
    ) {
        self.inner.opcode = libc::IORING_OP_RENAMEAT as u8;
        self.inner.fd = old_fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: new_path as _ };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: old_path as _,
        };
        self.inner.len = new_fd as _;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            rename_flags: flags as _,
        };
    }

    pub(crate) unsafe fn unlinkat(
        &mut self,
        dirfd: RawFd,
        path: *const libc::c_char,
        flags: libc::c_int,
    ) {
        self.inner.opcode = libc::IORING_OP_UNLINKAT as u8;
        self.inner.fd = dirfd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: path as _ };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            unlink_flags: flags as _,
        };
    }

    pub(crate) unsafe fn socket(
        &mut self,
        domain: libc::c_int,
        r#type: libc::c_int,
        protocol: libc::c_int,
        flags: libc::c_int,
    ) {
        self.inner.opcode = libc::IORING_OP_SOCKET as u8;
        self.inner.fd = domain;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: r#type as _ };
        self.inner.len = protocol as _;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { rw_flags: flags };
    }

    pub(crate) unsafe fn connect(
        &mut self,
        fd: RawFd,
        address: *const libc::sockaddr,
        address_length: libc::socklen_t,
    ) {
        self.inner.opcode = libc::IORING_OP_CONNECT as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: u64::from(address_length),
        };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: address as _ };
    }

    /// `opcode` must be `IORING_OP_SEND` or `IORING_OP_SEND_ZC`.
    pub(crate) unsafe fn send(
        &mut self,
        opcode: u8,
        fd: RawFd,
        ptr: *const u8,
        len: u32,
        flags: libc::c_int,
    ) {
        debug_assert!(
            opcode == libc::IORING_OP_SEND as u8 || opcode == libc::IORING_OP_SEND_ZC as u8
        );
        self.inner.opcode = opcode;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as u64 };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: flags as _,
        };
        self.inner.len = len;
    }

    /// `opcode` must be `IORING_OP_SEND` or `IORING_OP_SEND_ZC`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn sendto(
        &mut self,
        opcode: u8,
        fd: RawFd,
        buf_ptr: *const u8,
        buf_len: u32,
        address: *const libc::sockaddr,
        address_length: libc::socklen_t,
        flags: libc::c_int,
    ) {
        self.send(opcode, fd, buf_ptr, buf_len, flags);
        self.inner.__bindgen_anon_1.addr2 = address as _;
        self.inner.__bindgen_anon_5.__bindgen_anon_1.addr_len = address_length as _;
    }

    /// `opcode` must be `IORING_OP_SENDMSG` or `IORING_OP_SENDMSG_ZC`.
    pub(crate) unsafe fn sendmsg(
        &mut self,
        opcode: u8,
        fd: RawFd,
        msg: *const libc::msghdr,
        flags: libc::c_int,
    ) {
        debug_assert!(
            opcode == libc::IORING_OP_SENDMSG as u8 || opcode == libc::IORING_OP_SENDMSG_ZC as u8
        );
        self.inner.opcode = opcode;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: msg as u64 };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: flags as _,
        };
        self.inner.len = 1;
    }

    pub(crate) unsafe fn recv(&mut self, fd: RawFd, ptr: *mut u8, len: u32, flags: libc::c_int) {
        self.inner.opcode = libc::IORING_OP_RECV as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: flags as _,
        };
        self.inner.len = len;
    }

    pub(crate) unsafe fn multishot_recv(&mut self, fd: RawFd, flags: libc::c_int, buf_group: u16) {
        self.inner.opcode = libc::IORING_OP_RECV as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: flags as _,
        };
        self.inner.ioprio = libc::IORING_RECV_MULTISHOT as _;
        self.set_buffer_select(buf_group);
    }

    pub(crate) unsafe fn recvmsg(
        &mut self,
        fd: RawFd,
        msg: *const libc::msghdr,
        flags: libc::c_int,
    ) {
        self.inner.opcode = libc::IORING_OP_RECVMSG as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: msg as u64 };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: flags as _,
        };
        self.inner.len = 1;
    }

    pub(crate) unsafe fn shutdown(&mut self, fd: RawFd, how: libc::c_int) {
        self.inner.opcode = libc::IORING_OP_SHUTDOWN as u8;
        self.inner.fd = fd;
        self.inner.len = how as u32;
    }

    /// Create a accept submission starting.
    ///
    /// Avaialable since Linux kernel 5.5.
    pub(crate) unsafe fn accept(
        &mut self,
        fd: RawFd,
        address: *mut libc::sockaddr,
        address_length: *mut libc::socklen_t,
        flags: libc::c_int,
    ) {
        self.inner.opcode = libc::IORING_OP_ACCEPT as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: address_length as _,
        };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: address as _ };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            accept_flags: flags as _,
        };
    }

    pub(crate) unsafe fn multishot_accept(&mut self, fd: RawFd, flags: libc::c_int) {
        self.inner.opcode = libc::IORING_OP_ACCEPT as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            accept_flags: flags as _,
        };
        self.inner.ioprio = libc::IORING_ACCEPT_MULTISHOT as _;
    }

    /// Attempt to cancel an already issued request.
    ///
    /// Avaialable since Linux kernel 5.5.
    pub(crate) unsafe fn cancel(&mut self, fd: RawFd, flags: u32) {
        self.inner.opcode = libc::IORING_OP_ASYNC_CANCEL as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            cancel_flags: flags | libc::IORING_ASYNC_CANCEL_FD,
        };
    }

    pub(crate) unsafe fn cancel_op(&mut self, op_index: OpIndex) {
        self.inner.opcode = libc::IORING_OP_ASYNC_CANCEL as u8;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: op_index.0 as u64,
        };
    }

    /// Open a file by `pathname` in directory `dir_fd`.
    pub(crate) unsafe fn open_at(
        &mut self,
        dir_fd: RawFd,
        pathname: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) {
        self.inner.opcode = libc::IORING_OP_OPENAT as u8;
        self.inner.fd = dir_fd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: pathname as _,
        };
        self.inner.len = mode;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            open_flags: flags as _,
        };
    }

    pub(crate) unsafe fn splice(
        &mut self,
        fd_in: RawFd,
        off_in: u64,
        fd_out: RawFd,
        off_out: u64,
        len: u32,
        flags: libc::c_int,
    ) {
        self.inner.opcode = libc::IORING_OP_SPLICE as u8;
        self.inner.fd = fd_out;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: off_out };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            splice_off_in: off_in,
        };
        self.inner.len = len;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            splice_flags: flags as u32,
        };
        self.inner.__bindgen_anon_5 = libc::io_uring_sqe__bindgen_ty_5 {
            splice_fd_in: fd_in,
        };
    }

    /// Close the `fd`.
    pub(crate) unsafe fn close(&mut self, fd: RawFd) {
        self.inner.opcode = libc::IORING_OP_CLOSE as u8;
        self.inner.fd = fd;
    }

    /// Call `statx(2)` on `fd`, where `fd` points to a file.
    ///
    /// Avaialable since Linux kernel 5.6.
    pub(crate) unsafe fn statx_file(&mut self, fd: RawFd, statx: &mut libc::statx, flags: u32) {
        self.inner.opcode = libc::IORING_OP_STATX as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: statx as *mut _ as _,
        };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: "\0".as_ptr() as _, // Not using a path.
        };
        self.inner.len = flags;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            statx_flags: libc::AT_EMPTY_PATH as _,
        };
    }

    pub(crate) unsafe fn fadvise(&mut self, fd: RawFd, offset: u64, len: u32, advise: libc::c_int) {
        self.inner.opcode = libc::IORING_OP_FADVISE as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.len = len;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            fadvise_advice: advise as _,
        };
    }

    pub(crate) unsafe fn fallocate(&mut self, fd: RawFd, offset: u64, len: u32, mode: libc::c_int) {
        self.inner.opcode = libc::IORING_OP_FALLOCATE as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: u64::from(len),
        };
        self.inner.len = mode as u32;
    }

    pub(crate) unsafe fn poll(&mut self, fd: RawFd, mask: u32) {
        self.inner.opcode = libc::IORING_OP_POLL_ADD as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            poll32_events: mask,
        };
    }

    pub(crate) unsafe fn remove_poll(&mut self, user_data: OpIndex) {
        self.inner.opcode = libc::IORING_OP_POLL_REMOVE as u8;
        self.inner.fd = -1;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: user_data.0 as _,
        };
    }

    pub(crate) unsafe fn multishot_poll(&mut self, fd: RawFd, mask: u32) {
        self.poll(fd, mask);
        self.inner.len = libc::IORING_POLL_ADD_MULTI;
    }

    pub(crate) unsafe fn madvise(&mut self, address: *mut (), len: u32, advice: libc::c_int) {
        self.inner.opcode = libc::IORING_OP_MADVISE as u8;
        self.inner.fd = -1;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: address as _ };
        self.inner.len = len;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            fadvise_advice: advice as _,
        };
    }

    pub(crate) unsafe fn wake(&mut self, ring_fd: RawFd) {
        self.msg(ring_fd, u64::MAX, 0, 0);
        self.no_completion_event();
    }

    /// Note that the argument `user_data` and `res` names are the same as the
    /// field names on the completion event.
    // TODO: we can add another ~`u32` by setting `file_index` (returned as
    // `flags` on the completion) and using `IORING_MSG_RING_FLAGS_PASS`, should
    // be available in 6.3.
    pub(crate) unsafe fn msg(
        &mut self,
        ring_fd: RawFd,
        user_data: u64,
        res: u32,
        msg_ring_flags: u32,
    ) {
        self.inner.opcode = libc::IORING_OP_MSG_RING as u8;
        self.inner.fd = ring_fd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: u64::from(libc::IORING_MSG_DATA),
        };
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: user_data };
        self.inner.len = res;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { msg_ring_flags };
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

        let mut f = f.debug_struct("Submission");
        match u32::from(self.inner.opcode) {
            libc::IORING_OP_NOP => {
                f.field("opcode", &"IORING_OP_NOP");
            }
            libc::IORING_OP_FSYNC => {
                f.field("opcode", &"IORING_OP_FSYNC")
                    .field("fd", &self.inner.fd)
                    .field("fsync_flags", unsafe {
                        &self.inner.__bindgen_anon_3.fsync_flags
                    });
            }
            libc::IORING_OP_READ => io_op(&mut f, &self.inner, "IORING_OP_READ"),
            libc::IORING_OP_READV => io_op(&mut f, &self.inner, "IORING_OP_READV"),
            libc::IORING_OP_WRITE => io_op(&mut f, &self.inner, "IORING_OP_WRITE"),
            libc::IORING_OP_WRITEV => io_op(&mut f, &self.inner, "IORING_OP_WRITEV"),
            libc::IORING_OP_RENAMEAT => {
                f.field("opcode", &"IORING_OP_RENAMEAT")
                    .field("old_fd", &self.inner.fd)
                    .field("old_path", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("new_fd", &self.inner.len)
                    .field("new_path", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("rename_flags", unsafe {
                        &self.inner.__bindgen_anon_3.rename_flags
                    });
            }
            libc::IORING_OP_SOCKET => {
                f.field("opcode", &"IORING_OP_SOCKET")
                    .field("domain", &self.inner.fd)
                    .field("type", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("protocol", &self.inner.len)
                    .field("rw_flags", unsafe { &self.inner.__bindgen_anon_3.rw_flags });
            }
            libc::IORING_OP_CONNECT => {
                f.field("opcode", &"IORING_OP_CONNECT")
                    .field("fd", &self.inner.fd)
                    .field("addr", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("addr_size", unsafe { &self.inner.__bindgen_anon_1.off });
            }
            libc::IORING_OP_SEND => net_op(&mut f, &self.inner, "IORING_OP_SEND"),
            libc::IORING_OP_SEND_ZC => net_op(&mut f, &self.inner, "IORING_OP_SEND_ZC"),
            libc::IORING_OP_SENDMSG => net_op(&mut f, &self.inner, "IORING_OP_SENDMSG"),
            libc::IORING_OP_SENDMSG_ZC => net_op(&mut f, &self.inner, "IORING_OP_SENDMSG_ZC"),
            libc::IORING_OP_RECV => net_op(&mut f, &self.inner, "IORING_OP_RECV"),
            libc::IORING_OP_RECVMSG => net_op(&mut f, &self.inner, "IORING_OP_RECVMSG"),
            libc::IORING_OP_SHUTDOWN => {
                f.field("opcode", &"IORING_OP_SHUTDOWN")
                    .field("fd", &self.inner.fd)
                    .field("how", &self.inner.len);
            }
            libc::IORING_OP_ACCEPT => {
                f.field("opcode", &"IORING_OP_ACCEPT")
                    .field("fd", &self.inner.fd)
                    .field("addr", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("addr_size", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("accept_flags", unsafe {
                        &self.inner.__bindgen_anon_3.accept_flags
                    })
                    .field("ioprio", &self.inner.ioprio);
            }
            libc::IORING_OP_ASYNC_CANCEL => {
                f.field("opcode", &"IORING_OP_ASYNC_CANCEL");
                let cancel_flags = unsafe { self.inner.__bindgen_anon_3.cancel_flags };
                #[allow(clippy::if_not_else)]
                if (cancel_flags & libc::IORING_ASYNC_CANCEL_FD) != 0 {
                    f.field("fd", &self.inner.fd)
                        .field("cancel_flags", &cancel_flags);
                } else {
                    f.field("addr", unsafe { &self.inner.__bindgen_anon_2.addr });
                }
            }
            libc::IORING_OP_OPENAT => {
                f.field("opcode", &"IORING_OP_OPENAT")
                    .field("dirfd", &self.inner.fd)
                    .field("pathname", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("mode", &self.inner.len)
                    .field("open_flags", unsafe {
                        &self.inner.__bindgen_anon_3.open_flags
                    });
            }
            libc::IORING_OP_SPLICE => {
                f.field("opcode", &"IORING_OP_SPLICE")
                    .field("fd_in", unsafe {
                        &self.inner.__bindgen_anon_5.splice_fd_in
                    })
                    .field("off_in", unsafe {
                        &self.inner.__bindgen_anon_2.splice_off_in
                    })
                    .field("fd_out", &self.inner.fd)
                    .field("off_out", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("len", &self.inner.len)
                    .field("splice_flags", unsafe {
                        &self.inner.__bindgen_anon_3.splice_flags
                    });
            }
            libc::IORING_OP_CLOSE => {
                f.field("opcode", &"IORING_OP_CLOSE")
                    .field("fd", &self.inner.fd);
            }
            libc::IORING_OP_STATX => {
                f.field("opcode", &"IORING_OP_STATX")
                    .field("fd", &self.inner.fd)
                    .field("pathname", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("statx_flags", unsafe {
                        &self.inner.__bindgen_anon_3.statx_flags
                    })
                    .field("mask", &self.inner.len)
                    .field("statx", unsafe { &self.inner.__bindgen_anon_1.off });
            }
            libc::IORING_OP_FADVISE => {
                f.field("opcode", &"IORING_OP_FADVISE")
                    .field("fd", &self.inner.fd)
                    .field("offset", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("len", &self.inner.len)
                    .field("advise", unsafe {
                        &self.inner.__bindgen_anon_3.fadvise_advice
                    });
            }
            libc::IORING_OP_FALLOCATE => {
                f.field("opcode", &"IORING_OP_FALLOCATE")
                    .field("fd", &self.inner.fd)
                    .field("offset", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("len", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("mode", &self.inner.len);
            }
            libc::IORING_OP_UNLINKAT => {
                f.field("opcode", &"IORING_OP_UNLINKAT")
                    .field("dirfd", &self.inner.fd)
                    .field("path", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("unlink_flags", unsafe {
                        &self.inner.__bindgen_anon_3.unlink_flags
                    });
            }
            libc::IORING_OP_MKDIRAT => {
                f.field("opcode", &"IORING_OP_MKDIRAT")
                    .field("dirfd", &self.inner.fd)
                    .field("path", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("mode", &self.inner.len);
            }
            libc::IORING_OP_POLL_ADD => {
                f.field("opcode", &"IORING_OP_POLL_ADD")
                    .field("fd", &self.inner.fd)
                    .field("poll_events", unsafe {
                        &self.inner.__bindgen_anon_3.poll32_events
                    })
                    .field(
                        "multishot",
                        &(self.inner.len == libc::IORING_POLL_ADD_MULTI),
                    );
            }
            libc::IORING_OP_POLL_REMOVE => {
                f.field("opcode", &"IORING_OP_POLL_REMOVE")
                    .field("target_user_data", unsafe {
                        &self.inner.__bindgen_anon_2.addr
                    });
            }
            libc::IORING_OP_MADVISE => {
                f.field("opcode", &"IORING_OP_MADVISE")
                    .field("address", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("len", &self.inner.len)
                    .field("advise", unsafe {
                        &self.inner.__bindgen_anon_3.fadvise_advice
                    });
            }
            libc::IORING_OP_MSG_RING => {
                f.field("opcode", &"IORING_OP_MSG_RING")
                    .field("ringfd", &self.inner.fd)
                    .field("msg1", &self.inner.len)
                    .field("msg2", unsafe { &self.inner.__bindgen_anon_1.off });
            }
            _ => {
                // NOTE: we can't access the unions safely without know what
                // fields to read.
                f.field("opcode", &self.inner.opcode)
                    .field("ioprio", &self.inner.ioprio)
                    .field("fd", &self.inner.fd)
                    .field("len", &self.inner.len)
                    .field("personality", &self.inner.personality);
            }
        }
        f.field("flags", &self.inner.flags)
            .field("user_data", &self.inner.user_data)
            .finish()
    }
}

/// Macro to create an operation [`Future`] structure.
///
/// [`Future`]: std::future::Future
macro_rules! op_future {
    (
        // File type and function name.
        fn $type: ident :: $method: ident -> $result: ty,
        // Future structure.
        struct $name: ident < $lifetime: lifetime $(, $generic: ident $(: $trait: path )? )* $(; const $const_generic: ident : $const_ty: ty )* > {
            $(
            // Field(s) passed to io_uring, always wrapped in an `Option`.
            // Syntax is the same a struct definition.
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty,
            )*
        },
        // Whether or not the structure should be `!Unpin` by including
        // `PhantomPinned`.
        $(
            $(#[ $phantom_doc: meta ])*
            impl !Upin,
        )?
        // State held in the setup function.
        setup_state: $setup_field: ident : $setup_ty: ty,
        // Function to setup the operation.
        setup: |$setup_submission: ident, $setup_fd: ident, $setup_resources: tt, $setup_state: tt| $setup_fn: expr,
        // Mapping function that maps the returned `$arg`ument into
        // `$map_result`. The `$resources` is a tuple of the `$field`s on the
        // future.
        map_result: |$self: ident, $resources: tt, $flags: ident, $map_arg: ident| $map_result: expr,
        // Mapping function for `Extractor` implementation. See above.
        extract: |$extract_self: ident, $extract_resources: tt, $extract_flags: ident, $extract_arg: ident| -> $extract_result: ty $extract_map: block,
    ) => {
        $crate::op::op_future!{
            fn $type::$method -> $result,
            struct $name<$lifetime $(, $generic $(: $trait )? )* $(; const $const_generic: $const_ty )*> {
                $(
                $(#[$field_doc])*
                $field: $value,
                )*
            },
            $(
                $(#[ $phantom_doc ])*
                impl !Upin,
            )?
            setup_state: $setup_field : $setup_ty,
            setup: |$setup_submission, $setup_fd, $setup_resources, $setup_state| $setup_fn,
            map_result: |$self, $resources, $flags, $map_arg| $map_result,
        }

        impl<$lifetime $(, $generic $(: $trait )? )* $(, const $const_generic: $const_ty )*> $crate::Extract for $name<$lifetime $(, $generic)* $(, $const_generic )*> {}

        impl<$lifetime $(, $generic $(: $trait )? )* $(, const $const_generic: $const_ty )*> std::future::Future for $crate::extract::Extractor<$name<$lifetime $(, $generic)* $(, $const_generic )*>> {
            type Output = std::io::Result<$extract_result>;

            fn poll(self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
                // SAFETY: we're not moving anything out of `self.
                let $self = unsafe { std::pin::Pin::into_inner_unchecked(self) };
                let op_index = std::task::ready!($self.fut.poll_op_index(ctx));

                match $self.fut.fd.sq.poll_op(ctx, op_index) {
                    std::task::Poll::Ready(result) => {
                        $self.fut.state = $crate::op::OpState::Done;
                        match result {
                            std::result::Result::Ok(($extract_flags, $extract_arg)) => {
                                let $extract_self = &mut $self.fut;
                                // SAFETY: this will not panic because we need
                                // to keep the resources around until the
                                // operation is completed.
                                let $extract_resources = $extract_self.resources.take().unwrap().into_inner();
                                std::task::Poll::Ready($extract_map)
                            },
                            std::result::Result::Err(err) => std::task::Poll::Ready(std::result::Result::Err(err)),
                        }
                    },
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }
        }
    };
    // Base version (without any additional implementations).
    (
        fn $type: ident :: $method: ident -> $result: ty,
        struct $name: ident < $lifetime: lifetime $(, $generic: ident $(: $trait: path )? )* $(; const $const_generic: ident : $const_ty: ty )* > {
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty,
            )*
        },
        $(
            $(#[ $phantom_doc: meta ])*
            impl !Upin,
        )?
        setup_state: $setup_field: ident : $setup_ty: ty,
        setup: |$setup_submission: ident, $setup_fd: ident, $setup_resources: tt, $setup_state: tt| $setup_fn: expr,
        map_result: |$self: ident, $resources: tt, $flags: ident, $map_arg: ident| $map_result: expr,
    ) => {
        #[doc = concat!("[`Future`](std::future::Future) behind [`", stringify!($type), "::", stringify!($method), "`].")]
        #[derive(Debug)]
        #[must_use = "`Future`s do nothing unless polled"]
        pub struct $name<$lifetime $(, $generic)* $(, const $const_generic: $const_ty )*> {
            /// Resoures used in the operation.
            ///
            /// If this is `Some` when the future is dropped it will assume it
            /// was dropped before completion and set the operation state to
            /// dropped.
            resources: std::option::Option<std::cell::UnsafeCell<(
                $( $value, )*
            )>>,
            /// File descriptor used in the operation.
            fd: &$lifetime $crate::AsyncFd,
            /// State of the operation.
            state: $crate::op::OpState<$setup_ty>,
            $(
                $( #[ $phantom_doc ] )*
                _phantom: std::marker::PhantomPinned,
            )?
        }

        impl<$lifetime $(, $generic )* $(, const $const_generic: $const_ty )*> $name<$lifetime $(, $generic)* $(, $const_generic )*> {
            #[doc = concat!("Create a new `", stringify!($name), "`.")]
            const fn new(fd: &$lifetime $crate::AsyncFd, $( $field: $value, )* $setup_field : $setup_ty) -> $name<$lifetime $(, $generic)* $(, $const_generic )*> {
                // This is needed because of the usage of `$phantom_doc`, which
                // is needed for the macro to work, even though it doesn't
                // create any documentation.
                #[allow(unused_doc_comments)]
                $name {
                    resources: std::option::Option::Some(std::cell::UnsafeCell::new((
                        $( $field, )*
                    ))),
                    fd,
                    state: $crate::op::OpState::NotStarted($setup_field),
                    $(
                        $( #[ $phantom_doc ] )*
                        _phantom: std::marker::PhantomPinned,
                    )?
                }
            }
        }

        impl<$lifetime $(, $generic )* $(, const $const_generic: $const_ty )*> $crate::cancel::Cancel for $name<$lifetime $(, $generic)* $(, $const_generic )*> {
            fn try_cancel(&mut self) -> $crate::cancel::CancelResult {
                self.state.try_cancel(&self.fd.sq)
            }

            fn cancel(&mut self) -> $crate::cancel::CancelOp {
                self.state.cancel(&self.fd.sq)
            }
        }

        impl<$lifetime $(, $generic $(: $trait )? )* $(, const $const_generic: $const_ty )*> $name<$lifetime $(, $generic)* $(, $const_generic )*> {
            /// Poll for the `OpIndex`.
            fn poll_op_index(&mut self, ctx: &mut std::task::Context<'_>) -> std::task::Poll<$crate::OpIndex> {
                std::task::Poll::Ready($crate::op::poll_state!($name, *self, ctx, |$setup_submission, $setup_fd, $setup_resources, $setup_state| {
                    $setup_fn
                }))
            }
        }

        impl<$lifetime $(, $generic $(: $trait )? )* $(, const $const_generic: $const_ty )*> std::future::Future for $name<$lifetime $(, $generic)* $(, $const_generic )*> {
            type Output = std::io::Result<$result>;

            fn poll(self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
                // SAFETY: we're not moving anything out of `self.
                let $self = unsafe { std::pin::Pin::into_inner_unchecked(self) };
                let op_index = std::task::ready!($self.poll_op_index(ctx));

                match $self.fd.sq.poll_op(ctx, op_index) {
                    std::task::Poll::Ready(result) => {
                        $self.state = $crate::op::OpState::Done;
                        match result {
                            std::result::Result::Ok(($flags, $map_arg)) => {
                                // SAFETY: this will not panic because we need
                                // to keep the resources around until the
                                // operation is completed.
                                let $resources = $self.resources.take().unwrap().into_inner();
                                std::task::Poll::Ready($map_result)
                            },
                            std::result::Result::Err(err) => std::task::Poll::Ready(std::result::Result::Err(err)),
                        }
                    },
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }
        }

        unsafe impl<$lifetime $(, $generic: std::marker::Send )* $(, const $const_generic: $const_ty )*> std::marker::Send for $name<$lifetime $(, $generic)* $(, $const_generic )*> {}
        unsafe impl<$lifetime $(, $generic: std::marker::Sync )* $(, const $const_generic: $const_ty )*> std::marker::Sync for $name<$lifetime $(, $generic)* $(, $const_generic )*> {}

        impl<$lifetime $(, $generic)* $(, const $const_generic: $const_ty )*> std::ops::Drop for $name<$lifetime $(, $generic)* $(, $const_generic )*> {
            fn drop(&mut self) {
                if let std::option::Option::Some(resources) = self.resources.take() {
                    match self.state {
                        $crate::op::OpState::Running(op_index) => self.fd.sq.drop_op(op_index, resources),
                        // NOTE: `Done` should not be reachable, but no point in
                        // creating another branch.
                        #[allow(clippy::drop_non_drop)]
                        $crate::op::OpState::NotStarted(_) | $crate::op::OpState::Done => drop(resources),
                    }
                }
            }
        }
    };
    // Version that doesn't need the `flags` from the result in `$map_result`.
    (
        fn $type: ident :: $method: ident -> $result: ty,
        struct $name: ident < $lifetime: lifetime $(, $generic: ident $(: $trait: path )? )* $(; const $const_generic: ident : $const_ty: ty )* > {
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty,
            )*
        },
        $(
            $(#[ $phantom_doc: meta ])*
            impl !Upin,
        )?
        setup_state: $setup_data: ident : $setup_ty: ty,
        setup: |$setup_submission: ident, $setup_fd: ident, $setup_resources: tt, $setup_state: tt| $setup_fn: expr,
        map_result: |$self: ident, $resources: tt, $map_arg: ident| $map_result: expr,
        $( extract: |$extract_self: ident, $extract_resources: tt, $extract_flags: ident, $extract_arg: ident| -> $extract_result: ty $extract_map: block, )?
    ) => {
        $crate::op::op_future!{
            fn $type::$method -> $result,
            struct $name<$lifetime $(, $generic $(: $trait )? )* $(; const $const_generic: $const_ty )*> {
                $(
                $(#[$field_doc])*
                $field: $value,
                )*
            },
            $(
                $(#[ $phantom_doc ])*
                impl !Upin,
            )?
            setup_state: $setup_data: $setup_ty,
            setup: |$setup_submission, $setup_fd, $setup_resources, $setup_state| $setup_fn,
            map_result: |$self, $resources, _unused_flags, $map_arg| $map_result,
            $( extract: |$extract_self, $extract_resources, _unused_flags, $extract_arg| -> $extract_result $extract_map, )?
        }
    };
    // Version that doesn't need `self` (this) or resources in `$map_result`.
    (
        fn $type: ident :: $method: ident -> $result: ty,
        struct $name: ident < $lifetime: lifetime $(, $generic: ident $(: $trait: path )? )* $(; const $const_generic: ident : $const_ty: ty )* > {
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty,
            )*
        },
        $(
            $(#[ $phantom_doc: meta ])*
            impl !Upin,
        )?
        setup_state: $setup_field: ident : $setup_ty: ty,
        setup: |$setup_submission: ident, $setup_fd: ident, $setup_resources: tt, $setup_state: tt| $setup_fn: expr,
        map_result: |$map_arg: ident| $map_result: expr, // Only difference: 1 argument.
        $( extract: |$extract_self: ident, $extract_resources: tt, $extract_arg: ident| -> $extract_result: ty $extract_map: block, )?
    ) => {
        $crate::op::op_future!{
            fn $type::$method -> $result,
            struct $name<$lifetime $(, $generic $(: $trait )? )* $(; const $const_generic: $const_ty )*> {
                $(
                $(#[$field_doc])*
                $field: $value,
                )*
            },
            $(
                $(#[ $phantom_doc ])*
                impl !Upin,
            )?
            setup_state: $setup_field : $setup_ty,
            setup: |$setup_submission, $setup_fd, $setup_resources, $setup_state| $setup_fn,
            map_result: |_unused_this, _unused_resources, _unused_flags, $map_arg| $map_result,
            $( extract: |$extract_self, $extract_resources, _unused_flags, $extract_arg| -> $extract_result $extract_map, )?
        }
    };
}

pub(crate) use op_future;

/// State of an [`op_future!`] [`Future`] or [`op_async_iter!`]
/// [`AsyncIterator`].
///
/// [`Future`]: std::future::Future
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
#[derive(Debug)]
pub(crate) enum OpState<S> {
    /// The operation has not started yet.
    NotStarted(S),
    /// Operation is running, waiting for the (next) result.
    Running(OpIndex),
    /// Operation is done.
    Done,
}

impl<S> OpState<S> {
    pub(crate) fn try_cancel(&mut self, sq: &SubmissionQueue) -> CancelResult {
        match self {
            OpState::NotStarted(_) => CancelResult::NotStarted,
            OpState::Running(op_index) => {
                let result = sq.add_no_result(|submission| unsafe {
                    submission.cancel_op(*op_index);
                    submission.no_completion_event();
                });
                match result {
                    Ok(()) => CancelResult::Canceled,
                    Err(QueueFull(())) => CancelResult::QueueFull,
                }
            }
            OpState::Done => CancelResult::Canceled,
        }
    }

    pub(crate) fn cancel<'a>(&mut self, sq: &'a SubmissionQueue) -> CancelOp<'a> {
        let op_index = match self {
            OpState::Running(op_index) => Some(*op_index),
            OpState::NotStarted(_) | OpState::Done => None,
        };
        CancelOp::new(sq, op_index)
    }
}

/// Poll an [`OpState`].
macro_rules! poll_state {
    // Variant used by `op_future!`.
    (
        $name: ident, $self: expr, $ctx: expr,
        |$setup_submission: ident, $setup_fd: ident, $setup_resources: tt, $setup_state: tt| $setup_fn: expr $(,)?
    ) => {
        match $self.state {
            $crate::op::OpState::Running(op_index) => op_index,
            $crate::op::OpState::NotStarted($setup_state) => {
                let $name {
                    fd: $setup_fd,
                    resources,
                    ..
                } = &mut $self;
                // SAFETY: this will not panic as the resources are only removed
                // after the state is set to `Done`.
                #[allow(clippy::let_unit_value)]
                let $setup_resources = resources.as_mut().take().unwrap().get_mut();
                let result = $setup_fd.sq.add(|$setup_submission| $setup_fn);
                match result {
                    Ok(op_index) => {
                        $self.state = $crate::op::OpState::Running(op_index);
                        op_index
                    }
                    Err($crate::QueueFull(())) => {
                        $self.fd.sq.wait_for_submission($ctx.waker().clone());
                        return std::task::Poll::Pending;
                    }
                }
            }
            $crate::op::OpState::Done => $crate::op::poll_state!(__panic $name),
        }
    };
    // Without `$setup_resources`, but expects `$self.fd` to be `AsyncFd`.
    (
        $name: ident, $self: expr, $ctx: expr,
        |$setup_submission: ident, $setup_fd: ident, $setup_state: tt| $setup_fn: expr $(,)?
    ) => {
        match $self.state {
            $crate::op::OpState::Running(op_index) => op_index,
            $crate::op::OpState::NotStarted($setup_state) => {
                let $setup_fd = $self.fd;
                let result = $self.fd.sq.add(|$setup_submission| $setup_fn);
                match result {
                    Ok(op_index) => {
                        $self.state = $crate::op::OpState::Running(op_index);
                        op_index
                    }
                    Err($crate::QueueFull(())) => {
                        $self.fd.sq.wait_for_submission($ctx.waker().clone());
                        return std::task::Poll::Pending;
                    }
                }
            }
            $crate::op::OpState::Done => $crate::op::poll_state!(__panic $name),
        }
    };
    // No `AsyncFd` or `$setup_resources`.
    // NOTE: this doesn't take `$self`, but `$state` and `$sq`.
    (
        $name: ident, $state: expr, $sq: expr, $ctx: expr,
        |$setup_submission: ident, $setup_state: tt| $setup_fn: expr $(,)?
    ) => {
        match $state {
            $crate::op::OpState::Running(op_index) => op_index,
            $crate::op::OpState::NotStarted($setup_state) => {
                let result = $sq.add(|$setup_submission| $setup_fn);
                match result {
                    Ok(op_index) => {
                        $state = $crate::op::OpState::Running(op_index);
                        op_index
                    }
                    Err($crate::QueueFull(())) => {
                        $sq.wait_for_submission($ctx.waker().clone());
                        return std::task::Poll::Pending;
                    }
                }
            }
            $crate::op::OpState::Done => $crate::op::poll_state!(__panic $name),
        }
    };
    (__panic $name: ident) => {
        unreachable!(concat!("a10::", stringify!($name), " polled after completion"))
    }
}

pub(crate) use poll_state;

/// Macro to create an operation [`AsyncIterator`] structure based on multishot
/// operations.
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
macro_rules! op_async_iter {
    (
        fn $type: ident :: $method: ident -> $result: ty,
        struct $name: ident < $lifetime: lifetime > {
            // Additional and optional state field. The lifetime of this field
            // MUST NOT connected to the operation, in other words this must be
            // able to be dropped before the operation completes.
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty,
            )?
        },
        setup_state: $state: ident : $setup_ty: ty,
        setup: |$setup_submission: ident, $setup_self: ident, $setup_state: tt| $setup_fn: expr,
        map_result: |$self: ident, $map_flags: ident, $map_arg: ident| $map_result: expr,
    ) => {
        #[doc = concat!("[`AsyncIterator`](std::async_iter::AsyncIterator) behind [`", stringify!($type), "::", stringify!($method), "`].")]
        #[derive(Debug)]
        #[must_use = "`AsyncIterator`s do nothing unless polled"]
        pub struct $name<$lifetime> {
            /// File descriptor used in the operation.
            fd: &$lifetime $crate::AsyncFd,
            $(
            $(#[ $field_doc ])*
            $field: $value,
            )?
            /// State of the operation.
            state: $crate::op::OpState<$setup_ty>,
        }

        impl<$lifetime> $name<$lifetime> {
            #[doc = concat!("Create a new `", stringify!($name), "`.")]
            const fn new(fd: &$lifetime $crate::AsyncFd, $($field: $value, )? $state : $setup_ty) -> $name<$lifetime> {
                $name {
                    fd,
                    $( $field, )?
                    state: $crate::op::OpState::NotStarted($state),
                }
            }

            /// This is the same as the `AsyncIterator::poll_next` function, but
            /// available on stable Rust.
            pub fn poll_next(self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Option<std::io::Result<$result>>> {
                // SAFETY: we're not moving anything out of `self.
                let $self = unsafe { std::pin::Pin::into_inner_unchecked(self) };
                let op_index = match $self.state {
                    $crate::op::OpState::Running(op_index) => op_index,
                    $crate::op::OpState::NotStarted($setup_state) => {
                        let result = $self.fd.sq.add_multishot(|$setup_submission| {
                            let $setup_self = &mut *$self;
                            $setup_fn
                        });
                        match result {
                            Ok(op_index) => {
                                $self.state = $crate::op::OpState::Running(op_index);
                                op_index
                            }
                            Err($crate::QueueFull(())) => {
                                $self.fd.sq.wait_for_submission(ctx.waker().clone());
                                return std::task::Poll::Pending;
                            }
                        }
                    }
                    // We can reach this if we return an error, but not `None`
                    // yet.
                    $crate::op::OpState::Done => return std::task::Poll::Ready(std::option::Option::None),
                };

                match $self.fd.sq.poll_multishot_op(ctx, op_index) {
                    std::task::Poll::Ready(std::option::Option::Some(std::result::Result::Ok(($map_flags, $map_arg)))) => {
                        std::task::Poll::Ready(std::option::Option::Some(std::result::Result::Ok($map_result)))
                    },
                    std::task::Poll::Ready(std::option::Option::Some(std::result::Result::Err(err))) => {
                        // After an error we also don't expect any more results.
                        $self.state = $crate::op::OpState::Done;
                        if let Some(libc::ECANCELED) = err.raw_os_error() {
                            // Operation was canceled, so we expect no more
                            // results.
                            std::task::Poll::Ready(std::option::Option::None)
                        } else {
                            std::task::Poll::Ready(std::option::Option::Some(std::result::Result::Err(err)))
                        }
                    },
                    std::task::Poll::Ready(std::option::Option::None) => {
                        $self.state = $crate::op::OpState::Done;
                        std::task::Poll::Ready(std::option::Option::None)
                    },
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }
        }

        impl<$lifetime> $crate::cancel::Cancel for $name<$lifetime> {
            fn try_cancel(&mut self) -> $crate::cancel::CancelResult {
                self.state.try_cancel(&self.fd.sq)
            }

            fn cancel(&mut self) -> $crate::cancel::CancelOp {
                self.state.cancel(&self.fd.sq)
            }
        }

        #[cfg(feature = "nightly")]
        impl<$lifetime> std::async_iter::AsyncIterator for $name<$lifetime> {
            type Item = std::io::Result<$result>;

            fn poll_next(self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
                self.poll_next(ctx)
            }
        }

        impl<$lifetime> std::ops::Drop for $name<$lifetime> {
            fn drop(&mut self) {
                if let $crate::op::OpState::Running(op_index) = self.state {
                    let result = self.fd.sq.cancel_op(op_index, (), |submission| unsafe {
                        submission.cancel_op(op_index);
                        // We'll get a canceled completion event if we succeeded, which
                        // is sufficient to cleanup the operation.
                        submission.no_completion_event();
                    });
                    if let std::result::Result::Err(err) = result {
                        log::error!(concat!("dropped a10::", stringify!($name), " before canceling it, attempt to cancel failed: {}"), err);
                    }
                }
            }
        }
    };
}

pub(crate) use op_async_iter;

#[test]
fn size_assertion() {
    assert_eq!(std::mem::size_of::<CompletionResult>(), 8);
    // On nightly this is 24 bytes, on stable 32.
    assert!(std::mem::size_of::<QueuedOperationKind>() <= 32);
    assert_eq!(std::mem::size_of::<Option<task::Waker>>(), 16);
    // On nightly the following two are 48 bytes, on stable 56.
    assert!(std::mem::size_of::<QueuedOperation>() <= 56);
    assert!(std::mem::size_of::<Option<QueuedOperation>>() <= 56);
    assert_eq!(std::mem::size_of::<OpState<()>>(), 16);
    assert_eq!(std::mem::size_of::<OpState<u8>>(), 16);
    assert_eq!(std::mem::size_of::<OpState<u16>>(), 16);
    assert_eq!(std::mem::size_of::<OpState<u32>>(), 16);
    assert_eq!(std::mem::size_of::<OpState<u64>>(), 16);
}
