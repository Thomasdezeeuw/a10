//! Code that should be moved to libc once C libraries have a wrapper.

#![allow(dead_code, non_camel_case_types, non_snake_case)]
#![allow(
    clippy::cast_lossless,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::missing_const_for_fn,
    clippy::missing_safety_doc,
    clippy::ptr_as_ptr,
    clippy::unreadable_literal
)]

/// Helper macro to execute a system call that returns an `io::Result`.
macro_rules! syscall {
    ($fn: ident ( $($arg: expr),* $(,)? ) ) => {{
        let res = unsafe { libc::$fn($( $arg, )*) };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}

pub use libc::*;
pub use syscall;

pub unsafe fn io_uring_setup(entries: c_uint, p: *mut io_uring_params) -> c_int {
    syscall(SYS_io_uring_setup, entries as c_long, p as c_long) as _
}

pub unsafe fn io_uring_register(
    fd: c_int,
    opcode: c_uint,
    arg: *const c_void,
    nr_args: c_uint,
) -> c_int {
    syscall(
        SYS_io_uring_register,
        fd as c_long,
        opcode as c_long,
        arg as c_long,
        nr_args as c_long,
    ) as _
}

pub unsafe fn io_uring_enter2(
    fd: c_int,
    to_submit: c_uint,
    min_complete: c_uint,
    flags: c_uint,
    arg: *const libc::c_void,
    size: usize,
) -> c_int {
    syscall(
        SYS_io_uring_enter,
        fd as c_long,
        to_submit as c_long,
        min_complete as c_long,
        flags as c_long,
        arg as c_long,
        size as c_long,
    ) as _
}

// Work around for <https://github.com/rust-lang/rust-bindgen/issues/1642>,
// <https://github.com/rust-lang/rust-bindgen/issues/258>.
pub const IOSQE_FIXED_FILE: u8 = 1 << IOSQE_FIXED_FILE_BIT as u8;
pub const IOSQE_IO_DRAIN: u8 = 1 << IOSQE_IO_DRAIN_BIT as u8;
pub const IOSQE_IO_LINK: u8 = 1 << IOSQE_IO_LINK_BIT as u8;
pub const IOSQE_IO_HARDLINK: u8 = 1 << IOSQE_IO_HARDLINK_BIT as u8;
pub const IOSQE_ASYNC: u8 = 1 << IOSQE_ASYNC_BIT as u8;
pub const IOSQE_BUFFER_SELECT: u8 = 1 << IOSQE_BUFFER_SELECT_BIT as u8;
pub const IOSQE_CQE_SKIP_SUCCESS: u8 = 1 << IOSQE_CQE_SKIP_SUCCESS_BIT as u8;

pub type __kernel_time64_t = ::std::os::raw::c_longlong;
pub type __u8 = ::std::os::raw::c_uchar;
pub type __u16 = ::std::os::raw::c_ushort;
pub type __s32 = ::std::os::raw::c_int;
pub type __u32 = ::std::os::raw::c_uint;
pub type __u64 = ::std::os::raw::c_ulonglong;
pub type __kernel_rwf_t = ::std::os::raw::c_int;
pub type _bindgen_ty_18 = ::std::os::raw::c_uint;
pub type io_uring_op = ::std::os::raw::c_uint;
pub type _bindgen_ty_19 = ::std::os::raw::c_uint;
pub type _bindgen_ty_20 = ::std::os::raw::c_uint;
pub type _bindgen_ty_21 = ::std::os::raw::c_uint;
pub type _bindgen_ty_23 = ::std::os::raw::c_uint;
#[repr(C)]
#[derive(Default)]
pub struct __IncompleteArrayField<T>(::std::marker::PhantomData<T>, [T; 0]);
#[repr(C)]
#[derive(Copy, Clone)]
pub struct __kernel_timespec {
    pub tv_sec: __kernel_time64_t,
    pub tv_nsec: ::std::os::raw::c_longlong,
}
#[repr(C)]
pub struct io_uring_sqe {
    pub opcode: __u8,
    pub flags: __u8,
    pub ioprio: __u16,
    pub fd: __s32,
    pub __bindgen_anon_1: io_uring_sqe__bindgen_ty_1,
    pub __bindgen_anon_2: io_uring_sqe__bindgen_ty_2,
    pub len: __u32,
    pub __bindgen_anon_3: io_uring_sqe__bindgen_ty_3,
    pub user_data: __u64,
    pub __bindgen_anon_4: io_uring_sqe__bindgen_ty_4,
    pub personality: __u16,
    pub __bindgen_anon_5: io_uring_sqe__bindgen_ty_5,
    pub __bindgen_anon_6: io_uring_sqe__bindgen_ty_6,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_sqe__bindgen_ty_1__bindgen_ty_1 {
    pub cmd_op: __u32,
    pub __pad1: __u32,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_sqe__bindgen_ty_5__bindgen_ty_1 {
    pub addr_len: __u16,
    pub __pad3: [__u16; 1usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_sqe__bindgen_ty_6__bindgen_ty_1 {
    pub addr3: __u64,
    pub __pad2: [__u64; 1usize],
}
#[repr(C)]
pub struct io_uring_cqe {
    pub user_data: __u64,
    pub res: __s32,
    pub flags: __u32,
    pub big_cqe: __IncompleteArrayField<__u64>,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_sqring_offsets {
    pub head: __u32,
    pub tail: __u32,
    pub ring_mask: __u32,
    pub ring_entries: __u32,
    pub flags: __u32,
    pub dropped: __u32,
    pub array: __u32,
    pub resv1: __u32,
    pub resv2: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_cqring_offsets {
    pub head: __u32,
    pub tail: __u32,
    pub ring_mask: __u32,
    pub ring_entries: __u32,
    pub overflow: __u32,
    pub cqes: __u32,
    pub flags: __u32,
    pub resv1: __u32,
    pub resv2: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_params {
    pub sq_entries: __u32,
    pub cq_entries: __u32,
    pub flags: __u32,
    pub sq_thread_cpu: __u32,
    pub sq_thread_idle: __u32,
    pub features: __u32,
    pub wq_fd: __u32,
    pub resv: [__u32; 3usize],
    pub sq_off: io_sqring_offsets,
    pub cq_off: io_cqring_offsets,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_files_update {
    pub offset: __u32,
    pub resv: __u32,
    pub fds: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_rsrc_register {
    pub nr: __u32,
    pub flags: __u32,
    pub resv2: __u64,
    pub data: __u64,
    pub tags: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_rsrc_update {
    pub offset: __u32,
    pub resv: __u32,
    pub data: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_rsrc_update2 {
    pub offset: __u32,
    pub resv: __u32,
    pub data: __u64,
    pub tags: __u64,
    pub nr: __u32,
    pub resv2: __u32,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_notification_slot {
    pub tag: __u64,
    pub resv: [__u64; 3usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_notification_register {
    pub nr_slots: __u32,
    pub resv: __u32,
    pub resv2: __u64,
    pub data: __u64,
    pub resv3: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_probe_op {
    pub op: __u8,
    pub resv: __u8,
    pub flags: __u16,
    pub resv2: __u32,
}
#[repr(C)]
pub struct io_uring_probe {
    pub last_op: __u8,
    pub ops_len: __u8,
    pub resv: __u16,
    pub resv2: [__u32; 3usize],
    pub ops: __IncompleteArrayField<io_uring_probe_op>,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_restriction {
    pub opcode: __u16,
    pub __bindgen_anon_1: io_uring_restriction__bindgen_ty_1,
    pub resv: __u8,
    pub resv2: [__u32; 3usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_buf {
    pub addr: __u64,
    pub len: __u32,
    pub bid: __u16,
    pub resv: __u16,
}
#[repr(C)]
pub struct io_uring_buf_ring {
    pub __bindgen_anon_1: io_uring_buf_ring__bindgen_ty_1,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1 {
    pub resv1: __u64,
    pub resv2: __u32,
    pub resv3: __u16,
    pub tail: __u16,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_buf_reg {
    pub ring_addr: __u64,
    pub ring_entries: __u32,
    pub bgid: __u16,
    pub pad: __u16,
    pub resv: [__u64; 3usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_getevents_arg {
    pub sigmask: __u64,
    pub sigmask_sz: __u32,
    pub pad: __u32,
    pub ts: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_sync_cancel_reg {
    pub addr: __u64,
    pub fd: __s32,
    pub flags: __u32,
    pub timeout: __kernel_timespec,
    pub pad: [__u64; 4usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_file_index_range {
    pub off: __u32,
    pub len: __u32,
    pub resv: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_recvmsg_out {
    pub namelen: __u32,
    pub controllen: __u32,
    pub payloadlen: __u32,
    pub flags: __u32,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_sq {
    pub khead: *mut ::std::os::raw::c_uint,
    pub ktail: *mut ::std::os::raw::c_uint,
    pub kring_mask: *mut ::std::os::raw::c_uint,
    pub kring_entries: *mut ::std::os::raw::c_uint,
    pub kflags: *mut ::std::os::raw::c_uint,
    pub kdropped: *mut ::std::os::raw::c_uint,
    pub array: *mut ::std::os::raw::c_uint,
    pub sqes: *mut io_uring_sqe,
    pub sqe_head: ::std::os::raw::c_uint,
    pub sqe_tail: ::std::os::raw::c_uint,
    pub ring_sz: usize,
    pub ring_ptr: *mut ::std::os::raw::c_void,
    pub ring_mask: ::std::os::raw::c_uint,
    pub ring_entries: ::std::os::raw::c_uint,
    pub pad: [::std::os::raw::c_uint; 2usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_cq {
    pub khead: *mut ::std::os::raw::c_uint,
    pub ktail: *mut ::std::os::raw::c_uint,
    pub kring_mask: *mut ::std::os::raw::c_uint,
    pub kring_entries: *mut ::std::os::raw::c_uint,
    pub kflags: *mut ::std::os::raw::c_uint,
    pub koverflow: *mut ::std::os::raw::c_uint,
    pub cqes: *mut io_uring_cqe,
    pub ring_sz: usize,
    pub ring_ptr: *mut ::std::os::raw::c_void,
    pub ring_mask: ::std::os::raw::c_uint,
    pub ring_entries: ::std::os::raw::c_uint,
    pub pad: [::std::os::raw::c_uint; 2usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring {
    pub sq: io_uring_sq,
    pub cq: io_uring_cq,
    pub flags: ::std::os::raw::c_uint,
    pub ring_fd: ::std::os::raw::c_int,
    pub features: ::std::os::raw::c_uint,
    pub enter_ring_fd: ::std::os::raw::c_int,
    pub int_flags: __u8,
    pub pad: [__u8; 3usize],
    pub pad2: ::std::os::raw::c_uint,
}
pub const IORING_FILE_INDEX_ALLOC: i32 = -1;
pub const IORING_SETUP_IOPOLL: u32 = 1;
pub const IORING_SETUP_SQPOLL: u32 = 2;
pub const IORING_SETUP_SQ_AFF: u32 = 4;
pub const IORING_SETUP_CQSIZE: u32 = 8;
pub const IORING_SETUP_CLAMP: u32 = 16;
pub const IORING_SETUP_ATTACH_WQ: u32 = 32;
pub const IORING_SETUP_R_DISABLED: u32 = 64;
pub const IORING_SETUP_SUBMIT_ALL: u32 = 128;
pub const IORING_SETUP_COOP_TASKRUN: u32 = 256;
pub const IORING_SETUP_TASKRUN_FLAG: u32 = 512;
pub const IORING_SETUP_SQE128: u32 = 1024;
pub const IORING_SETUP_CQE32: u32 = 2048;
pub const IORING_SETUP_SINGLE_ISSUER: u32 = 4096;
pub const IORING_SETUP_DEFER_TASKRUN: u32 = 8192;
pub const IORING_URING_CMD_FIXED: u32 = 1;
pub const IORING_FSYNC_DATASYNC: u32 = 1;
pub const IORING_TIMEOUT_ABS: u32 = 1;
pub const IORING_TIMEOUT_UPDATE: u32 = 2;
pub const IORING_TIMEOUT_BOOTTIME: u32 = 4;
pub const IORING_TIMEOUT_REALTIME: u32 = 8;
pub const IORING_LINK_TIMEOUT_UPDATE: u32 = 16;
pub const IORING_TIMEOUT_ETIME_SUCCESS: u32 = 32;
pub const IORING_TIMEOUT_CLOCK_MASK: u32 = 12;
pub const IORING_TIMEOUT_UPDATE_MASK: u32 = 18;
pub const IORING_POLL_ADD_MULTI: u32 = 1;
pub const IORING_POLL_UPDATE_EVENTS: u32 = 2;
pub const IORING_POLL_UPDATE_USER_DATA: u32 = 4;
pub const IORING_POLL_ADD_LEVEL: u32 = 8;
pub const IORING_ASYNC_CANCEL_ALL: u32 = 1;
pub const IORING_ASYNC_CANCEL_FD: u32 = 2;
pub const IORING_ASYNC_CANCEL_ANY: u32 = 4;
pub const IORING_ASYNC_CANCEL_FD_FIXED: u32 = 8;
pub const IORING_RECVSEND_POLL_FIRST: u32 = 1;
pub const IORING_RECV_MULTISHOT: u32 = 2;
pub const IORING_RECVSEND_FIXED_BUF: u32 = 4;
pub const IORING_SEND_ZC_REPORT_USAGE: u32 = 8;
pub const IORING_NOTIF_USAGE_ZC_COPIED: u32 = 2147483648;
pub const IORING_ACCEPT_MULTISHOT: u32 = 1;
pub const IORING_MSG_RING_CQE_SKIP: u32 = 1;
pub const IORING_CQE_F_BUFFER: u32 = 1;
pub const IORING_CQE_F_MORE: u32 = 2;
pub const IORING_CQE_F_SOCK_NONEMPTY: u32 = 4;
pub const IORING_CQE_F_NOTIF: u32 = 8;
pub const IORING_OFF_SQ_RING: u32 = 0;
pub const IORING_OFF_CQ_RING: u32 = 134217728;
pub const IORING_OFF_SQES: u32 = 268435456;
pub const IORING_SQ_NEED_WAKEUP: u32 = 1;
pub const IORING_SQ_CQ_OVERFLOW: u32 = 2;
pub const IORING_SQ_TASKRUN: u32 = 4;
pub const IORING_CQ_EVENTFD_DISABLED: u32 = 1;
pub const IORING_ENTER_GETEVENTS: u32 = 1;
pub const IORING_ENTER_SQ_WAKEUP: u32 = 2;
pub const IORING_ENTER_SQ_WAIT: u32 = 4;
pub const IORING_ENTER_EXT_ARG: u32 = 8;
pub const IORING_ENTER_REGISTERED_RING: u32 = 16;
pub const IORING_FEAT_SINGLE_MMAP: u32 = 1;
pub const IORING_FEAT_NODROP: u32 = 2;
pub const IORING_FEAT_SUBMIT_STABLE: u32 = 4;
pub const IORING_FEAT_RW_CUR_POS: u32 = 8;
pub const IORING_FEAT_CUR_PERSONALITY: u32 = 16;
pub const IORING_FEAT_FAST_POLL: u32 = 32;
pub const IORING_FEAT_POLL_32BITS: u32 = 64;
pub const IORING_FEAT_SQPOLL_NONFIXED: u32 = 128;
pub const IORING_FEAT_EXT_ARG: u32 = 256;
pub const IORING_FEAT_NATIVE_WORKERS: u32 = 512;
pub const IORING_FEAT_RSRC_TAGS: u32 = 1024;
pub const IORING_FEAT_CQE_SKIP: u32 = 2048;
pub const IORING_FEAT_LINKED_FILE: u32 = 4096;
pub const IORING_RSRC_REGISTER_SPARSE: u32 = 1;
pub const IORING_REGISTER_FILES_SKIP: i32 = -2;
pub const IOSQE_FIXED_FILE_BIT: _bindgen_ty_18 = 0;
pub const IOSQE_IO_DRAIN_BIT: _bindgen_ty_18 = 1;
pub const IOSQE_IO_LINK_BIT: _bindgen_ty_18 = 2;
pub const IOSQE_IO_HARDLINK_BIT: _bindgen_ty_18 = 3;
pub const IOSQE_ASYNC_BIT: _bindgen_ty_18 = 4;
pub const IOSQE_BUFFER_SELECT_BIT: _bindgen_ty_18 = 5;
pub const IOSQE_CQE_SKIP_SUCCESS_BIT: _bindgen_ty_18 = 6;
pub const IORING_OP_NOP: io_uring_op = 0;
pub const IORING_OP_READV: io_uring_op = 1;
pub const IORING_OP_WRITEV: io_uring_op = 2;
pub const IORING_OP_FSYNC: io_uring_op = 3;
pub const IORING_OP_READ_FIXED: io_uring_op = 4;
pub const IORING_OP_WRITE_FIXED: io_uring_op = 5;
pub const IORING_OP_POLL_ADD: io_uring_op = 6;
pub const IORING_OP_POLL_REMOVE: io_uring_op = 7;
pub const IORING_OP_SYNC_FILE_RANGE: io_uring_op = 8;
pub const IORING_OP_SENDMSG: io_uring_op = 9;
pub const IORING_OP_RECVMSG: io_uring_op = 10;
pub const IORING_OP_TIMEOUT: io_uring_op = 11;
pub const IORING_OP_TIMEOUT_REMOVE: io_uring_op = 12;
pub const IORING_OP_ACCEPT: io_uring_op = 13;
pub const IORING_OP_ASYNC_CANCEL: io_uring_op = 14;
pub const IORING_OP_LINK_TIMEOUT: io_uring_op = 15;
pub const IORING_OP_CONNECT: io_uring_op = 16;
pub const IORING_OP_FALLOCATE: io_uring_op = 17;
pub const IORING_OP_OPENAT: io_uring_op = 18;
pub const IORING_OP_CLOSE: io_uring_op = 19;
pub const IORING_OP_FILES_UPDATE: io_uring_op = 20;
pub const IORING_OP_STATX: io_uring_op = 21;
pub const IORING_OP_READ: io_uring_op = 22;
pub const IORING_OP_WRITE: io_uring_op = 23;
pub const IORING_OP_FADVISE: io_uring_op = 24;
pub const IORING_OP_MADVISE: io_uring_op = 25;
pub const IORING_OP_SEND: io_uring_op = 26;
pub const IORING_OP_RECV: io_uring_op = 27;
pub const IORING_OP_OPENAT2: io_uring_op = 28;
pub const IORING_OP_EPOLL_CTL: io_uring_op = 29;
pub const IORING_OP_SPLICE: io_uring_op = 30;
pub const IORING_OP_PROVIDE_BUFFERS: io_uring_op = 31;
pub const IORING_OP_REMOVE_BUFFERS: io_uring_op = 32;
pub const IORING_OP_TEE: io_uring_op = 33;
pub const IORING_OP_SHUTDOWN: io_uring_op = 34;
pub const IORING_OP_RENAMEAT: io_uring_op = 35;
pub const IORING_OP_UNLINKAT: io_uring_op = 36;
pub const IORING_OP_MKDIRAT: io_uring_op = 37;
pub const IORING_OP_SYMLINKAT: io_uring_op = 38;
pub const IORING_OP_LINKAT: io_uring_op = 39;
pub const IORING_OP_MSG_RING: io_uring_op = 40;
pub const IORING_OP_FSETXATTR: io_uring_op = 41;
pub const IORING_OP_SETXATTR: io_uring_op = 42;
pub const IORING_OP_FGETXATTR: io_uring_op = 43;
pub const IORING_OP_GETXATTR: io_uring_op = 44;
pub const IORING_OP_SOCKET: io_uring_op = 45;
pub const IORING_OP_URING_CMD: io_uring_op = 46;
pub const IORING_OP_SEND_ZC: io_uring_op = 47;
pub const IORING_OP_SENDMSG_ZC: io_uring_op = 48;
pub const IORING_OP_LAST: io_uring_op = 49;
pub const IORING_MSG_DATA: _bindgen_ty_19 = 0;
pub const IORING_MSG_SEND_FD: _bindgen_ty_19 = 1;
pub const IORING_CQE_BUFFER_SHIFT: _bindgen_ty_20 = 16;
pub const IORING_REGISTER_BUFFERS: _bindgen_ty_21 = 0;
pub const IORING_UNREGISTER_BUFFERS: _bindgen_ty_21 = 1;
pub const IORING_REGISTER_FILES: _bindgen_ty_21 = 2;
pub const IORING_UNREGISTER_FILES: _bindgen_ty_21 = 3;
pub const IORING_REGISTER_EVENTFD: _bindgen_ty_21 = 4;
pub const IORING_UNREGISTER_EVENTFD: _bindgen_ty_21 = 5;
pub const IORING_REGISTER_FILES_UPDATE: _bindgen_ty_21 = 6;
pub const IORING_REGISTER_EVENTFD_ASYNC: _bindgen_ty_21 = 7;
pub const IORING_REGISTER_PROBE: _bindgen_ty_21 = 8;
pub const IORING_REGISTER_PERSONALITY: _bindgen_ty_21 = 9;
pub const IORING_UNREGISTER_PERSONALITY: _bindgen_ty_21 = 10;
pub const IORING_REGISTER_RESTRICTIONS: _bindgen_ty_21 = 11;
pub const IORING_REGISTER_ENABLE_RINGS: _bindgen_ty_21 = 12;
pub const IORING_REGISTER_FILES2: _bindgen_ty_21 = 13;
pub const IORING_REGISTER_FILES_UPDATE2: _bindgen_ty_21 = 14;
pub const IORING_REGISTER_BUFFERS2: _bindgen_ty_21 = 15;
pub const IORING_REGISTER_BUFFERS_UPDATE: _bindgen_ty_21 = 16;
pub const IORING_REGISTER_IOWQ_AFF: _bindgen_ty_21 = 17;
pub const IORING_UNREGISTER_IOWQ_AFF: _bindgen_ty_21 = 18;
pub const IORING_REGISTER_IOWQ_MAX_WORKERS: _bindgen_ty_21 = 19;
pub const IORING_REGISTER_RING_FDS: _bindgen_ty_21 = 20;
pub const IORING_UNREGISTER_RING_FDS: _bindgen_ty_21 = 21;
pub const IORING_REGISTER_PBUF_RING: _bindgen_ty_21 = 22;
pub const IORING_UNREGISTER_PBUF_RING: _bindgen_ty_21 = 23;
pub const IORING_REGISTER_SYNC_CANCEL: _bindgen_ty_21 = 24;
pub const IORING_REGISTER_FILE_ALLOC_RANGE: _bindgen_ty_21 = 25;
pub const IORING_REGISTER_LAST: _bindgen_ty_21 = 26;
pub const IORING_RESTRICTION_REGISTER_OP: _bindgen_ty_23 = 0;
pub const IORING_RESTRICTION_SQE_OP: _bindgen_ty_23 = 1;
pub const IORING_RESTRICTION_SQE_FLAGS_ALLOWED: _bindgen_ty_23 = 2;
pub const IORING_RESTRICTION_SQE_FLAGS_REQUIRED: _bindgen_ty_23 = 3;
pub const IORING_RESTRICTION_LAST: _bindgen_ty_23 = 4;
#[test]
fn bindgen_test_layout___kernel_timespec() {
    const UNINIT: ::std::mem::MaybeUninit<__kernel_timespec> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<__kernel_timespec>(),
        16usize,
        concat!("Size of: ", stringify!(__kernel_timespec))
    );
    assert_eq!(
        ::std::mem::align_of::<__kernel_timespec>(),
        8usize,
        concat!("Alignment of ", stringify!(__kernel_timespec))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).tv_sec) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(__kernel_timespec),
            "::",
            stringify!(tv_sec)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).tv_nsec) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(__kernel_timespec),
            "::",
            stringify!(tv_nsec)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_1__bindgen_ty_1() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe__bindgen_ty_1__bindgen_ty_1> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe__bindgen_ty_1__bindgen_ty_1>(),
        8usize,
        concat!(
            "Size of: ",
            stringify!(io_uring_sqe__bindgen_ty_1__bindgen_ty_1)
        )
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe__bindgen_ty_1__bindgen_ty_1>(),
        4usize,
        concat!(
            "Alignment of ",
            stringify!(io_uring_sqe__bindgen_ty_1__bindgen_ty_1)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).cmd_op) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_1__bindgen_ty_1),
            "::",
            stringify!(cmd_op)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).__pad1) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_1__bindgen_ty_1),
            "::",
            stringify!(__pad1)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_1() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe__bindgen_ty_1> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe__bindgen_ty_1>(),
        8usize,
        concat!("Size of: ", stringify!(io_uring_sqe__bindgen_ty_1))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe__bindgen_ty_1>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_sqe__bindgen_ty_1))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).off) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_1),
            "::",
            stringify!(off)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).addr2) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_1),
            "::",
            stringify!(addr2)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_2() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe__bindgen_ty_2> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe__bindgen_ty_2>(),
        8usize,
        concat!("Size of: ", stringify!(io_uring_sqe__bindgen_ty_2))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe__bindgen_ty_2>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_sqe__bindgen_ty_2))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).addr) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_2),
            "::",
            stringify!(addr)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).splice_off_in) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_2),
            "::",
            stringify!(splice_off_in)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_3() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe__bindgen_ty_3> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe__bindgen_ty_3>(),
        4usize,
        concat!("Size of: ", stringify!(io_uring_sqe__bindgen_ty_3))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe__bindgen_ty_3>(),
        4usize,
        concat!("Alignment of ", stringify!(io_uring_sqe__bindgen_ty_3))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).rw_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(rw_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).fsync_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(fsync_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).poll_events) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(poll_events)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).poll32_events) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(poll32_events)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sync_range_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(sync_range_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).msg_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(msg_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).timeout_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(timeout_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).accept_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(accept_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).cancel_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(cancel_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).open_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(open_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).statx_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(statx_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).fadvise_advice) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(fadvise_advice)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).splice_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(splice_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).rename_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(rename_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).unlink_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(unlink_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).hardlink_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(hardlink_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).xattr_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(xattr_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).msg_ring_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(msg_ring_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).uring_cmd_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_3),
            "::",
            stringify!(uring_cmd_flags)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_4() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe__bindgen_ty_4> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe__bindgen_ty_4>(),
        2usize,
        concat!("Size of: ", stringify!(io_uring_sqe__bindgen_ty_4))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe__bindgen_ty_4>(),
        1usize,
        concat!("Alignment of ", stringify!(io_uring_sqe__bindgen_ty_4))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).buf_index) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_4),
            "::",
            stringify!(buf_index)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).buf_group) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_4),
            "::",
            stringify!(buf_group)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_5__bindgen_ty_1() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe__bindgen_ty_5__bindgen_ty_1> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe__bindgen_ty_5__bindgen_ty_1>(),
        4usize,
        concat!(
            "Size of: ",
            stringify!(io_uring_sqe__bindgen_ty_5__bindgen_ty_1)
        )
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe__bindgen_ty_5__bindgen_ty_1>(),
        2usize,
        concat!(
            "Alignment of ",
            stringify!(io_uring_sqe__bindgen_ty_5__bindgen_ty_1)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).addr_len) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_5__bindgen_ty_1),
            "::",
            stringify!(addr_len)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).__pad3) as usize - ptr as usize },
        2usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_5__bindgen_ty_1),
            "::",
            stringify!(__pad3)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_5() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe__bindgen_ty_5> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe__bindgen_ty_5>(),
        4usize,
        concat!("Size of: ", stringify!(io_uring_sqe__bindgen_ty_5))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe__bindgen_ty_5>(),
        4usize,
        concat!("Alignment of ", stringify!(io_uring_sqe__bindgen_ty_5))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).splice_fd_in) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_5),
            "::",
            stringify!(splice_fd_in)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).file_index) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_5),
            "::",
            stringify!(file_index)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_6__bindgen_ty_1() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe__bindgen_ty_6__bindgen_ty_1> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe__bindgen_ty_6__bindgen_ty_1>(),
        16usize,
        concat!(
            "Size of: ",
            stringify!(io_uring_sqe__bindgen_ty_6__bindgen_ty_1)
        )
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe__bindgen_ty_6__bindgen_ty_1>(),
        8usize,
        concat!(
            "Alignment of ",
            stringify!(io_uring_sqe__bindgen_ty_6__bindgen_ty_1)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).addr3) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_6__bindgen_ty_1),
            "::",
            stringify!(addr3)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).__pad2) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_6__bindgen_ty_1),
            "::",
            stringify!(__pad2)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_6() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe__bindgen_ty_6> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe__bindgen_ty_6>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_sqe__bindgen_ty_6))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe__bindgen_ty_6>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_sqe__bindgen_ty_6))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).cmd) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe__bindgen_ty_6),
            "::",
            stringify!(cmd)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sqe() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sqe> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sqe>(),
        64usize,
        concat!("Size of: ", stringify!(io_uring_sqe))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sqe>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_sqe))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).opcode) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe),
            "::",
            stringify!(opcode)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        1usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe),
            "::",
            stringify!(flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ioprio) as usize - ptr as usize },
        2usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe),
            "::",
            stringify!(ioprio)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).fd) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe),
            "::",
            stringify!(fd)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).len) as usize - ptr as usize },
        24usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe),
            "::",
            stringify!(len)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).user_data) as usize - ptr as usize },
        32usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe),
            "::",
            stringify!(user_data)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).personality) as usize - ptr as usize },
        42usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sqe),
            "::",
            stringify!(personality)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_cqe() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_cqe> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_cqe>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_cqe))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_cqe>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_cqe))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).user_data) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cqe),
            "::",
            stringify!(user_data)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).res) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cqe),
            "::",
            stringify!(res)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cqe),
            "::",
            stringify!(flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).big_cqe) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cqe),
            "::",
            stringify!(big_cqe)
        )
    );
}
#[test]
fn bindgen_test_layout_io_sqring_offsets() {
    const UNINIT: ::std::mem::MaybeUninit<io_sqring_offsets> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_sqring_offsets>(),
        40usize,
        concat!("Size of: ", stringify!(io_sqring_offsets))
    );
    assert_eq!(
        ::std::mem::align_of::<io_sqring_offsets>(),
        8usize,
        concat!("Alignment of ", stringify!(io_sqring_offsets))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).head) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_sqring_offsets),
            "::",
            stringify!(head)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).tail) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_sqring_offsets),
            "::",
            stringify!(tail)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_mask) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_sqring_offsets),
            "::",
            stringify!(ring_mask)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_entries) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_sqring_offsets),
            "::",
            stringify!(ring_entries)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_sqring_offsets),
            "::",
            stringify!(flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).dropped) as usize - ptr as usize },
        20usize,
        concat!(
            "Offset of field: ",
            stringify!(io_sqring_offsets),
            "::",
            stringify!(dropped)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).array) as usize - ptr as usize },
        24usize,
        concat!(
            "Offset of field: ",
            stringify!(io_sqring_offsets),
            "::",
            stringify!(array)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv1) as usize - ptr as usize },
        28usize,
        concat!(
            "Offset of field: ",
            stringify!(io_sqring_offsets),
            "::",
            stringify!(resv1)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize },
        32usize,
        concat!(
            "Offset of field: ",
            stringify!(io_sqring_offsets),
            "::",
            stringify!(resv2)
        )
    );
}
#[test]
fn bindgen_test_layout_io_cqring_offsets() {
    const UNINIT: ::std::mem::MaybeUninit<io_cqring_offsets> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_cqring_offsets>(),
        40usize,
        concat!("Size of: ", stringify!(io_cqring_offsets))
    );
    assert_eq!(
        ::std::mem::align_of::<io_cqring_offsets>(),
        8usize,
        concat!("Alignment of ", stringify!(io_cqring_offsets))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).head) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_cqring_offsets),
            "::",
            stringify!(head)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).tail) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_cqring_offsets),
            "::",
            stringify!(tail)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_mask) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_cqring_offsets),
            "::",
            stringify!(ring_mask)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_entries) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_cqring_offsets),
            "::",
            stringify!(ring_entries)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).overflow) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_cqring_offsets),
            "::",
            stringify!(overflow)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).cqes) as usize - ptr as usize },
        20usize,
        concat!(
            "Offset of field: ",
            stringify!(io_cqring_offsets),
            "::",
            stringify!(cqes)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        24usize,
        concat!(
            "Offset of field: ",
            stringify!(io_cqring_offsets),
            "::",
            stringify!(flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv1) as usize - ptr as usize },
        28usize,
        concat!(
            "Offset of field: ",
            stringify!(io_cqring_offsets),
            "::",
            stringify!(resv1)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize },
        32usize,
        concat!(
            "Offset of field: ",
            stringify!(io_cqring_offsets),
            "::",
            stringify!(resv2)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_params() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_params> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_params>(),
        120usize,
        concat!("Size of: ", stringify!(io_uring_params))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_params>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_params))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sq_entries) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(sq_entries)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).cq_entries) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(cq_entries)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sq_thread_cpu) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(sq_thread_cpu)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sq_thread_idle) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(sq_thread_idle)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).features) as usize - ptr as usize },
        20usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(features)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).wq_fd) as usize - ptr as usize },
        24usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(wq_fd)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        28usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(resv)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sq_off) as usize - ptr as usize },
        40usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(sq_off)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).cq_off) as usize - ptr as usize },
        80usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_params),
            "::",
            stringify!(cq_off)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_files_update() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_files_update> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_files_update>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_files_update))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_files_update>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_files_update))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).offset) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_files_update),
            "::",
            stringify!(offset)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_files_update),
            "::",
            stringify!(resv)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).fds) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_files_update),
            "::",
            stringify!(fds)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_rsrc_register() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_rsrc_register> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_rsrc_register>(),
        32usize,
        concat!("Size of: ", stringify!(io_uring_rsrc_register))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_rsrc_register>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_rsrc_register))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).nr) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_register),
            "::",
            stringify!(nr)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_register),
            "::",
            stringify!(flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_register),
            "::",
            stringify!(resv2)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).data) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_register),
            "::",
            stringify!(data)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).tags) as usize - ptr as usize },
        24usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_register),
            "::",
            stringify!(tags)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_rsrc_update() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_rsrc_update> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_rsrc_update>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_rsrc_update))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_rsrc_update>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_rsrc_update))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).offset) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_update),
            "::",
            stringify!(offset)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_update),
            "::",
            stringify!(resv)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).data) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_update),
            "::",
            stringify!(data)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_rsrc_update2() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_rsrc_update2> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_rsrc_update2>(),
        32usize,
        concat!("Size of: ", stringify!(io_uring_rsrc_update2))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_rsrc_update2>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_rsrc_update2))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).offset) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_update2),
            "::",
            stringify!(offset)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_update2),
            "::",
            stringify!(resv)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).data) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_update2),
            "::",
            stringify!(data)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).tags) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_update2),
            "::",
            stringify!(tags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).nr) as usize - ptr as usize },
        24usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_update2),
            "::",
            stringify!(nr)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize },
        28usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_rsrc_update2),
            "::",
            stringify!(resv2)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_notification_slot() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_notification_slot> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_notification_slot>(),
        32usize,
        concat!("Size of: ", stringify!(io_uring_notification_slot))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_notification_slot>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_notification_slot))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).tag) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_notification_slot),
            "::",
            stringify!(tag)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_notification_slot),
            "::",
            stringify!(resv)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_notification_register() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_notification_register> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_notification_register>(),
        32usize,
        concat!("Size of: ", stringify!(io_uring_notification_register))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_notification_register>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_notification_register))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).nr_slots) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_notification_register),
            "::",
            stringify!(nr_slots)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_notification_register),
            "::",
            stringify!(resv)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_notification_register),
            "::",
            stringify!(resv2)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).data) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_notification_register),
            "::",
            stringify!(data)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv3) as usize - ptr as usize },
        24usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_notification_register),
            "::",
            stringify!(resv3)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_probe_op() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_probe_op> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_probe_op>(),
        8usize,
        concat!("Size of: ", stringify!(io_uring_probe_op))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_probe_op>(),
        4usize,
        concat!("Alignment of ", stringify!(io_uring_probe_op))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).op) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_probe_op),
            "::",
            stringify!(op)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        1usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_probe_op),
            "::",
            stringify!(resv)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        2usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_probe_op),
            "::",
            stringify!(flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_probe_op),
            "::",
            stringify!(resv2)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_probe() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_probe> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_probe>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_probe))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_probe>(),
        4usize,
        concat!("Alignment of ", stringify!(io_uring_probe))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).last_op) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_probe),
            "::",
            stringify!(last_op)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ops_len) as usize - ptr as usize },
        1usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_probe),
            "::",
            stringify!(ops_len)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        2usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_probe),
            "::",
            stringify!(resv)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_probe),
            "::",
            stringify!(resv2)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ops) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_probe),
            "::",
            stringify!(ops)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_restriction__bindgen_ty_1() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_restriction__bindgen_ty_1> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_restriction__bindgen_ty_1>(),
        1usize,
        concat!("Size of: ", stringify!(io_uring_restriction__bindgen_ty_1))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_restriction__bindgen_ty_1>(),
        1usize,
        concat!(
            "Alignment of ",
            stringify!(io_uring_restriction__bindgen_ty_1)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).register_op) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_restriction__bindgen_ty_1),
            "::",
            stringify!(register_op)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sqe_op) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_restriction__bindgen_ty_1),
            "::",
            stringify!(sqe_op)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sqe_flags) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_restriction__bindgen_ty_1),
            "::",
            stringify!(sqe_flags)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_restriction() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_restriction> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_restriction>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_restriction))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_restriction>(),
        4usize,
        concat!("Alignment of ", stringify!(io_uring_restriction))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).opcode) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_restriction),
            "::",
            stringify!(opcode)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        3usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_restriction),
            "::",
            stringify!(resv)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_restriction),
            "::",
            stringify!(resv2)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_buf() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_buf> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_buf>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_buf))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_buf>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_buf))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).addr) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf),
            "::",
            stringify!(addr)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).len) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf),
            "::",
            stringify!(len)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).bid) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf),
            "::",
            stringify!(bid)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        14usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf),
            "::",
            stringify!(resv)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1>(),
        16usize,
        concat!(
            "Size of: ",
            stringify!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1)
        )
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1>(),
        8usize,
        concat!(
            "Alignment of ",
            stringify!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv1) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1),
            "::",
            stringify!(resv1)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1),
            "::",
            stringify!(resv2)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv3) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1),
            "::",
            stringify!(resv3)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).tail) as usize - ptr as usize },
        14usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1),
            "::",
            stringify!(tail)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_buf_ring__bindgen_ty_1() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_buf_ring__bindgen_ty_1> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_buf_ring__bindgen_ty_1>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_buf_ring__bindgen_ty_1))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_buf_ring__bindgen_ty_1>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_buf_ring__bindgen_ty_1))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).bufs) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_ring__bindgen_ty_1),
            "::",
            stringify!(bufs)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_buf_ring() {
    assert_eq!(
        ::std::mem::size_of::<io_uring_buf_ring>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_buf_ring))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_buf_ring>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_buf_ring))
    );
}
#[test]
fn bindgen_test_layout_io_uring_buf_reg() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_buf_reg> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_buf_reg>(),
        40usize,
        concat!("Size of: ", stringify!(io_uring_buf_reg))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_buf_reg>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_buf_reg))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_addr) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_reg),
            "::",
            stringify!(ring_addr)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_entries) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_reg),
            "::",
            stringify!(ring_entries)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).bgid) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_reg),
            "::",
            stringify!(bgid)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).pad) as usize - ptr as usize },
        14usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_reg),
            "::",
            stringify!(pad)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_buf_reg),
            "::",
            stringify!(resv)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_getevents_arg() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_getevents_arg> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_getevents_arg>(),
        24usize,
        concat!("Size of: ", stringify!(io_uring_getevents_arg))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_getevents_arg>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_getevents_arg))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sigmask) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_getevents_arg),
            "::",
            stringify!(sigmask)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sigmask_sz) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_getevents_arg),
            "::",
            stringify!(sigmask_sz)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).pad) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_getevents_arg),
            "::",
            stringify!(pad)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ts) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_getevents_arg),
            "::",
            stringify!(ts)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sync_cancel_reg() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sync_cancel_reg> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sync_cancel_reg>(),
        64usize,
        concat!("Size of: ", stringify!(io_uring_sync_cancel_reg))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sync_cancel_reg>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_sync_cancel_reg))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).addr) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sync_cancel_reg),
            "::",
            stringify!(addr)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).fd) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sync_cancel_reg),
            "::",
            stringify!(fd)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sync_cancel_reg),
            "::",
            stringify!(flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).timeout) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sync_cancel_reg),
            "::",
            stringify!(timeout)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).pad) as usize - ptr as usize },
        32usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sync_cancel_reg),
            "::",
            stringify!(pad)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_file_index_range() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_file_index_range> =
        ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_file_index_range>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_file_index_range))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_file_index_range>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_file_index_range))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).off) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_file_index_range),
            "::",
            stringify!(off)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).len) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_file_index_range),
            "::",
            stringify!(len)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_file_index_range),
            "::",
            stringify!(resv)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_recvmsg_out() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_recvmsg_out> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_recvmsg_out>(),
        16usize,
        concat!("Size of: ", stringify!(io_uring_recvmsg_out))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_recvmsg_out>(),
        4usize,
        concat!("Alignment of ", stringify!(io_uring_recvmsg_out))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).namelen) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_recvmsg_out),
            "::",
            stringify!(namelen)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).controllen) as usize - ptr as usize },
        4usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_recvmsg_out),
            "::",
            stringify!(controllen)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).payloadlen) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_recvmsg_out),
            "::",
            stringify!(payloadlen)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        12usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_recvmsg_out),
            "::",
            stringify!(flags)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_sq() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_sq> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_sq>(),
        104usize,
        concat!("Size of: ", stringify!(io_uring_sq))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_sq>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_sq))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).khead) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(khead)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ktail) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(ktail)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).kring_mask) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(kring_mask)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).kring_entries) as usize - ptr as usize },
        24usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(kring_entries)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).kflags) as usize - ptr as usize },
        32usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(kflags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).kdropped) as usize - ptr as usize },
        40usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(kdropped)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).array) as usize - ptr as usize },
        48usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(array)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sqes) as usize - ptr as usize },
        56usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(sqes)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sqe_head) as usize - ptr as usize },
        64usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(sqe_head)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sqe_tail) as usize - ptr as usize },
        68usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(sqe_tail)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_sz) as usize - ptr as usize },
        72usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(ring_sz)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_ptr) as usize - ptr as usize },
        80usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(ring_ptr)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_mask) as usize - ptr as usize },
        88usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(ring_mask)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_entries) as usize - ptr as usize },
        92usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(ring_entries)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).pad) as usize - ptr as usize },
        96usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_sq),
            "::",
            stringify!(pad)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring_cq() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring_cq> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring_cq>(),
        88usize,
        concat!("Size of: ", stringify!(io_uring_cq))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring_cq>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring_cq))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).khead) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(khead)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ktail) as usize - ptr as usize },
        8usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(ktail)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).kring_mask) as usize - ptr as usize },
        16usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(kring_mask)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).kring_entries) as usize - ptr as usize },
        24usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(kring_entries)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).kflags) as usize - ptr as usize },
        32usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(kflags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).koverflow) as usize - ptr as usize },
        40usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(koverflow)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).cqes) as usize - ptr as usize },
        48usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(cqes)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_sz) as usize - ptr as usize },
        56usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(ring_sz)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_ptr) as usize - ptr as usize },
        64usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(ring_ptr)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_mask) as usize - ptr as usize },
        72usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(ring_mask)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_entries) as usize - ptr as usize },
        76usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(ring_entries)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).pad) as usize - ptr as usize },
        80usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring_cq),
            "::",
            stringify!(pad)
        )
    );
}
#[test]
fn bindgen_test_layout_io_uring() {
    const UNINIT: ::std::mem::MaybeUninit<io_uring> = ::std::mem::MaybeUninit::uninit();
    let ptr = UNINIT.as_ptr();
    assert_eq!(
        ::std::mem::size_of::<io_uring>(),
        216usize,
        concat!("Size of: ", stringify!(io_uring))
    );
    assert_eq!(
        ::std::mem::align_of::<io_uring>(),
        8usize,
        concat!("Alignment of ", stringify!(io_uring))
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).sq) as usize - ptr as usize },
        0usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring),
            "::",
            stringify!(sq)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).cq) as usize - ptr as usize },
        104usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring),
            "::",
            stringify!(cq)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize },
        192usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring),
            "::",
            stringify!(flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).ring_fd) as usize - ptr as usize },
        196usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring),
            "::",
            stringify!(ring_fd)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).features) as usize - ptr as usize },
        200usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring),
            "::",
            stringify!(features)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).enter_ring_fd) as usize - ptr as usize },
        204usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring),
            "::",
            stringify!(enter_ring_fd)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).int_flags) as usize - ptr as usize },
        208usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring),
            "::",
            stringify!(int_flags)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).pad) as usize - ptr as usize },
        209usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring),
            "::",
            stringify!(pad)
        )
    );
    assert_eq!(
        unsafe { ::std::ptr::addr_of!((*ptr).pad2) as usize - ptr as usize },
        212usize,
        concat!(
            "Offset of field: ",
            stringify!(io_uring),
            "::",
            stringify!(pad2)
        )
    );
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union io_uring_sqe__bindgen_ty_1 {
    pub off: __u64,
    pub addr2: __u64,
    pub __bindgen_anon_1: io_uring_sqe__bindgen_ty_1__bindgen_ty_1,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union io_uring_sqe__bindgen_ty_2 {
    pub addr: __u64,
    pub splice_off_in: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union io_uring_sqe__bindgen_ty_3 {
    pub rw_flags: __kernel_rwf_t,
    pub fsync_flags: __u32,
    pub poll_events: __u16,
    pub poll32_events: __u32,
    pub sync_range_flags: __u32,
    pub msg_flags: __u32,
    pub timeout_flags: __u32,
    pub accept_flags: __u32,
    pub cancel_flags: __u32,
    pub open_flags: __u32,
    pub statx_flags: __u32,
    pub fadvise_advice: __u32,
    pub splice_flags: __u32,
    pub rename_flags: __u32,
    pub unlink_flags: __u32,
    pub hardlink_flags: __u32,
    pub xattr_flags: __u32,
    pub msg_ring_flags: __u32,
    pub uring_cmd_flags: __u32,
}
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub union io_uring_sqe__bindgen_ty_4 {
    pub buf_index: __u16,
    pub buf_group: __u16,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union io_uring_sqe__bindgen_ty_5 {
    pub splice_fd_in: __s32,
    pub file_index: __u32,
    pub __bindgen_anon_1: io_uring_sqe__bindgen_ty_5__bindgen_ty_1,
}
#[repr(C)]
pub union io_uring_sqe__bindgen_ty_6 {
    pub __bindgen_anon_1: ::std::mem::ManuallyDrop<io_uring_sqe__bindgen_ty_6__bindgen_ty_1>,
    pub cmd: ::std::mem::ManuallyDrop<[__u8; 0usize]>,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub union io_uring_restriction__bindgen_ty_1 {
    pub register_op: __u8,
    pub sqe_op: __u8,
    pub sqe_flags: __u8,
}
#[repr(C)]
pub union io_uring_buf_ring__bindgen_ty_1 {
    pub __bindgen_anon_1: ::std::mem::ManuallyDrop<io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1>,
    pub bufs: ::std::mem::ManuallyDrop<[io_uring_buf; 0usize]>,
}
impl<T> __IncompleteArrayField<T> {
    #[inline]
    pub const fn new() -> Self {
        __IncompleteArrayField(::std::marker::PhantomData, [])
    }
    #[inline]
    pub fn as_ptr(&self) -> *const T {
        self as *const _ as *const T
    }
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self as *mut _ as *mut T
    }
    #[inline]
    pub unsafe fn as_slice(&self, len: usize) -> &[T] {
        ::std::slice::from_raw_parts(self.as_ptr(), len)
    }
    #[inline]
    pub unsafe fn as_mut_slice(&mut self, len: usize) -> &mut [T] {
        ::std::slice::from_raw_parts_mut(self.as_mut_ptr(), len)
    }
}
impl<T> ::std::fmt::Debug for __IncompleteArrayField<T> {
    fn fmt(&self, fmt: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        fmt.write_str("__IncompleteArrayField")
    }
}
