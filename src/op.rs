//! Code related to executing an asynchronous operations.

use std::cmp::min;
use std::mem::{replace, MaybeUninit};
use std::os::unix::io::RawFd;
use std::sync::{Arc, Mutex};
use std::task::{self, Poll};
use std::{fmt, io, ptr};

use crate::{libc, QueueFull, SubmissionQueue};

/// Shared version of [`OperationState`].
pub(crate) struct SharedOperationState {
    inner: Arc<Mutex<OperationState>>,
}

/// State of an asynchronous operation.
struct OperationState {
    /// Submission queue to submit operations to.
    sq: SubmissionQueue,
    /// Result of the operation.
    result: OperationResult,
    waker: Option<task::Waker>,
}

#[derive(Copy, Clone, Debug)]
enum OperationResult {
    /// No operation queued.
    NotStarted,
    /// Operation is in progress, waiting on result.
    InProgress,
    /// Operation done.
    ///
    /// Value is the result from the operation; negative is a (negative) errno,
    /// positive a successful result.
    Done(i32),
}

impl SharedOperationState {
    /// Create a new `SharedOperationState`.
    pub(crate) fn new(sq: SubmissionQueue) -> SharedOperationState {
        SharedOperationState {
            inner: Arc::new(Mutex::new(OperationState {
                sq,
                result: OperationResult::NotStarted,
                waker: None,
            })),
        }
    }

    pub(crate) fn submission_queue(&self) -> SubmissionQueue {
        self.inner.lock().unwrap().sq.clone()
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
        let mut this = self.inner.lock().unwrap();
        let user_data = Arc::as_ptr(&self.inner) as u64;
        this.sq.add(|submission| {
            // SAFETY: we set the `user_data` before calling `submit` because
            // for some operations we don't want a callback, e.g. `close_fd`.
            submission.inner.user_data = user_data;
            submit(submission);
            if submission.inner.user_data != 0 {
                // If we do want a callback we need to clone the `inner` as it
                // will owned by the submission (i.e. the kernel).
                let user_data = Arc::into_raw(self.inner.clone()) as u64;
                debug_assert_eq!(submission.inner.user_data, user_data);
                // Can't have two concurrent operations overwriting the result.
                // However because we wake the waker before we reduce the strong
                // count (in `complete`) there is a small gap where the the lock
                // is unlocked, but the strong count isn't reduced yet. In that
                // gap as stricter assertion (count == 2) would fail.
                debug_assert!({
                    let count = Arc::strong_count(&self.inner);
                    count == 2 || count == 3
                });
                assert!(matches!(this.result, OperationResult::NotStarted));
            }
        })?;
        this.result = OperationResult::InProgress;
        Ok(())
    }

    /// Mark the asynchronous operation as complete with `result`.
    ///
    /// # Safety
    ///
    /// Caller must ensure the `user_data` was created in
    /// [`SharedOperationState::start`].
    pub(crate) unsafe fn complete(user_data: u64, result: i32) {
        let state: Arc<Mutex<OperationState>> = Arc::from_raw(user_data as _);
        debug_assert!(Arc::strong_count(&state) == 2);
        let mut state = state.lock().unwrap();
        let res = replace(&mut state.result, OperationResult::Done(result));
        assert!(matches!(res, OperationResult::InProgress));
        if let Some(waker) = &state.waker {
            waker.wake_by_ref();
        }
    }

    /// Poll the operation check if it's ready.
    pub(crate) fn poll(&self, ctx: &mut task::Context<'_>) -> Poll<io::Result<i32>> {
        let mut this = self.inner.lock().unwrap();
        match this.result {
            OperationResult::NotStarted => panic!("a10::OperationState in invalid state"),
            OperationResult::InProgress => {
                let waker = ctx.waker();
                if !matches!(&this.waker, Some(w) if w.will_wake(waker)) {
                    this.waker = Some(waker.clone());
                }
                Poll::Pending
            }
            OperationResult::Done(res) => {
                this.result = OperationResult::NotStarted;
                if res.is_negative() {
                    // TODO: handle `-EBUSY` on operations.
                    // TODO: handle io_uring specific errors here, read CQE
                    // ERRORS in the manual.
                    Poll::Ready(Err(io::Error::from_raw_os_error(-res)))
                } else {
                    Poll::Ready(Ok(res))
                }
            }
        }
    }
}

