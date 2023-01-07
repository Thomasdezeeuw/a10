//! Code related to executing an asynchronous operations.

use std::mem::{replace, MaybeUninit};
use std::os::unix::io::RawFd;
use std::task::{self, Poll};
use std::{fmt, io, ptr};

use crate::{libc, OpIndex};

/// State of a queue operation.
#[derive(Debug)]
pub(crate) struct QueuedOperation {
    /// Result of the operation.
    result: OperationResult,
    /// Waker to wake when the operation is done.
    waker: Option<task::Waker>,
}

impl QueuedOperation {
    /// Create a queued operation.
    pub(crate) const fn new() -> QueuedOperation {
        QueuedOperation {
            result: OperationResult::Started,
            waker: None,
        }
    }

    /// Set the result of the operation, but don't mark it as complete.
    pub(crate) fn set_in_progress_result(&mut self, flags: u16, result: i32) {
        match self.result {
            OperationResult::Started => self.result = OperationResult::InProgress { flags, result },
            OperationResult::Dropped => {
                self.result = OperationResult::InProgressDropped { flags, result }
            }
            OperationResult::InProgress { .. }
            | OperationResult::InProgressDropped { .. }
            | OperationResult::Done { .. } => {
                unreachable!("set_in_progress_result called incorrectly")
            }
        }
    }

    /// Wake to `Future` waiting for the result.
    ///
    /// Returns `true` if the operation was previously dropped.
    pub(crate) fn complete_in_progress(&mut self) -> bool {
        let is_dropped = match self.result {
            OperationResult::InProgress { flags, result } => {
                self.result = OperationResult::Done { flags, result };
                false
            }
            OperationResult::InProgressDropped { flags, result } => {
                self.result = OperationResult::Done { flags, result };
                true
            }
            OperationResult::Started | OperationResult::Dropped | OperationResult::Done { .. } => {
                unreachable!("complete_in_progress called incorrectly")
            }
        };
        // NOTE: if `is_dropped` is true this drops the operations' resources
        // (e.g. buffers).
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
        is_dropped
    }

    /// Set the result of the operation to `result` and wake to `Future` waiting
    /// for the result.
    ///
    /// Returns `true` if the operation was previously dropped.
    pub(crate) fn complete(&mut self, flags: u16, result: i32) -> bool {
        let old_state = replace(&mut self.result, OperationResult::Done { flags, result });
        debug_assert!(match old_state {
            OperationResult::Started | OperationResult::Dropped => true,
            OperationResult::InProgress { .. }
            | OperationResult::InProgressDropped { .. }
            | OperationResult::Done { .. } => false,
        });
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
        // NOTE: `OperationResult::InProgressDropped` shouldn't be reachable
        // here.
        matches!(old_state, OperationResult::Dropped)
    }

    /// Poll the operation check if it's ready.
    ///
    /// Returns the `flags` and the `result` (always positive).
    pub(crate) fn poll(&mut self, ctx: &mut task::Context<'_>) -> Poll<io::Result<(u16, i32)>> {
        match self.result {
            OperationResult::Started | OperationResult::InProgress { .. } => {
                let waker = ctx.waker();
                if !matches!(&self.waker, Some(w) if w.will_wake(waker)) {
                    self.waker = Some(waker.clone());
                }
                Poll::Pending
            }
            OperationResult::Dropped | OperationResult::InProgressDropped { .. } => {
                unreachable!("polling a dropped Future")
            }
            OperationResult::Done { flags, result } => {
                if result.is_negative() {
                    // TODO: handle `-EBUSY` on operations.
                    // TODO: handle io_uring specific errors here, read CQE
                    // ERRORS in the manual.
                    Poll::Ready(Err(io::Error::from_raw_os_error(-result)))
                } else {
                    Poll::Ready(Ok((flags, result)))
                }
            }
        }
    }

    /// Returns true if the operation is done.
    pub(crate) const fn is_done(&self) -> bool {
        matches!(self.result, OperationResult::Done { .. })
    }

