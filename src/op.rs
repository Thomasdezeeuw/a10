//! Code related to executing an asynchronous operations.

use std::cmp::min;
use std::mem::{replace, MaybeUninit};
use std::os::unix::io::RawFd;
use std::sync::Arc;
use std::{fmt, io};

use parking_lot::Mutex;

use crate::{libc, QueueFull, SubmissionQueue};

/// Shared version of [`OperationState`].
pub(crate) struct SharedOperationState {
    inner: Arc<Mutex<OperationState>>,
}

/// No operation is queued.
const NO_OP: i32 = i32::MIN;
/// Operation is in progress, waiting on result.
const IN_PROGRESS: i32 = i32::MIN + 1;

/// State of an asynchronous operation.
struct OperationState {
    /// Submission queue to submit operations to.
    sq: SubmissionQueue,
    /// Result of the operation.
    /// Two special states:
    /// * [`NO_OP`] menas no operation is being executed.
    /// * [`IN_PROGRESS`] means the operation is waiting.
    /// Other values mean a result from the operation; negative is a (negative)
    /// errno, positive a succesfull result.
    result: i32,
    // TODO: add flags once there used.
}

impl SharedOperationState {
    /// Create a new `SharedOperationState`.
    pub(crate) fn new(sq: SubmissionQueue) -> SharedOperationState {
        SharedOperationState {
            inner: Arc::new(Mutex::new(OperationState { sq, result: NO_OP })),
        }
    }

    /// Start a new operation by calling [`SubmissionQueue.add`].
    ///
    /// # Panics
    ///
    /// This will panic if an operation is already in progress.
    pub(crate) fn start<F>(&self, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Submission),
    {
        let mut this = self.inner.lock();
        let user_data = Arc::as_ptr(&self.inner) as u64;
        this.sq.add(|submission| {
            // SAFETY: we set the `user_data` before calling `submit` because
            // for some operations we don't want a callback, e.g. `close_fd`.
            submission.inner.user_data = user_data;
            submit(submission);
            if submission.inner.user_data != 0 {
                // If we do want a callback we need to clone the `inner` as it's
                // now owned by the submission (i.e. the kernel).
                let user_data = Arc::into_raw(self.inner.clone()) as u64;
                assert!(submission.inner.user_data == user_data);
                // Can't have two concurrent operations overwrite the result.
                debug_assert!(Arc::strong_count(&self.inner) == 2);
                assert!(this.result == NO_OP);
            }
        })?;
        this.result = IN_PROGRESS;
        Ok(())
    }

    /// Mark the asynchronous operation as complete with `result`.
    ///
    /// # Safety
    ///
    /// Caller must ensure the `user_data` was created using
    /// [`SharedOperationState::as_user_data`].
    pub(crate) unsafe fn complete(user_data: u64, result: i32) {
        let state: Arc<Mutex<OperationState>> = Arc::from_raw(user_data as _);
        debug_assert!(Arc::strong_count(&state) == 2);
        let res = replace(&mut state.lock().result, result);
        assert!(res == IN_PROGRESS);
    }

    /// Take the result of the operation, if any.
    pub(crate) fn take_result(&self) -> Option<io::Result<i32>> {
        // TODO: handle `-EBUSY` on operations.
        // TODO: handle I/O uring specific errors here, read CQE ERRORS in the
        // manual.
        let res = replace(&mut self.inner.lock().result, NO_OP);
        if res == IN_PROGRESS || res == NO_OP {
            None
        } else if res.is_negative() {
            Some(Err(io::Error::from_raw_os_error(-res)))
        } else {
            Some(Ok(res))
        }
    }
}

impl fmt::Debug for SharedOperationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.inner.lock().result;
        let mut d = f.debug_struct("SharedOperationState");
        if state == NO_OP {
            d.field("result", &"no operation");
        } else if state == IN_PROGRESS {
            d.field("result", &"in progress");
        } else {
            d.field("result", &state);
        }
        d.finish()
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