impl fmt::Debug for SharedOperationState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedOperationState")
            .field("result", &self.inner.lock().unwrap().result)
            .finish()
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
    pub(crate) fn reset(&mut self) {
        debug_assert!(OperationCode::Nop as u8 == 0);
        unsafe {
            ptr::addr_of_mut!(self.inner)
                .cast::<libc::io_uring_sqe>()
                .write_bytes(0, 1);
        }
    }

    /// Returns `true` if the submission is unchanged after a [`reset`].
    ///
    /// [`reset`]: Submission::reset
    #[cfg(debug_assertions)]
    pub(crate) const fn is_unchanged(&self) -> bool {
        self.inner.opcode == OperationCode::Nop as u8
            && self.inner.flags == 0
            && self.inner.user_data == 0
    }

    /// Sync the `fd` with `fsync_flags`.
    pub(crate) unsafe fn fsync(&mut self, fd: RawFd, fsync_flags: libc::__u32) {
        self.inner.opcode = OperationCode::Fsync as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 { fsync_flags };
    }

    /// Create a timeout submission waiting for at least one completion or
    /// triggers a timeout.
    ///
    /// Avaialable since Linux kernel 5.4.
    pub(crate) unsafe fn timeout(&mut self, ts: *const libc::timespec) {
        self.inner.opcode = OperationCode::Timeout as u8;
        self.inner.fd = -1;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: 1 };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ts as _ };
        self.inner.len = 1;
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
    }

    /// Create a write submission starting at `offset`.
    ///
    /// Avaialable since Linux kernel 5.6.
    pub(crate) unsafe fn write_at(&mut self, fd: RawFd, buf: &[u8], offset: u64) {
        self.inner.opcode = OperationCode::Write as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: offset };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: buf.as_ptr() as _,
        };
        self.inner.len = min(buf.len(), u32::MAX as usize) as u32;
    }

    pub(crate) unsafe fn socket(
        &mut self,
        domain: libc::c_int,
        r#type: libc::c_int,
        protocol: libc::c_int,
        flags: libc::c_int,
    ) {
        self.inner.opcode = OperationCode::Socket as u8;
        self.inner.fd = domain;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: r#type as _ };
        self.inner.len = protocol as _;
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            rw_flags: flags as _,
        };
    }

    pub(crate) unsafe fn connect(
        &mut self,
        fd: RawFd,
        address: &mut libc::sockaddr_storage,
        address_length: libc::socklen_t,
    ) {
        self.inner.opcode = OperationCode::Connect as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: address_length as _,
        };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: address as *mut _ as _,
        };
    }

    pub(crate) unsafe fn send(&mut self, fd: RawFd, buf: &mut [u8], flags: libc::c_int) {
        self.inner.opcode = OperationCode::Send as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: buf.as_mut_ptr() as _,
        };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: flags as _,
        };
        self.inner.len = min(buf.len(), u32::MAX as usize) as u32;
    }

    pub(crate) unsafe fn recv(
        &mut self,
        fd: RawFd,
        buf: &mut [MaybeUninit<u8>],
        flags: libc::c_int,
    ) {
        self.inner.opcode = OperationCode::Recv as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: buf.as_mut_ptr() as _,
        };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: flags as _,
        };
        self.inner.len = min(buf.len(), u32::MAX as usize) as u32;
    }

    /// Create a accept submission starting.
    ///
    /// Avaialable since Linux kernel 5.5.
    pub(crate) unsafe fn accept(
        &mut self,
        fd: RawFd,
        address: &mut MaybeUninit<libc::sockaddr_storage>,
        address_length: &mut libc::socklen_t,
        flags: libc::c_int,
    ) {
        self.inner.opcode = OperationCode::Accept as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            addr2: address_length as *mut _ as _,
        };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: address as *mut _ as _,
        };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            accept_flags: flags as _,
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
    pub(crate) unsafe fn close(&mut self, fd: RawFd, want_callback: bool) {
        self.inner.opcode = OperationCode::Close as u8;
        self.inner.fd = fd;
        if !want_callback {
            self.inner.user_data = 0; // Don't want a callback.
        }
    }

    /// Call `statx(2)` on `fd`, where `fd` points to a file.
    ///
    /// Avaialable since Linux kernel 5.6.
    pub(crate) unsafe fn statx_file(&mut self, fd: RawFd, statx: &mut libc::statx, flags: u32) {
        self.inner.opcode = OperationCode::Statx as u8;
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
    Nop = libc::IORING_OP_NOP as u8,
    Readv = libc::IORING_OP_READV as u8,
    Writev = libc::IORING_OP_WRITEV as u8,
    Fsync = libc::IORING_OP_FSYNC as u8,
    ReadFixed = libc::IORING_OP_READ_FIXED as u8,
    WriteFixed = libc::IORING_OP_WRITE_FIXED as u8,
    PollAdd = libc::IORING_OP_POLL_ADD as u8,
    PollRemove = libc::IORING_OP_POLL_REMOVE as u8,
    SyncFileRange = libc::IORING_OP_SYNC_FILE_RANGE as u8,
    Sendmsg = libc::IORING_OP_SENDMSG as u8,
    Recvmsg = libc::IORING_OP_RECVMSG as u8,
    Timeout = libc::IORING_OP_TIMEOUT as u8,
    TimeoutRemove = libc::IORING_OP_TIMEOUT_REMOVE as u8,
    Accept = libc::IORING_OP_ACCEPT as u8,
    AsyncCancel = libc::IORING_OP_ASYNC_CANCEL as u8,
    LinkTimeout = libc::IORING_OP_LINK_TIMEOUT as u8,
    Connect = libc::IORING_OP_CONNECT as u8,
    Fallocate = libc::IORING_OP_FALLOCATE as u8,
    Openat = libc::IORING_OP_OPENAT as u8,
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
    MkDirat = libc::IORING_OP_MKDIRAT as u8,
    SymLinkat = libc::IORING_OP_SYMLINKAT as u8,
    Linkat = libc::IORING_OP_LINKAT as u8,
    MsgRing = libc::IORING_OP_MSG_RING as u8,
    FSetXAttr = libc::IORING_OP_FSETXATTR as u8,
    SetXattr = libc::IORING_OP_SETXATTR as u8,
    FGetXAttr = libc::IORING_OP_FGETXATTR as u8,
    GetXAttr = libc::IORING_OP_GETXATTR as u8,
    Socket = libc::IORING_OP_SOCKET as u8,
    UringCmd = libc::IORING_OP_URING_CMD as u8,
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
            libc::IORING_OP_MKDIRAT => OperationCode::MkDirat,
            libc::IORING_OP_SYMLINKAT => OperationCode::SymLinkat,
            libc::IORING_OP_LINKAT => OperationCode::Linkat,
            libc::IORING_OP_MSG_RING => OperationCode::MsgRing,
            libc::IORING_OP_FSETXATTR => OperationCode::FSetXAttr,
            libc::IORING_OP_SETXATTR => OperationCode::SetXattr,
            libc::IORING_OP_FGETXATTR => OperationCode::FGetXAttr,
            libc::IORING_OP_GETXATTR => OperationCode::GetXAttr,
            libc::IORING_OP_SOCKET => OperationCode::Socket,
            libc::IORING_OP_URING_CMD => OperationCode::UringCmd,
            libc::IORING_OP_LAST => OperationCode::Last,
            _ => OperationCode::Unknown,
        }
    }
}