    /// Set the state of the operation as dropped, but still in progress kernel
    /// side. This set the waker to `waker` and make `set_result` return `true`.
    pub(crate) fn set_dropped(&mut self, waker: task::Waker) {
        let res = replace(&mut self.result, OperationResult::Dropped);
        debug_assert!(match res {
            OperationResult::Started | OperationResult::InProgress { .. } => true,
            OperationResult::Dropped
            | OperationResult::InProgressDropped { .. }
            | OperationResult::Done { .. } => false,
        });
        self.waker = Some(waker);
    }
}

/// Result of an operation.
#[derive(Copy, Clone, Debug)]
enum OperationResult {
    /// Operation has started, but hasn't report a result yet.
    Started,
    /// Operation is in progress, but has already reported the result.
    ///
    /// This is for zero copy operations which report the result in one
    /// completion and notify us in another completion when the buffer can be
    /// released.
    ///
    InProgress { flags: u16, result: i32 },
    /// The `Future` waiting for this operation has been dropped, but kernel
    /// side it's still in progress.
    Dropped,
    /// Combination of `InProgress` and `Dropped`.
    InProgressDropped { flags: u16, result: i32 },
    /// Operation done.
    ///
    /// The `flags` are the 16 upper bits of `io_uring_cqe::flags`, usually the
    /// index of a buffer in a buffer pool. `result` is the result of an
    /// operation; negative is a (negative) errno, positive a successful result.
    /// The meaning of both fields are depended on the operation itself.
    Done { flags: u16, result: i32 },
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
        debug_assert!(libc::IORING_OP_NOP == 0);
        unsafe { ptr::addr_of_mut!(self.inner).write_bytes(0, 1) };
    }

    /// Set the user data to `user_data`.
    pub(crate) const fn set_user_data(&mut self, user_data: u64) {
        self.inner.user_data = user_data;
    }

    /// Mark the submission as using `IOSQE_BUFFER_SELECT`.
    pub(crate) const fn set_buffer_select(&mut self, buf_group: u16) {
        self.inner.__bindgen_anon_4.buf_group = buf_group;
        self.inner.flags |= libc::IOSQE_BUFFER_SELECT;
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
        self.inner.opcode = libc::IORING_OP_CONNECT as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: address_length as _,
        };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: address as *mut _ as _,
        };
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

    pub(crate) unsafe fn recv(&mut self, fd: RawFd, ptr: *mut u8, len: u32, flags: libc::c_int) {
        self.inner.opcode = libc::IORING_OP_RECV as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ptr as _ };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            msg_flags: flags as _,
        };
        self.inner.len = len;
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
        self.inner.opcode = libc::IORING_OP_ACCEPT as u8;
        self.inner.fd = fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: address_length as *mut _ as _,
        };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 {
            addr: address as *mut _ as _,
        };
        self.inner.__bindgen_anon_3 = libc::io_uring_sqe__bindgen_ty_3 {
            accept_flags: flags as _,
        };
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

    pub(crate) unsafe fn wake(&mut self, ring_fd: RawFd) {
        self.inner.opcode = libc::IORING_OP_MSG_RING as u8;
        self.inner.fd = ring_fd;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 {
            off: u64::MAX, // `user_data` in the completion event.
        };
    }
}