impl Submission {
    /// Reset the submission.
    #[cfg(debug_assertions)]
    pub(crate) fn reset(&mut self) {
        debug_assert!(OperationCode::Nop as u8 == 0);
        unsafe { (&mut self.inner as *mut libc::io_uring_sqe).write_bytes(0, 1) }
    }

    /// Returns `true` if the submission is unchanged after a [`reset`].
    ///
    /// [`reset`]: Submission::reset
    #[cfg(debug_assertions)]
    pub(crate) fn is_unchanged(&self) -> bool {
        self.inner.opcode == OperationCode::Nop as u8
            && self.inner.flags == 0
            && self.inner.user_data == 0
    }

    /*
    /// Set the `OperationCode` to [`Operation::Nop`].
    fn nop(&mut self) {
        self.inner.opcode = OperationCode::Nop as u8;
    }
    */

    /*
    unsafe fn read_vectored<F>(
        &mut self,
        fd: &F,
        bufs: &mut [MaybeUninitSlice<'_>], // FIXME: lifetime.
    ) where
        F: io::Read + AsRawFd,
    {
        self.read_vectored_at(fd, bufs, NO_OFFSET)
    }

    unsafe fn read_vectored_at<F>(
        &mut self,
        fd: &F,
        bufs: &mut [MaybeUninitSlice<'_>], // FIXME: lifetime.
        offset: u64,
    ) where
        F: io::Read + AsRawFd,
    {
        self.inner.opcode = OperationCode::Readv as u8;
        self.inner.fd = fd.as_raw_fd();
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: bufs.as_mut_ptr() as _,
        };
        self.inner.len = min(bufs.len(), u32::MAX as usize) as u32;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { rw_flags: 0 };
    }
    */

    /*
    /// Create a read submission.
    ///
    /// Avaialable since Linux kernel 5.6.
    pub(crate) unsafe fn read(&mut self, fd: RawFd, buf: &mut [MaybeUninit<u8>]) {
        self.read_at(fd, buf, NO_OFFSET)
    }
    */

    /// Create a timeout submission waiting for at least one completion or
    /// triggers a timeout.
    ///
    /// Avaialable since Linux kernel 5.4.
    pub(crate) unsafe fn timeout(&mut self, ts: *const libc::__kernel_timespec) {
        self.inner.opcode = OperationCode::Timeout as u8;
        self.inner.fd = -1;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: 1 };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ts as _ };
        self.inner.len = 1;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { timeout_flags: 0 };
        self.inner.user_data = 0;
    }

    /// Create a read submission starting at `offset`.
    ///
    /// Avaialable since Linux kernel 5.6.
    pub(crate) unsafe fn read_at(&mut self, fd: RawFd, buf: &mut [MaybeUninit<u8>], offset: u64) {
        self.inner.opcode = OperationCode::Read as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: buf.as_mut_ptr() as _,
        };
        self.inner.len = min(buf.len(), u32::MAX as usize) as u32;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { rw_flags: 0 };
    }

    /// Open a file by `pathname` in directory `dir_fd`.
    pub(crate) unsafe fn open_at(
        &mut self,
        dir_fd: RawFd,
        pathname: *const libc::c_char,
        flags: libc::c_int,
        mode: libc::mode_t,
    ) {
        self.inner.opcode = OperationCode::Openat as u8;
        self.inner.fd = dir_fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: 0 }; // Unused.
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: pathname as _,
        };
        self.inner.len = mode; // Name is weird, but correct.
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            open_flags: flags as _,
        };
    }

    /// Close the `fd`.
    pub(crate) unsafe fn close_fd(&mut self, fd: RawFd) {
        self.inner.opcode = OperationCode::Close as u8;
        self.inner.fd = fd;
        self.inner.user_data = 0; // Don't want a callback.
    }

    // TODO: add other operations, see `io_uring_enter` manual.

    /*
    /// Set the I/O priority.
    ///
    /// See the [`io_prio_get(2)`] manual for more information.
    ///
    /// [`io_prio_get(2)`]: https://man7.org/linux/man-pages/man2/ioprio_get.2.html
    fn set_io_priority(&mut self, io_prio: u16) {
        self.inner.ioprio = io_prio;
    }
    */
}