/// Macro to create an operation [`Future`] structure.
///
/// [`Future`]: std::future::Future
#[macro_export]
macro_rules! op_future {
    (
        // File type and function name.
        fn $f: ident :: $fn: ident -> $result: ty,
        // Future structure.
        struct $name: ident < $lifetime: lifetime > {
            $(
            // Field passed to io_uring, must be an `Option`. Syntax is the same
            // a struct definition, with `$drop_msg` being the message logged
            // when leaking `$field`.
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty, $drop_msg: expr,
            )?
        },
        // Mapping function for `SharedOperationState::poll` result.
        |$self: ident, $arg: ident| $map_result: expr,
        // Mapping function for `Extractor` implementation.
        $( extract: |$extract_self: ident, $extract_arg: ident| -> $extract_result: ty $extract_map: block )?
    ) => {
        #[doc = concat!("[`Future`](std::future::Future) behind [`", stringify!($f), "::", stringify!($fn), "`].")]
        #[derive(Debug)]
        pub struct $name<$lifetime> {
            $(
            $(#[ $field_doc ])*
            $field: std::option::Option<std::cell::UnsafeCell<$value>>,
            )?
            fd: &$lifetime $f,
        }

        impl<$lifetime> std::future::Future for $name<$lifetime> {
            type Output = std::io::Result<$result>;

            fn poll(mut self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
                match self.fd.state.poll(ctx) {
                    std::task::Poll::Ready(std::result::Result::Ok($arg)) => std::task::Poll::Ready({
                        let $self = &mut self;
                        $map_result
                    }),
                    std::task::Poll::Ready(std::result::Result::Err(err)) => std::task::Poll::Ready(std::result::Result::Err(err)),
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }
        }

        $(
        impl<$lifetime> $crate::Extract for $name<$lifetime> {}

        impl<$lifetime> std::future::Future for $crate::extract::Extractor<$name<$lifetime>> {
            type Output = std::io::Result<$extract_result>;

            fn poll(mut self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
                match self.fut.fd.state.poll(ctx) {
                    std::task::Poll::Ready(std::result::Result::Ok($extract_arg)) => std::task::Poll::Ready({
                        let $extract_self = &mut self.fut;
                        $extract_map
                    }),
                    std::task::Poll::Ready(std::result::Result::Err(err)) => std::task::Poll::Ready(std::result::Result::Err(err)),
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }
        }
        )?

        impl<$lifetime> std::ops::Drop for $name<$lifetime> {
            fn drop(&mut self) {
                $(
                if let Some($field) = std::mem::take(&mut self.$field) {
                    log::debug!($drop_msg);
                    std::mem::forget($field);
                }
                )?
            }
        }
    };
    // Version that doesn't need `self` (this) in `$map_result`.
    (
        fn $f: ident :: $fn: ident -> $result: ty,
        struct $name: ident < $lifetime: lifetime > {
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty, $drop_msg: expr,
            )?
        },
        |$n: ident| $map_result: expr, // Only difference: 1 argument.
        $( extract: |$extract_self: ident, $extract_arg: ident| -> $extract_result: ty $extract_map: block )?
    ) => {
        op_future!{
            fn $f :: $fn -> $result,
            struct $name<$lifetime> {
                $(
                $(#[ $field_doc ])*
                $field: $value, $drop_msg,
                )?
            },
            |_unused_this, $n| $map_result,
            $( extract: |$extract_self, $extract_arg| -> $extract_result $extract_map )*
        }
    };
}
