//! Code related to executing an asynchronous operations.

use std::mem::{replace, MaybeUninit};
use std::os::unix::io::RawFd;
use std::task::{self, Poll};
use std::{fmt, io, ptr};

use crate::libc;

/// State of a queue operation.
#[derive(Debug)]
pub(crate) struct QueuedOperation {
    /// Result of the operation.
    // NOTE: we could reduce the size of `OperationResult` to an `i32` by using
    // some unused error number to represent `InProgress`, but on 64bit padding
    // is added and we end up with 24 bytes any way, so not point at the moment.
    result: OperationResult,
    /// Waker to wake when the operation is done.
    waker: Option<task::Waker>,
}

impl QueuedOperation {
    /// Create a queued operation, marked as in progress.
    pub(crate) const fn in_progress() -> QueuedOperation {
        QueuedOperation {
            result: OperationResult::InProgress,
            waker: None,
        }
    }

    /// Set the result of the operation to `result` and wake to `Future` waiting
    /// for the result.
    ///
    /// Returns `true` if the operation was previously dropped.
    pub(crate) fn set_result(&mut self, result: i32) -> bool {
        let res = replace(&mut self.result, OperationResult::Done(result));
        debug_assert!(matches!(
            res,
            OperationResult::InProgress | OperationResult::Dropped
        ));
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
        matches!(res, OperationResult::Dropped)
    }