impl fmt::Debug for Submission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operation = OperationCode::from_u8(self.inner.opcode);
        let mut d = f.debug_struct("Submission");
        d.field("opcode", &operation)
            .field("flags", &self.inner.flags)
            .field("ioprio", &self.inner.ioprio)
            .field("fd", &self.inner.fd);
        match operation {
            OperationCode::Read => {
                d.field("off", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("addr", unsafe {
                        &(self.inner.__bindgen_anon_2.addr as *const libc::c_void)
                    });
            }
            _ => { /* TODO. */ }
        }
        d.field("len", &self.inner.len);
        match operation {
            OperationCode::Read => {
                d.field("rw_flags", unsafe { &self.inner.__bindgen_anon_3.rw_flags });
            }
            _ => { /* TODO. */ }
        }
        d.field("user_data", &self.inner.user_data).finish()
    }
}

/// Operation code, or opcode, to determine what kind of system call to execute.
#[derive(Debug)]
#[repr(u8)]
pub(crate) enum OperationCode {
    /// Do not perform any I/O.
    #[doc(alias = "IORING_OP_NOP")]
    Nop = libc::IORING_OP_NOP as u8,
    /// Vectored read operation.
    Readv = libc::IORING_OP_READV as u8,
    /// Vectored write operation.
    Writev = libc::IORING_OP_WRITEV as u8,
    /// File sync.
    Fsync = libc::IORING_OP_FSYNC as u8,
    /// Read from pre-mapped buffers.
    ReadFixed = libc::IORING_OP_READ_FIXED as u8,
    /// Write to pre-mapped buffers.
    WriteFixed = libc::IORING_OP_WRITE_FIXED as u8,
    PollAdd = libc::IORING_OP_POLL_ADD as u8,
    PollRemove = libc::IORING_OP_POLL_REMOVE as u8,
    SyncFileRange = libc::IORING_OP_SYNC_FILE_RANGE as u8,
    Sendmsg = libc::IORING_OP_SENDMSG as u8,
    Recvmsg = libc::IORING_OP_RECVMSG as u8,
    /// Register a timeout operation.
    #[doc(alias = "IORING_OP_TIMEOUT")]
    Timeout = libc::IORING_OP_TIMEOUT as u8,
    /// Remove an existing timeout operation.
    #[doc(alias = "IORING_OP_TIMEOUT_REMOVE")]
    TimeoutRemove = libc::IORING_OP_TIMEOUT_REMOVE as u8,
    Accept = libc::IORING_OP_ACCEPT as u8,
    AsyncCancel = libc::IORING_OP_ASYNC_CANCEL as u8,
    LinkTimeout = libc::IORING_OP_LINK_TIMEOUT as u8,
    Connect = libc::IORING_OP_CONNECT as u8,
    Fallocate = libc::IORING_OP_FALLOCATE as u8,
    /// Issue the equivalent of a `openat(2)` system call.
    #[doc(alias = "IORING_OP_OPENAT")]
    Openat = libc::IORING_OP_OPENAT as u8,
    /// Issue the equivalent of a `close(2)` system call.
    #[doc(alias = "IORING_OP_CLOSE")]
    Close = libc::IORING_OP_CLOSE as u8,
    FilesUpdate = libc::IORING_OP_FILES_UPDATE as u8,
    Statx = libc::IORING_OP_STATX as u8,
    Read = libc::IORING_OP_READ as u8,
    Write = libc::IORING_OP_WRITE as u8,
    Fadvise = libc::IORING_OP_FADVISE as u8,
    Madvise = libc::IORING_OP_MADVISE as u8,
    Send = libc::IORING_OP_SEND as u8,
    Recv = libc::IORING_OP_RECV as u8,
    Openat2 = libc::IORING_OP_OPENAT2 as u8,
    EpollCtl = libc::IORING_OP_EPOLL_CTL as u8,
    Splice = libc::IORING_OP_SPLICE as u8,
    ProvideBuffers = libc::IORING_OP_PROVIDE_BUFFERS as u8,
    RemoveBuffers = libc::IORING_OP_REMOVE_BUFFERS as u8,
    Tee = libc::IORING_OP_TEE as u8,
    Shutdown = libc::IORING_OP_SHUTDOWN as u8,
    Renameat = libc::IORING_OP_RENAMEAT as u8,
    Unlinkat = libc::IORING_OP_UNLINKAT as u8,
    Last = libc::IORING_OP_LAST as u8,