impl fmt::Debug for Submission {
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
            f.field("opcode", &name)
                .field("fd", &submission.fd)
                .field("addr", unsafe { &submission.__bindgen_anon_2.addr })
                .field("len", &submission.len)
                .field("flags", unsafe { &submission.__bindgen_anon_3.msg_flags });
        }

        let mut f = f.debug_struct("Submission");
        match self.inner.opcode as u32 {
            libc::IORING_OP_FSYNC => {
                f.field("opcode", &"IORING_OP_FSYNC")
                    .field("fd", &self.inner.fd)
                    .field("fsync_flags", unsafe {
                        &self.inner.__bindgen_anon_3.fsync_flags
                    });
            }
            libc::IORING_OP_READ => io_op(&mut f, &self.inner, "IORING_OP_READ"),
            libc::IORING_OP_WRITE => io_op(&mut f, &self.inner, "IORING_OP_WRITE"),
            libc::IORING_OP_SOCKET => {
                f.field("opcode", &"IORING_OP_SOCKET")
                    .field("domain", &self.inner.fd)
                    .field("type", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("protocol", &self.inner.len)
                    .field("flags", unsafe { &self.inner.__bindgen_anon_3.rw_flags });
            }
            libc::IORING_OP_CONNECT => {
                f.field("opcode", &"IORING_OP_CONNECT")
                    .field("fd", &self.inner.fd)
                    .field("addr", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("addr_size", unsafe { &self.inner.__bindgen_anon_1.off });
            }
            libc::IORING_OP_SEND => net_op(&mut f, &self.inner, "IORING_OP_SEND"),
            libc::IORING_OP_SEND_ZC => net_op(&mut f, &self.inner, "IORING_OP_SEND_ZC"),
            libc::IORING_OP_RECV => net_op(&mut f, &self.inner, "IORING_OP_RECV"),
            libc::IORING_OP_ACCEPT => {
                f.field("opcode", &"IORING_OP_ACCEPT")
                    .field("fd", &self.inner.fd)
                    .field("addr", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("addr_size", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("flags", unsafe {
                        &self.inner.__bindgen_anon_3.accept_flags
                    });
            }
            libc::IORING_OP_ASYNC_CANCEL => {
                f.field("opcode", &"IORING_OP_ASYNC_CANCEL")
                    .field("fd", &self.inner.fd)
                    .field("flags", unsafe {
                        &self.inner.__bindgen_anon_3.cancel_flags
                    });
            }
            libc::IORING_OP_OPENAT => {
                f.field("opcode", &"IORING_OP_OPENAT")
                    .field("dirfd", &self.inner.fd)
                    .field("pathname", unsafe {
                        &self.inner.__bindgen_anon_3.cancel_flags
                    })
                    .field("mode", &self.inner.len)
                    .field("flags", unsafe { &self.inner.__bindgen_anon_3.open_flags });
            }
            libc::IORING_OP_CLOSE => {
                f.field("opcode", &"IORING_OP_CLOSE")
                    .field("fd", &self.inner.fd);
            }
            libc::IORING_OP_STATX => {
                f.field("opcode", &"IORING_OP_STATX")
                    .field("fd", &self.inner.fd)
                    .field("pathname", unsafe { &self.inner.__bindgen_anon_2.addr })
                    .field("flags", unsafe { &self.inner.__bindgen_anon_3.statx_flags })
                    .field("mask", &self.inner.len)
                    .field("statx", unsafe { &self.inner.__bindgen_anon_1.off });
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
        f.field("user_data", &self.inner.user_data).finish()
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
        struct $name: ident < $lifetime: lifetime $(, $generic: ident: $($trait: ident)? )* > {
            $(
            // Field passed to io_uring, always wrapped in an `Option`. Syntax
            // is the same a struct definition.
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty,
            )*
        },
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
            struct $name<$lifetime $(, $generic: $($trait)? )*> {
                $(
                $(#[$field_doc])*
                $field: $value,
                )*
            },
            setup_state: $setup_field : $setup_ty,
            setup: |$setup_submission, $setup_fd, $setup_resources, $setup_state| $setup_fn,
            map_result: |$self, $resources, $flags, $map_arg| $map_result,
        }

        impl<$lifetime $(, $generic: std::marker::Unpin $(+ $trait)? )*> $crate::Extract for $name<$lifetime $(, $generic)*> {}

        impl<$lifetime $(, $generic: std::marker::Unpin $(+ $trait)? )*> std::future::Future for $crate::extract::Extractor<$name<$lifetime $(, $generic)*>> {
            type Output = std::io::Result<$extract_result>;

            fn poll(mut self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
                let op_index = std::task::ready!(self.fut.poll_op_index(ctx));

                match self.fut.fd.sq.poll_op(ctx, op_index) {
                    std::task::Poll::Ready(result) => {
                        self.fut.state = $crate::op::OpState::Done;
                        match result {
                            std::result::Result::Ok(($extract_flags, $extract_arg)) => {
                                let $extract_self = &mut self.fut;
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
        struct $name: ident < $lifetime: lifetime $(, $generic: ident: $($trait: ident)? )* > {
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty,
            )*
        },
        setup_state: $setup_field: ident : $setup_ty: ty,
        setup: |$setup_submission: ident, $setup_fd: ident, $setup_resources: tt, $setup_state: tt| $setup_fn: expr,
        map_result: |$self: ident, $resources: tt, $flags: ident, $map_arg: ident| $map_result: expr,
    ) => {
        #[doc = concat!("[`Future`](std::future::Future) behind [`", stringify!($type), "::", stringify!($method), "`].")]
        #[derive(Debug)]
        pub struct $name<$lifetime $(, $generic)*> {
            /// Resoures used in the operation.
            ///
            /// If this is `Some` when the future is dropped it will assume it
            /// was dropped before completion and set the operation state to
            /// dropped.
            resources: std::option::Option<std::cell::UnsafeCell<(
                $( $value, )*
            )>>,
            fd: &$lifetime $crate::AsyncFd,
            /// State of the operation.
            state: $crate::op::OpState<$setup_ty>,
        }

        impl<$lifetime $(, $generic )*> $name<$lifetime $(, $generic)*> {
            #[doc = concat!("Create a new `", stringify!($name), "`.")]
            const fn new(fd: &$lifetime $crate::AsyncFd, $( $field: $value, )* $setup_field : $setup_ty) -> $name<$lifetime $(, $generic)*> {
                $name {
                    resources: std::option::Option::Some(std::cell::UnsafeCell::new((
                        $( $field, )*
                    ))),
                    fd,
                    state: $crate::op::OpState::NotStarted($setup_field),
                }
            }
        }

        impl<$lifetime $(, $generic $(: $trait)? )*> $name<$lifetime $(, $generic)*> {
            /// Poll for the `OpIndex`.
            fn poll_op_index(&mut self, ctx: &mut std::task::Context<'_>) -> std::task::Poll<$crate::OpIndex> {
                std::task::Poll::Ready($crate::op::poll_state!($name, *self, ctx, |$setup_submission, $setup_fd, $setup_resources, $setup_state| {
                    $setup_fn
                }))
            }
        }

        impl<$lifetime $(, $generic: std::marker::Unpin $(+ $trait)? )*> std::future::Future for $name<$lifetime $(, $generic)*> {
            type Output = std::io::Result<$result>;

            fn poll(mut self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
                let op_index = std::task::ready!(self.poll_op_index(ctx));

                match self.fd.sq.poll_op(ctx, op_index) {
                    std::task::Poll::Ready(result) => {
                        self.state = $crate::op::OpState::Done;
                        match result {
                            std::result::Result::Ok(($flags, $map_arg)) => {
                                let $self = &mut self;
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

        impl<$lifetime $(, $generic)*> std::ops::Drop for $name<$lifetime $(, $generic)*> {
            fn drop(&mut self) {
                if let std::option::Option::Some(resources) = self.resources.take() {
                    match self.state {
                        $crate::op::OpState::Waiting(op_index) => self.fd.sq.drop_op(op_index, resources),
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
        struct $name: ident < $lifetime: lifetime $(, $generic: ident: $($trait: ident)? )* > {
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty,
            )*
        },
        setup_state: $setup_data: ident : $setup_ty: ty,
        setup: |$setup_submission: ident, $setup_fd: ident, $setup_resources: tt, $setup_state: tt| $setup_fn: expr,
        map_result: |$self: ident, $resources: tt, $map_arg: ident| $map_result: expr,
        $( extract: |$extract_self: ident, $extract_resources: tt, $extract_flags: ident, $extract_arg: ident| -> $extract_result: ty $extract_map: block, )?
    ) => {
        $crate::op::op_future!{
            fn $type::$method -> $result,
            struct $name<$lifetime $(, $generic: $($trait)? )*> {
                $(
                $(#[$field_doc])*
                $field: $value,
                )*
            },
            setup_state: $setup_data: $setup_ty,
            setup: |$setup_submission, $setup_fd, $setup_resources, $setup_state| $setup_fn,
            map_result: |$self, $resources, _unused_flags, $map_arg| $map_result,
            $( extract: |$extract_self, $extract_resources, _unused_flags, $extract_arg| -> $extract_result $extract_map, )?
        }
    };
    // Version that doesn't need `self` (this) or resources in `$map_result`.
    (
        fn $type: ident :: $method: ident -> $result: ty,
        struct $name: ident < $lifetime: lifetime $(, $generic: ident: $($trait: ident)? )* > {
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty,
            )*
        },
        setup_state: $setup_field: ident : $setup_ty: ty,
        setup: |$setup_submission: ident, $setup_fd: ident, $setup_resources: tt, $setup_state: tt| $setup_fn: expr,
        map_result: |$map_arg: ident| $map_result: expr, // Only difference: 1 argument.
        $( extract: |$extract_self: ident, $extract_resources: tt, $extract_arg: ident| -> $extract_result: ty $extract_map: block, )?
    ) => {
        $crate::op::op_future!{
            fn $type::$method -> $result,
            struct $name<$lifetime $(, $generic: $($trait)? )*> {
                $(
                $(#[$field_doc])*
                $field: $value,
                )*
            },
            setup_state: $setup_field : $setup_ty,
            setup: |$setup_submission, $setup_fd, $setup_resources, $setup_state| $setup_fn,
            map_result: |_unused_this, _unused_resources, _unused_flags, $map_arg| $map_result,
            $( extract: |$extract_self, $extract_resources, _unused_flags, $extract_arg| -> $extract_result $extract_map, )?
        }
    };
}

pub(crate) use op_future;

/// State of an [`op_future!`] [`Future`].
///
/// [`Future`]: std::future::Future
#[derive(Debug)]
pub(crate) enum OpState<S> {
    /// The operation has not started yet.
    NotStarted(S),
    /// Operation has started, waiting for the result.
    Waiting(OpIndex),
    /// Operation is done.
    Done,
}

/// Poll an [`OpState`].
macro_rules! poll_state {
    // Variant used by `op_future!`.
    (
        $name: ident, $self: expr, $ctx: expr,
        |$setup_submission: ident, $setup_fd: ident, $setup_resources: tt, $setup_state: tt| $setup_fn: expr $(,)?
    ) => {
        match $self.state {
            $crate::op::OpState::Waiting(op_index) => op_index,
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
                        $self.state = $crate::op::OpState::Waiting(op_index);
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
            $crate::op::OpState::Waiting(op_index) => op_index,
            $crate::op::OpState::NotStarted($setup_state) => {
                let $setup_fd = $self.fd;
                let result = $self.fd.sq.add(|$setup_submission| $setup_fn);
                match result {
                    Ok(op_index) => {
                        $self.state = $crate::op::OpState::Waiting(op_index);
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
            $crate::op::OpState::Waiting(op_index) => op_index,
            $crate::op::OpState::NotStarted($setup_state) => {
                let result = $sq.add(|$setup_submission| $setup_fn);
                match result {
                    Ok(op_index) => {
                        $state = $crate::op::OpState::Waiting(op_index);
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

#[test]
fn size_assertion() {
    assert_eq!(std::mem::size_of::<QueuedOperation>(), 24);
    assert_eq!(std::mem::size_of::<Option<QueuedOperation>>(), 24);
    assert_eq!(std::mem::size_of::<OperationResult>(), 8);
    assert_eq!(std::mem::size_of::<OpState<()>>(), 16);
    assert_eq!(std::mem::size_of::<OpState<u8>>(), 16);
    assert_eq!(std::mem::size_of::<OpState<u16>>(), 16);
    assert_eq!(std::mem::size_of::<OpState<u32>>(), 16);
    assert_eq!(std::mem::size_of::<OpState<u64>>(), 16);
}