    /// Poll the operation check if it's ready.
    pub(crate) fn poll(&mut self, ctx: &mut task::Context<'_>) -> Poll<io::Result<i32>> {
        match self.result {
            OperationResult::InProgress => {
                let waker = ctx.waker();
                if !matches!(&self.waker, Some(w) if w.will_wake(waker)) {
                    self.waker = Some(waker.clone());
                }
                Poll::Pending
            }
            OperationResult::Dropped => unreachable!("polling a dropped Future"),
            OperationResult::Done(res) => {
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

    /// Returns true if the operation is done.
    pub(crate) fn is_done(&self) -> bool {
        matches!(self.result, OperationResult::Done(_))
    }

    /// Set the state of the operation as dropped, but still in progress kernel
    /// side. This set the waker to `waker` and make `set_result` return `true`.
    pub(crate) fn set_dropped(&mut self, waker: task::Waker) {
        let res = replace(&mut self.result, OperationResult::Dropped);
        debug_assert!(matches!(res, OperationResult::InProgress));
        self.waker = Some(waker);
    }
}

/// Result of an operation.
#[derive(Copy, Clone, Debug)]
enum OperationResult {
    /// Operation is in progress, waiting on result.
    InProgress,
    /// The `Future` waiting for this operation has been dropped, but kernel
    /// side it's still in progress.
    Dropped,
    /// Operation done.
    ///
    /// Value is the result from the operation; negative is a (negative) errno,
    /// positive a successful result.
    Done(i32),
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

    /// Create a timeout submission waiting for at least one completion or
    /// triggers a timeout.
    ///
    /// Avaialable since Linux kernel 5.4.
    pub(crate) unsafe fn timeout(&mut self, ts: *const libc::timespec) {
        self.inner.opcode = libc::IORING_OP_TIMEOUT as u8;
        self.inner.fd = -1;
        self.inner.__bindgen_anon_1 = libc::io_uring_sqe__bindgen_ty_1 { off: 1 };
        self.inner.__bindgen_anon_2 = libc::io_uring_sqe__bindgen_ty_2 { addr: ts as _ };
        self.inner.len = 1;
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

    pub(crate) unsafe fn send(&mut self, fd: RawFd, ptr: *const u8, len: u32, flags: libc::c_int) {
        self.inner.opcode = libc::IORING_OP_SEND as u8;
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
}

impl fmt::Debug for Submission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_struct("Submission");
        d.field("opcode", &self.inner.opcode)
            .field("flags", &self.inner.flags)
            .field("ioprio", &self.inner.ioprio)
            .field("fd", &self.inner.fd);
        match self.inner.opcode as i32 {
            libc::IORING_OP_READ => {
                d.field("off", unsafe { &self.inner.__bindgen_anon_1.off })
                    .field("addr", unsafe {
                        &(self.inner.__bindgen_anon_2.addr as *const libc::c_void)
                    });
            }
            _ => { /* TODO. */ }
        }
        d.field("len", &self.inner.len);
        match self.inner.opcode as i32 {
            libc::IORING_OP_READ => {
                d.field("rw_flags", unsafe { &self.inner.__bindgen_anon_3.rw_flags });
            }
            _ => { /* TODO. */ }
        }
        d.field("user_data", &self.inner.user_data).finish()
    }
}

/// Macro to create an operation [`Future`] structure.
///
/// [`Future`]: std::future::Future
macro_rules! op_future {
    (
        // File type and function name.
        fn $f: ident :: $fn: ident -> $result: ty,
        // Future structure.
        struct $name: ident < $lifetime: lifetime $(, $generic: ident: $($trait: ident)? )* > {
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
        extract: |$extract_self: ident, $extract_arg: ident| -> $extract_result: ty $extract_map: block,
    ) => {
        op_future!{
            fn $f::$fn -> $result,
            struct $name<$lifetime $(, $generic: $($trait)? )*> {
                $(
                $(#[$field_doc])*
                $field: $value, $drop_msg,
                )?
            },
            |$self, $arg| $map_result,
        }

        impl<$lifetime $(, $generic: std::marker::Unpin $(+ $trait)? )*> $crate::Extract for $name<$lifetime $(, $generic)*> {}

        impl<$lifetime $(, $generic: std::marker::Unpin $(+ $trait)? )*> std::future::Future for $crate::extract::Extractor<$name<$lifetime $(, $generic)*>> {
            type Output = std::io::Result<$extract_result>;

            fn poll(mut self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
                match self.fut.fd.sq.poll_op(ctx, self.fut.op_index) {
                    std::task::Poll::Ready(std::result::Result::Ok($extract_arg)) => std::task::Poll::Ready({
                        let $extract_self = &mut self.fut;
                        $extract_map
                    }),
                    std::task::Poll::Ready(std::result::Result::Err(err)) => {
                        $( drop(self.fut.$field.take()); )?
                        std::task::Poll::Ready(std::result::Result::Err(err))
                    },
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }
        }
    };
    // Base version (without any additional implementations).
    (
        fn $f: ident :: $fn: ident -> $result: ty,
        struct $name: ident < $lifetime: lifetime $(, $generic: ident: $($trait: ident)? )* > {
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty, $drop_msg: expr,
            )?
        },
        |$self: ident, $arg: ident| $map_result: expr,
    ) => {
        #[doc = concat!("[`Future`](std::future::Future) behind [`", stringify!($f), "::", stringify!($fn), "`].")]
        #[derive(Debug)]
        pub struct $name<$lifetime $(, $generic)*> {
            $(
            $(#[ $field_doc ])*
            $field: std::option::Option<std::cell::UnsafeCell<$value>>,
            )?
            fd: &$lifetime $f,
            /// Index for the queued operation.
            op_index: $crate::OpIndex,
        }

        impl<$lifetime $(, $generic: std::marker::Unpin $(+ $trait)? )*> std::future::Future for $name<$lifetime $(, $generic)*> {
            type Output = std::io::Result<$result>;

            fn poll(mut self: std::pin::Pin<&mut Self>, ctx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
                match self.fd.sq.poll_op(ctx, self.op_index) {
                    std::task::Poll::Ready(std::result::Result::Ok($arg)) => std::task::Poll::Ready({
                        let $self = &mut self;
                        $map_result
                    }),
                    std::task::Poll::Ready(std::result::Result::Err(err)) => {
                        $( drop(self.$field.take()); )?
                        std::task::Poll::Ready(std::result::Result::Err(err))
                    },
                    std::task::Poll::Pending => std::task::Poll::Pending,
                }
            }
        }

        impl<$lifetime $(, $generic)*> std::ops::Drop for $name<$lifetime $(, $generic)*> {
            fn drop(&mut self) {
                $(
                if let Some($field) = self.$field.take() {
                    self.fd.sq.drop_op(self.op_index, $field);
                }
                )?
            }
        }
    };
    // Version that doesn't need `self` (this) in `$map_result`.
    (
        fn $f: ident :: $fn: ident -> $result: ty,
        struct $name: ident < $lifetime: lifetime $(, $generic: ident: $($trait: ident)? )* > {
            $(
            $(#[ $field_doc: meta ])*
            $field: ident : $value: ty, $drop_msg: expr,
            )?
        },
        |$arg: ident| $map_result: expr, // Only difference: 1 argument.
        $( extract: |$extract_self: ident, $extract_arg: ident| -> $extract_result: ty $extract_map: block, )?
    ) => {
        op_future!{
            fn $f::$fn -> $result,
            struct $name<$lifetime $(, $generic: $($trait)? )*> {
                $(
                $(#[$field_doc])*
                $field: $value, $drop_msg,
                )?
            },
            |_unused_this, $arg| $map_result,
            $( extract: |$extract_self, $extract_arg| -> $extract_result $extract_map, )?
        }
    };
}

pub(crate) use op_future;

#[test]
fn size_assertion() {
    assert_eq!(std::mem::size_of::<QueuedOperation>(), 24);
    assert_eq!(std::mem::size_of::<Option<QueuedOperation>>(), 24);
    assert_eq!(std::mem::size_of::<OperationResult>(), 8);
}