    #[doc(hidden)]
    Unknown = u8::MAX,
}

impl OperationCode {
    pub(crate) const fn from_u8(value: u8) -> OperationCode {
        match value as _ {
            libc::IORING_OP_NOP => OperationCode::Nop,
            libc::IORING_OP_READV => OperationCode::Readv,
            libc::IORING_OP_WRITEV => OperationCode::Writev,
            libc::IORING_OP_FSYNC => OperationCode::Fsync,
            libc::IORING_OP_READ_FIXED => OperationCode::ReadFixed,
            libc::IORING_OP_WRITE_FIXED => OperationCode::WriteFixed,
            libc::IORING_OP_POLL_ADD => OperationCode::PollAdd,
            libc::IORING_OP_POLL_REMOVE => OperationCode::PollRemove,
            libc::IORING_OP_SYNC_FILE_RANGE => OperationCode::SyncFileRange,
            libc::IORING_OP_SENDMSG => OperationCode::Sendmsg,
            libc::IORING_OP_RECVMSG => OperationCode::Recvmsg,
            libc::IORING_OP_TIMEOUT => OperationCode::Timeout,
            libc::IORING_OP_TIMEOUT_REMOVE => OperationCode::TimeoutRemove,
            libc::IORING_OP_ACCEPT => OperationCode::Accept,
            libc::IORING_OP_ASYNC_CANCEL => OperationCode::AsyncCancel,
            libc::IORING_OP_LINK_TIMEOUT => OperationCode::LinkTimeout,
            libc::IORING_OP_CONNECT => OperationCode::Connect,
            libc::IORING_OP_FALLOCATE => OperationCode::Fallocate,
            libc::IORING_OP_OPENAT => OperationCode::Openat,
            libc::IORING_OP_CLOSE => OperationCode::Close,
            libc::IORING_OP_FILES_UPDATE => OperationCode::FilesUpdate,
            libc::IORING_OP_STATX => OperationCode::Statx,
            libc::IORING_OP_READ => OperationCode::Read,
            libc::IORING_OP_WRITE => OperationCode::Write,
            libc::IORING_OP_FADVISE => OperationCode::Fadvise,
            libc::IORING_OP_MADVISE => OperationCode::Madvise,
            libc::IORING_OP_SEND => OperationCode::Send,
            libc::IORING_OP_RECV => OperationCode::Recv,
            libc::IORING_OP_OPENAT2 => OperationCode::Openat2,
            libc::IORING_OP_EPOLL_CTL => OperationCode::EpollCtl,
            libc::IORING_OP_SPLICE => OperationCode::Splice,
            libc::IORING_OP_PROVIDE_BUFFERS => OperationCode::ProvideBuffers,
            libc::IORING_OP_REMOVE_BUFFERS => OperationCode::RemoveBuffers,
            libc::IORING_OP_TEE => OperationCode::Tee,
            libc::IORING_OP_SHUTDOWN => OperationCode::Shutdown,
            libc::IORING_OP_RENAMEAT => OperationCode::Renameat,
            libc::IORING_OP_UNLINKAT => OperationCode::Unlinkat,
            libc::IORING_OP_LAST => OperationCode::Last,
            _ => OperationCode::Unknown,
        }
    }
}
