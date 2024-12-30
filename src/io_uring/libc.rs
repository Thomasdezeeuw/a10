//! Code that should be moved to libc once C libraries have a wrapper.

#![allow(warnings, clippy::all, clippy::pedantic, clippy::nursery)]

pub use libc::*;

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
pub type io_uring_sqe_flags_bit = ::std::os::raw::c_uint;
pub type io_uring_op = ::std::os::raw::c_uint;
pub type io_uring_msg_ring_flags = ::std::os::raw::c_uint;
pub type io_uring_register_op = ::std::os::raw::c_uint;
pub type _bindgen_ty_13 = ::std::os::raw::c_uint;
pub type _bindgen_ty_14 = ::std::os::raw::c_uint;
pub type _bindgen_ty_15 = ::std::os::raw::c_uint;
pub type io_uring_register_pbuf_ring_flags = ::std::os::raw::c_uint;
pub type io_uring_register_restriction_op = ::std::os::raw::c_uint;
pub type _bindgen_ty_16 = ::std::os::raw::c_uint;
pub type io_uring_socket_op = ::std::os::raw::c_uint;
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
pub struct io_uring_sqe__bindgen_ty_2__bindgen_ty_1 {
    pub level: __u32,
    pub optname: __u32,
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
    pub user_addr: __u64,
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
    pub user_addr: __u64,
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
pub struct io_uring_region_desc {
    pub user_addr: __u64,
    pub size: __u64,
    pub flags: __u32,
    pub id: __u32,
    pub mmap_offset: __u64,
    pub __resv: [__u64; 4usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_mem_region_reg {
    pub region_uptr: __u64,
    pub flags: __u64,
    pub __resv: [__u64; 2usize],
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
pub struct io_uring_clock_register {
    pub clockid: __u32,
    pub __resv: [__u32; 3usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_clone_buffers {
    pub src_fd: __u32,
    pub flags: __u32,
    pub src_off: __u32,
    pub dst_off: __u32,
    pub nr: __u32,
    pub pad: [__u32; 3usize],
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
    pub flags: __u16,
    pub resv: [__u64; 3usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_buf_status {
    pub buf_group: __u32,
    pub head: __u32,
    pub resv: [__u32; 8usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_napi {
    pub busy_poll_to: __u32,
    pub prefer_busy_poll: __u8,
    pub pad: [__u8; 3usize],
    pub resv: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_cqwait_reg_arg {
    pub flags: __u32,
    pub struct_size: __u32,
    pub nr_entries: __u32,
    pub pad: __u32,
    pub user_addr: __u64,
    pub pad2: [__u64; 3usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_reg_wait {
    pub ts: __kernel_timespec,
    pub min_wait_usec: __u32,
    pub flags: __u32,
    pub sigmask: __u64,
    pub sigmask_sz: __u32,
    pub pad: [__u32; 3usize],
    pub pad2: [__u64; 2usize],
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_getevents_arg {
    pub sigmask: __u64,
    pub sigmask_sz: __u32,
    pub min_wait_usec: __u32,
    pub ts: __u64,
}
#[repr(C)]
#[derive(Copy, Clone)]
pub struct io_uring_sync_cancel_reg {
    pub addr: __u64,
    pub fd: __s32,
    pub flags: __u32,
    pub timeout: __kernel_timespec,
    pub opcode: __u8,
    pub pad: [__u8; 7usize],
    pub pad2: [__u64; 3usize],
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
pub const IORING_SETUP_NO_MMAP: u32 = 16384;
pub const IORING_SETUP_REGISTERED_FD_ONLY: u32 = 32768;
pub const IORING_SETUP_NO_SQARRAY: u32 = 65536;
pub const IORING_SETUP_HYBRID_IOPOLL: u32 = 131072;
pub const IORING_URING_CMD_FIXED: u32 = 1;
pub const IORING_URING_CMD_MASK: u32 = 1;
pub const IORING_FSYNC_DATASYNC: u32 = 1;
pub const IORING_TIMEOUT_ABS: u32 = 1;
pub const IORING_TIMEOUT_UPDATE: u32 = 2;
pub const IORING_TIMEOUT_BOOTTIME: u32 = 4;
pub const IORING_TIMEOUT_REALTIME: u32 = 8;
pub const IORING_LINK_TIMEOUT_UPDATE: u32 = 16;
pub const IORING_TIMEOUT_ETIME_SUCCESS: u32 = 32;
pub const IORING_TIMEOUT_MULTISHOT: u32 = 64;
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
pub const IORING_ASYNC_CANCEL_USERDATA: u32 = 16;
pub const IORING_ASYNC_CANCEL_OP: u32 = 32;
pub const IORING_RECVSEND_POLL_FIRST: u32 = 1;
pub const IORING_RECV_MULTISHOT: u32 = 2;
pub const IORING_RECVSEND_FIXED_BUF: u32 = 4;
pub const IORING_SEND_ZC_REPORT_USAGE: u32 = 8;
pub const IORING_RECVSEND_BUNDLE: u32 = 16;
pub const IORING_NOTIF_USAGE_ZC_COPIED: u32 = 2147483648;
pub const IORING_ACCEPT_MULTISHOT: u32 = 1;
pub const IORING_ACCEPT_DONTWAIT: u32 = 2;
pub const IORING_ACCEPT_POLL_FIRST: u32 = 4;
pub const IORING_MSG_RING_CQE_SKIP: u32 = 1;
pub const IORING_MSG_RING_FLAGS_PASS: u32 = 2;
pub const IORING_FIXED_FD_NO_CLOEXEC: u32 = 1;
pub const IORING_NOP_INJECT_RESULT: u32 = 1;
pub const IORING_CQE_F_BUFFER: u32 = 1;
pub const IORING_CQE_F_MORE: u32 = 2;
pub const IORING_CQE_F_SOCK_NONEMPTY: u32 = 4;
pub const IORING_CQE_F_NOTIF: u32 = 8;
pub const IORING_CQE_F_BUF_MORE: u32 = 16;
pub const IORING_CQE_BUFFER_SHIFT: u32 = 16;
pub const IORING_OFF_SQ_RING: u32 = 0;
pub const IORING_OFF_CQ_RING: u32 = 134217728;
pub const IORING_OFF_SQES: u32 = 268435456;
pub const IORING_OFF_PBUF_RING: u32 = 2147483648;
pub const IORING_OFF_PBUF_SHIFT: u32 = 16;
pub const IORING_OFF_MMAP_MASK: u32 = 4160749568;
pub const IORING_SQ_NEED_WAKEUP: u32 = 1;
pub const IORING_SQ_CQ_OVERFLOW: u32 = 2;
pub const IORING_SQ_TASKRUN: u32 = 4;
pub const IORING_CQ_EVENTFD_DISABLED: u32 = 1;
pub const IORING_ENTER_GETEVENTS: u32 = 1;
pub const IORING_ENTER_SQ_WAKEUP: u32 = 2;
pub const IORING_ENTER_SQ_WAIT: u32 = 4;
pub const IORING_ENTER_EXT_ARG: u32 = 8;
pub const IORING_ENTER_REGISTERED_RING: u32 = 16;
pub const IORING_ENTER_ABS_TIMER: u32 = 32;
pub const IORING_ENTER_EXT_ARG_REG: u32 = 64;
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
pub const IORING_FEAT_REG_REG_RING: u32 = 8192;
pub const IORING_FEAT_RECVSEND_BUNDLE: u32 = 16384;
pub const IORING_FEAT_MIN_TIMEOUT: u32 = 32768;
pub const IORING_RSRC_REGISTER_SPARSE: u32 = 1;
pub const IORING_REGISTER_FILES_SKIP: i32 = -2;
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of __kernel_timespec"][::std::mem::size_of::<__kernel_timespec>() - 16usize];
    ["Alignment of __kernel_timespec"][::std::mem::align_of::<__kernel_timespec>() - 8usize];
    ["Offset of field: __kernel_timespec::tv_sec"]
        [::std::mem::offset_of!(__kernel_timespec, tv_sec) - 0usize];
    ["Offset of field: __kernel_timespec::tv_nsec"]
        [::std::mem::offset_of!(__kernel_timespec, tv_nsec) - 8usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_1__bindgen_ty_1"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_1__bindgen_ty_1>() - 8usize];
    ["Alignment of io_uring_sqe__bindgen_ty_1__bindgen_ty_1"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_1__bindgen_ty_1>() - 4usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_1__bindgen_ty_1::cmd_op"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_1__bindgen_ty_1, cmd_op) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_1__bindgen_ty_1::__pad1"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_1__bindgen_ty_1, __pad1) - 4usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_1"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_1>() - 8usize];
    ["Alignment of io_uring_sqe__bindgen_ty_1"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_1>() - 8usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_1::off"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_1, off) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_1::addr2"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_1, addr2) - 0usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_2__bindgen_ty_1"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_2__bindgen_ty_1>() - 8usize];
    ["Alignment of io_uring_sqe__bindgen_ty_2__bindgen_ty_1"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_2__bindgen_ty_1>() - 4usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_2__bindgen_ty_1::level"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_2__bindgen_ty_1, level) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_2__bindgen_ty_1::optname"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_2__bindgen_ty_1, optname) - 4usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_2"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_2>() - 8usize];
    ["Alignment of io_uring_sqe__bindgen_ty_2"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_2>() - 8usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_2::addr"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_2, addr) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_2::splice_off_in"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_2, splice_off_in) - 0usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_3"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_3>() - 4usize];
    ["Alignment of io_uring_sqe__bindgen_ty_3"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_3>() - 4usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::rw_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, rw_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::fsync_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, fsync_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::poll_events"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, poll_events) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::poll32_events"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, poll32_events) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::sync_range_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, sync_range_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::msg_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, msg_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::timeout_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, timeout_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::accept_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, accept_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::cancel_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, cancel_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::open_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, open_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::statx_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, statx_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::fadvise_advice"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, fadvise_advice) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::splice_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, splice_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::rename_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, rename_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::unlink_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, unlink_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::hardlink_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, hardlink_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::xattr_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, xattr_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::msg_ring_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, msg_ring_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::uring_cmd_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, uring_cmd_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::waitid_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, waitid_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::futex_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, futex_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::install_fd_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, install_fd_flags) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_3::nop_flags"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_3, nop_flags) - 0usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_4"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_4>() - 2usize];
    ["Alignment of io_uring_sqe__bindgen_ty_4"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_4>() - 1usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_4::buf_index"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_4, buf_index) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_4::buf_group"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_4, buf_group) - 0usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_5__bindgen_ty_1"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_5__bindgen_ty_1>() - 4usize];
    ["Alignment of io_uring_sqe__bindgen_ty_5__bindgen_ty_1"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_5__bindgen_ty_1>() - 2usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_5__bindgen_ty_1::addr_len"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_5__bindgen_ty_1, addr_len) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_5__bindgen_ty_1::__pad3"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_5__bindgen_ty_1, __pad3) - 2usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_5"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_5>() - 4usize];
    ["Alignment of io_uring_sqe__bindgen_ty_5"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_5>() - 4usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_5::splice_fd_in"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_5, splice_fd_in) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_5::file_index"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_5, file_index) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_5::optlen"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_5, optlen) - 0usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_6__bindgen_ty_1"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_6__bindgen_ty_1>() - 16usize];
    ["Alignment of io_uring_sqe__bindgen_ty_6__bindgen_ty_1"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_6__bindgen_ty_1>() - 8usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_6__bindgen_ty_1::addr3"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_6__bindgen_ty_1, addr3) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_6__bindgen_ty_1::__pad2"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_6__bindgen_ty_1, __pad2) - 8usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe__bindgen_ty_6"]
        [::std::mem::size_of::<io_uring_sqe__bindgen_ty_6>() - 16usize];
    ["Alignment of io_uring_sqe__bindgen_ty_6"]
        [::std::mem::align_of::<io_uring_sqe__bindgen_ty_6>() - 8usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_6::optval"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_6, optval) - 0usize];
    ["Offset of field: io_uring_sqe__bindgen_ty_6::cmd"]
        [::std::mem::offset_of!(io_uring_sqe__bindgen_ty_6, cmd) - 0usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sqe"][::std::mem::size_of::<io_uring_sqe>() - 64usize];
    ["Alignment of io_uring_sqe"][::std::mem::align_of::<io_uring_sqe>() - 8usize];
    ["Offset of field: io_uring_sqe::opcode"]
        [::std::mem::offset_of!(io_uring_sqe, opcode) - 0usize];
    ["Offset of field: io_uring_sqe::flags"][::std::mem::offset_of!(io_uring_sqe, flags) - 1usize];
    ["Offset of field: io_uring_sqe::ioprio"]
        [::std::mem::offset_of!(io_uring_sqe, ioprio) - 2usize];
    ["Offset of field: io_uring_sqe::fd"][::std::mem::offset_of!(io_uring_sqe, fd) - 4usize];
    ["Offset of field: io_uring_sqe::len"][::std::mem::offset_of!(io_uring_sqe, len) - 24usize];
    ["Offset of field: io_uring_sqe::user_data"]
        [::std::mem::offset_of!(io_uring_sqe, user_data) - 32usize];
    ["Offset of field: io_uring_sqe::personality"]
        [::std::mem::offset_of!(io_uring_sqe, personality) - 42usize];
};
pub const IOSQE_FIXED_FILE_BIT: io_uring_sqe_flags_bit = 0;
pub const IOSQE_IO_DRAIN_BIT: io_uring_sqe_flags_bit = 1;
pub const IOSQE_IO_LINK_BIT: io_uring_sqe_flags_bit = 2;
pub const IOSQE_IO_HARDLINK_BIT: io_uring_sqe_flags_bit = 3;
pub const IOSQE_ASYNC_BIT: io_uring_sqe_flags_bit = 4;
pub const IOSQE_BUFFER_SELECT_BIT: io_uring_sqe_flags_bit = 5;
pub const IOSQE_CQE_SKIP_SUCCESS_BIT: io_uring_sqe_flags_bit = 6;
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
pub const IORING_OP_READ_MULTISHOT: io_uring_op = 49;
pub const IORING_OP_WAITID: io_uring_op = 50;
pub const IORING_OP_FUTEX_WAIT: io_uring_op = 51;
pub const IORING_OP_FUTEX_WAKE: io_uring_op = 52;
pub const IORING_OP_FUTEX_WAITV: io_uring_op = 53;
pub const IORING_OP_FIXED_FD_INSTALL: io_uring_op = 54;
pub const IORING_OP_FTRUNCATE: io_uring_op = 55;
pub const IORING_OP_BIND: io_uring_op = 56;
pub const IORING_OP_LISTEN: io_uring_op = 57;
pub const IORING_OP_LAST: io_uring_op = 58;
pub const IORING_MSG_DATA: io_uring_msg_ring_flags = 0;
pub const IORING_MSG_SEND_FD: io_uring_msg_ring_flags = 1;
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_cqe"][::std::mem::size_of::<io_uring_cqe>() - 16usize];
    ["Alignment of io_uring_cqe"][::std::mem::align_of::<io_uring_cqe>() - 8usize];
    ["Offset of field: io_uring_cqe::user_data"]
        [::std::mem::offset_of!(io_uring_cqe, user_data) - 0usize];
    ["Offset of field: io_uring_cqe::res"][::std::mem::offset_of!(io_uring_cqe, res) - 8usize];
    ["Offset of field: io_uring_cqe::flags"][::std::mem::offset_of!(io_uring_cqe, flags) - 12usize];
    ["Offset of field: io_uring_cqe::big_cqe"]
        [::std::mem::offset_of!(io_uring_cqe, big_cqe) - 16usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_sqring_offsets"][::std::mem::size_of::<io_sqring_offsets>() - 40usize];
    ["Alignment of io_sqring_offsets"][::std::mem::align_of::<io_sqring_offsets>() - 8usize];
    ["Offset of field: io_sqring_offsets::head"]
        [::std::mem::offset_of!(io_sqring_offsets, head) - 0usize];
    ["Offset of field: io_sqring_offsets::tail"]
        [::std::mem::offset_of!(io_sqring_offsets, tail) - 4usize];
    ["Offset of field: io_sqring_offsets::ring_mask"]
        [::std::mem::offset_of!(io_sqring_offsets, ring_mask) - 8usize];
    ["Offset of field: io_sqring_offsets::ring_entries"]
        [::std::mem::offset_of!(io_sqring_offsets, ring_entries) - 12usize];
    ["Offset of field: io_sqring_offsets::flags"]
        [::std::mem::offset_of!(io_sqring_offsets, flags) - 16usize];
    ["Offset of field: io_sqring_offsets::dropped"]
        [::std::mem::offset_of!(io_sqring_offsets, dropped) - 20usize];
    ["Offset of field: io_sqring_offsets::array"]
        [::std::mem::offset_of!(io_sqring_offsets, array) - 24usize];
    ["Offset of field: io_sqring_offsets::resv1"]
        [::std::mem::offset_of!(io_sqring_offsets, resv1) - 28usize];
    ["Offset of field: io_sqring_offsets::user_addr"]
        [::std::mem::offset_of!(io_sqring_offsets, user_addr) - 32usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_cqring_offsets"][::std::mem::size_of::<io_cqring_offsets>() - 40usize];
    ["Alignment of io_cqring_offsets"][::std::mem::align_of::<io_cqring_offsets>() - 8usize];
    ["Offset of field: io_cqring_offsets::head"]
        [::std::mem::offset_of!(io_cqring_offsets, head) - 0usize];
    ["Offset of field: io_cqring_offsets::tail"]
        [::std::mem::offset_of!(io_cqring_offsets, tail) - 4usize];
    ["Offset of field: io_cqring_offsets::ring_mask"]
        [::std::mem::offset_of!(io_cqring_offsets, ring_mask) - 8usize];
    ["Offset of field: io_cqring_offsets::ring_entries"]
        [::std::mem::offset_of!(io_cqring_offsets, ring_entries) - 12usize];
    ["Offset of field: io_cqring_offsets::overflow"]
        [::std::mem::offset_of!(io_cqring_offsets, overflow) - 16usize];
    ["Offset of field: io_cqring_offsets::cqes"]
        [::std::mem::offset_of!(io_cqring_offsets, cqes) - 20usize];
    ["Offset of field: io_cqring_offsets::flags"]
        [::std::mem::offset_of!(io_cqring_offsets, flags) - 24usize];
    ["Offset of field: io_cqring_offsets::resv1"]
        [::std::mem::offset_of!(io_cqring_offsets, resv1) - 28usize];
    ["Offset of field: io_cqring_offsets::user_addr"]
        [::std::mem::offset_of!(io_cqring_offsets, user_addr) - 32usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_params"][::std::mem::size_of::<io_uring_params>() - 120usize];
    ["Alignment of io_uring_params"][::std::mem::align_of::<io_uring_params>() - 8usize];
    ["Offset of field: io_uring_params::sq_entries"]
        [::std::mem::offset_of!(io_uring_params, sq_entries) - 0usize];
    ["Offset of field: io_uring_params::cq_entries"]
        [::std::mem::offset_of!(io_uring_params, cq_entries) - 4usize];
    ["Offset of field: io_uring_params::flags"]
        [::std::mem::offset_of!(io_uring_params, flags) - 8usize];
    ["Offset of field: io_uring_params::sq_thread_cpu"]
        [::std::mem::offset_of!(io_uring_params, sq_thread_cpu) - 12usize];
    ["Offset of field: io_uring_params::sq_thread_idle"]
        [::std::mem::offset_of!(io_uring_params, sq_thread_idle) - 16usize];
    ["Offset of field: io_uring_params::features"]
        [::std::mem::offset_of!(io_uring_params, features) - 20usize];
    ["Offset of field: io_uring_params::wq_fd"]
        [::std::mem::offset_of!(io_uring_params, wq_fd) - 24usize];
    ["Offset of field: io_uring_params::resv"]
        [::std::mem::offset_of!(io_uring_params, resv) - 28usize];
    ["Offset of field: io_uring_params::sq_off"]
        [::std::mem::offset_of!(io_uring_params, sq_off) - 40usize];
    ["Offset of field: io_uring_params::cq_off"]
        [::std::mem::offset_of!(io_uring_params, cq_off) - 80usize];
};
pub const IORING_REGISTER_BUFFERS: io_uring_register_op = 0;
pub const IORING_UNREGISTER_BUFFERS: io_uring_register_op = 1;
pub const IORING_REGISTER_FILES: io_uring_register_op = 2;
pub const IORING_UNREGISTER_FILES: io_uring_register_op = 3;
pub const IORING_REGISTER_EVENTFD: io_uring_register_op = 4;
pub const IORING_UNREGISTER_EVENTFD: io_uring_register_op = 5;
pub const IORING_REGISTER_FILES_UPDATE: io_uring_register_op = 6;
pub const IORING_REGISTER_EVENTFD_ASYNC: io_uring_register_op = 7;
pub const IORING_REGISTER_PROBE: io_uring_register_op = 8;
pub const IORING_REGISTER_PERSONALITY: io_uring_register_op = 9;
pub const IORING_UNREGISTER_PERSONALITY: io_uring_register_op = 10;
pub const IORING_REGISTER_RESTRICTIONS: io_uring_register_op = 11;
pub const IORING_REGISTER_ENABLE_RINGS: io_uring_register_op = 12;
pub const IORING_REGISTER_FILES2: io_uring_register_op = 13;
pub const IORING_REGISTER_FILES_UPDATE2: io_uring_register_op = 14;
pub const IORING_REGISTER_BUFFERS2: io_uring_register_op = 15;
pub const IORING_REGISTER_BUFFERS_UPDATE: io_uring_register_op = 16;
pub const IORING_REGISTER_IOWQ_AFF: io_uring_register_op = 17;
pub const IORING_UNREGISTER_IOWQ_AFF: io_uring_register_op = 18;
pub const IORING_REGISTER_IOWQ_MAX_WORKERS: io_uring_register_op = 19;
pub const IORING_REGISTER_RING_FDS: io_uring_register_op = 20;
pub const IORING_UNREGISTER_RING_FDS: io_uring_register_op = 21;
pub const IORING_REGISTER_PBUF_RING: io_uring_register_op = 22;
pub const IORING_UNREGISTER_PBUF_RING: io_uring_register_op = 23;
pub const IORING_REGISTER_SYNC_CANCEL: io_uring_register_op = 24;
pub const IORING_REGISTER_FILE_ALLOC_RANGE: io_uring_register_op = 25;
pub const IORING_REGISTER_PBUF_STATUS: io_uring_register_op = 26;
pub const IORING_REGISTER_NAPI: io_uring_register_op = 27;
pub const IORING_UNREGISTER_NAPI: io_uring_register_op = 28;
pub const IORING_REGISTER_CLOCK: io_uring_register_op = 29;
pub const IORING_REGISTER_CLONE_BUFFERS: io_uring_register_op = 30;
pub const IORING_REGISTER_RESIZE_RINGS: io_uring_register_op = 33;
pub const IORING_REGISTER_MEM_REGION: io_uring_register_op = 34;
pub const IORING_REGISTER_LAST: io_uring_register_op = 35;
pub const IORING_REGISTER_USE_REGISTERED_RING: io_uring_register_op = 2147483648;
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_files_update"][::std::mem::size_of::<io_uring_files_update>() - 16usize];
    ["Alignment of io_uring_files_update"]
        [::std::mem::align_of::<io_uring_files_update>() - 8usize];
    ["Offset of field: io_uring_files_update::offset"]
        [::std::mem::offset_of!(io_uring_files_update, offset) - 0usize];
    ["Offset of field: io_uring_files_update::resv"]
        [::std::mem::offset_of!(io_uring_files_update, resv) - 4usize];
    ["Offset of field: io_uring_files_update::fds"]
        [::std::mem::offset_of!(io_uring_files_update, fds) - 8usize];
};
pub const IORING_MEM_REGION_TYPE_USER: _bindgen_ty_13 = 1;
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_region_desc"][::std::mem::size_of::<io_uring_region_desc>() - 64usize];
    ["Alignment of io_uring_region_desc"][::std::mem::align_of::<io_uring_region_desc>() - 8usize];
    ["Offset of field: io_uring_region_desc::user_addr"]
        [::std::mem::offset_of!(io_uring_region_desc, user_addr) - 0usize];
    ["Offset of field: io_uring_region_desc::size"]
        [::std::mem::offset_of!(io_uring_region_desc, size) - 8usize];
    ["Offset of field: io_uring_region_desc::flags"]
        [::std::mem::offset_of!(io_uring_region_desc, flags) - 16usize];
    ["Offset of field: io_uring_region_desc::id"]
        [::std::mem::offset_of!(io_uring_region_desc, id) - 20usize];
    ["Offset of field: io_uring_region_desc::mmap_offset"]
        [::std::mem::offset_of!(io_uring_region_desc, mmap_offset) - 24usize];
    ["Offset of field: io_uring_region_desc::__resv"]
        [::std::mem::offset_of!(io_uring_region_desc, __resv) - 32usize];
};
pub const IORING_MEM_REGION_REG_WAIT_ARG: _bindgen_ty_14 = 1;
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_mem_region_reg"][::std::mem::size_of::<io_uring_mem_region_reg>() - 32usize];
    ["Alignment of io_uring_mem_region_reg"]
        [::std::mem::align_of::<io_uring_mem_region_reg>() - 8usize];
    ["Offset of field: io_uring_mem_region_reg::region_uptr"]
        [::std::mem::offset_of!(io_uring_mem_region_reg, region_uptr) - 0usize];
    ["Offset of field: io_uring_mem_region_reg::flags"]
        [::std::mem::offset_of!(io_uring_mem_region_reg, flags) - 8usize];
    ["Offset of field: io_uring_mem_region_reg::__resv"]
        [::std::mem::offset_of!(io_uring_mem_region_reg, __resv) - 16usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_rsrc_register"][::std::mem::size_of::<io_uring_rsrc_register>() - 32usize];
    ["Alignment of io_uring_rsrc_register"]
        [::std::mem::align_of::<io_uring_rsrc_register>() - 8usize];
    ["Offset of field: io_uring_rsrc_register::nr"]
        [::std::mem::offset_of!(io_uring_rsrc_register, nr) - 0usize];
    ["Offset of field: io_uring_rsrc_register::flags"]
        [::std::mem::offset_of!(io_uring_rsrc_register, flags) - 4usize];
    ["Offset of field: io_uring_rsrc_register::resv2"]
        [::std::mem::offset_of!(io_uring_rsrc_register, resv2) - 8usize];
    ["Offset of field: io_uring_rsrc_register::data"]
        [::std::mem::offset_of!(io_uring_rsrc_register, data) - 16usize];
    ["Offset of field: io_uring_rsrc_register::tags"]
        [::std::mem::offset_of!(io_uring_rsrc_register, tags) - 24usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_rsrc_update"][::std::mem::size_of::<io_uring_rsrc_update>() - 16usize];
    ["Alignment of io_uring_rsrc_update"][::std::mem::align_of::<io_uring_rsrc_update>() - 8usize];
    ["Offset of field: io_uring_rsrc_update::offset"]
        [::std::mem::offset_of!(io_uring_rsrc_update, offset) - 0usize];
    ["Offset of field: io_uring_rsrc_update::resv"]
        [::std::mem::offset_of!(io_uring_rsrc_update, resv) - 4usize];
    ["Offset of field: io_uring_rsrc_update::data"]
        [::std::mem::offset_of!(io_uring_rsrc_update, data) - 8usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_rsrc_update2"][::std::mem::size_of::<io_uring_rsrc_update2>() - 32usize];
    ["Alignment of io_uring_rsrc_update2"]
        [::std::mem::align_of::<io_uring_rsrc_update2>() - 8usize];
    ["Offset of field: io_uring_rsrc_update2::offset"]
        [::std::mem::offset_of!(io_uring_rsrc_update2, offset) - 0usize];
    ["Offset of field: io_uring_rsrc_update2::resv"]
        [::std::mem::offset_of!(io_uring_rsrc_update2, resv) - 4usize];
    ["Offset of field: io_uring_rsrc_update2::data"]
        [::std::mem::offset_of!(io_uring_rsrc_update2, data) - 8usize];
    ["Offset of field: io_uring_rsrc_update2::tags"]
        [::std::mem::offset_of!(io_uring_rsrc_update2, tags) - 16usize];
    ["Offset of field: io_uring_rsrc_update2::nr"]
        [::std::mem::offset_of!(io_uring_rsrc_update2, nr) - 24usize];
    ["Offset of field: io_uring_rsrc_update2::resv2"]
        [::std::mem::offset_of!(io_uring_rsrc_update2, resv2) - 28usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_probe_op"][::std::mem::size_of::<io_uring_probe_op>() - 8usize];
    ["Alignment of io_uring_probe_op"][::std::mem::align_of::<io_uring_probe_op>() - 4usize];
    ["Offset of field: io_uring_probe_op::op"]
        [::std::mem::offset_of!(io_uring_probe_op, op) - 0usize];
    ["Offset of field: io_uring_probe_op::resv"]
        [::std::mem::offset_of!(io_uring_probe_op, resv) - 1usize];
    ["Offset of field: io_uring_probe_op::flags"]
        [::std::mem::offset_of!(io_uring_probe_op, flags) - 2usize];
    ["Offset of field: io_uring_probe_op::resv2"]
        [::std::mem::offset_of!(io_uring_probe_op, resv2) - 4usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_probe"][::std::mem::size_of::<io_uring_probe>() - 16usize];
    ["Alignment of io_uring_probe"][::std::mem::align_of::<io_uring_probe>() - 4usize];
    ["Offset of field: io_uring_probe::last_op"]
        [::std::mem::offset_of!(io_uring_probe, last_op) - 0usize];
    ["Offset of field: io_uring_probe::ops_len"]
        [::std::mem::offset_of!(io_uring_probe, ops_len) - 1usize];
    ["Offset of field: io_uring_probe::resv"]
        [::std::mem::offset_of!(io_uring_probe, resv) - 2usize];
    ["Offset of field: io_uring_probe::resv2"]
        [::std::mem::offset_of!(io_uring_probe, resv2) - 4usize];
    ["Offset of field: io_uring_probe::ops"][::std::mem::offset_of!(io_uring_probe, ops) - 16usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_restriction__bindgen_ty_1"]
        [::std::mem::size_of::<io_uring_restriction__bindgen_ty_1>() - 1usize];
    ["Alignment of io_uring_restriction__bindgen_ty_1"]
        [::std::mem::align_of::<io_uring_restriction__bindgen_ty_1>() - 1usize];
    ["Offset of field: io_uring_restriction__bindgen_ty_1::register_op"]
        [::std::mem::offset_of!(io_uring_restriction__bindgen_ty_1, register_op) - 0usize];
    ["Offset of field: io_uring_restriction__bindgen_ty_1::sqe_op"]
        [::std::mem::offset_of!(io_uring_restriction__bindgen_ty_1, sqe_op) - 0usize];
    ["Offset of field: io_uring_restriction__bindgen_ty_1::sqe_flags"]
        [::std::mem::offset_of!(io_uring_restriction__bindgen_ty_1, sqe_flags) - 0usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_restriction"][::std::mem::size_of::<io_uring_restriction>() - 16usize];
    ["Alignment of io_uring_restriction"][::std::mem::align_of::<io_uring_restriction>() - 4usize];
    ["Offset of field: io_uring_restriction::opcode"]
        [::std::mem::offset_of!(io_uring_restriction, opcode) - 0usize];
    ["Offset of field: io_uring_restriction::resv"]
        [::std::mem::offset_of!(io_uring_restriction, resv) - 3usize];
    ["Offset of field: io_uring_restriction::resv2"]
        [::std::mem::offset_of!(io_uring_restriction, resv2) - 4usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_clock_register"][::std::mem::size_of::<io_uring_clock_register>() - 16usize];
    ["Alignment of io_uring_clock_register"]
        [::std::mem::align_of::<io_uring_clock_register>() - 4usize];
    ["Offset of field: io_uring_clock_register::clockid"]
        [::std::mem::offset_of!(io_uring_clock_register, clockid) - 0usize];
    ["Offset of field: io_uring_clock_register::__resv"]
        [::std::mem::offset_of!(io_uring_clock_register, __resv) - 4usize];
};
pub const IORING_REGISTER_SRC_REGISTERED: _bindgen_ty_15 = 1;
pub const IORING_REGISTER_DST_REPLACE: _bindgen_ty_15 = 2;
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_clone_buffers"][::std::mem::size_of::<io_uring_clone_buffers>() - 32usize];
    ["Alignment of io_uring_clone_buffers"]
        [::std::mem::align_of::<io_uring_clone_buffers>() - 4usize];
    ["Offset of field: io_uring_clone_buffers::src_fd"]
        [::std::mem::offset_of!(io_uring_clone_buffers, src_fd) - 0usize];
    ["Offset of field: io_uring_clone_buffers::flags"]
        [::std::mem::offset_of!(io_uring_clone_buffers, flags) - 4usize];
    ["Offset of field: io_uring_clone_buffers::src_off"]
        [::std::mem::offset_of!(io_uring_clone_buffers, src_off) - 8usize];
    ["Offset of field: io_uring_clone_buffers::dst_off"]
        [::std::mem::offset_of!(io_uring_clone_buffers, dst_off) - 12usize];
    ["Offset of field: io_uring_clone_buffers::nr"]
        [::std::mem::offset_of!(io_uring_clone_buffers, nr) - 16usize];
    ["Offset of field: io_uring_clone_buffers::pad"]
        [::std::mem::offset_of!(io_uring_clone_buffers, pad) - 20usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_buf"][::std::mem::size_of::<io_uring_buf>() - 16usize];
    ["Alignment of io_uring_buf"][::std::mem::align_of::<io_uring_buf>() - 8usize];
    ["Offset of field: io_uring_buf::addr"][::std::mem::offset_of!(io_uring_buf, addr) - 0usize];
    ["Offset of field: io_uring_buf::len"][::std::mem::offset_of!(io_uring_buf, len) - 8usize];
    ["Offset of field: io_uring_buf::bid"][::std::mem::offset_of!(io_uring_buf, bid) - 12usize];
    ["Offset of field: io_uring_buf::resv"][::std::mem::offset_of!(io_uring_buf, resv) - 14usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1"]
        [::std::mem::size_of::<io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1>() - 16usize];
    ["Alignment of io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1"]
        [::std::mem::align_of::<io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1>() - 8usize];
    ["Offset of field: io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1::resv1"]
        [::std::mem::offset_of!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1, resv1) - 0usize];
    ["Offset of field: io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1::resv2"]
        [::std::mem::offset_of!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1, resv2) - 8usize];
    ["Offset of field: io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1::resv3"]
        [::std::mem::offset_of!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1, resv3) - 12usize];
    ["Offset of field: io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1::tail"]
        [::std::mem::offset_of!(io_uring_buf_ring__bindgen_ty_1__bindgen_ty_1, tail) - 14usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_buf_ring__bindgen_ty_1"]
        [::std::mem::size_of::<io_uring_buf_ring__bindgen_ty_1>() - 16usize];
    ["Alignment of io_uring_buf_ring__bindgen_ty_1"]
        [::std::mem::align_of::<io_uring_buf_ring__bindgen_ty_1>() - 8usize];
    ["Offset of field: io_uring_buf_ring__bindgen_ty_1::bufs"]
        [::std::mem::offset_of!(io_uring_buf_ring__bindgen_ty_1, bufs) - 0usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_buf_ring"][::std::mem::size_of::<io_uring_buf_ring>() - 16usize];
    ["Alignment of io_uring_buf_ring"][::std::mem::align_of::<io_uring_buf_ring>() - 8usize];
};
pub const IOU_PBUF_RING_MMAP: io_uring_register_pbuf_ring_flags = 1;
pub const IOU_PBUF_RING_INC: io_uring_register_pbuf_ring_flags = 2;
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_buf_reg"][::std::mem::size_of::<io_uring_buf_reg>() - 40usize];
    ["Alignment of io_uring_buf_reg"][::std::mem::align_of::<io_uring_buf_reg>() - 8usize];
    ["Offset of field: io_uring_buf_reg::ring_addr"]
        [::std::mem::offset_of!(io_uring_buf_reg, ring_addr) - 0usize];
    ["Offset of field: io_uring_buf_reg::ring_entries"]
        [::std::mem::offset_of!(io_uring_buf_reg, ring_entries) - 8usize];
    ["Offset of field: io_uring_buf_reg::bgid"]
        [::std::mem::offset_of!(io_uring_buf_reg, bgid) - 12usize];
    ["Offset of field: io_uring_buf_reg::flags"]
        [::std::mem::offset_of!(io_uring_buf_reg, flags) - 14usize];
    ["Offset of field: io_uring_buf_reg::resv"]
        [::std::mem::offset_of!(io_uring_buf_reg, resv) - 16usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_buf_status"][::std::mem::size_of::<io_uring_buf_status>() - 40usize];
    ["Alignment of io_uring_buf_status"][::std::mem::align_of::<io_uring_buf_status>() - 4usize];
    ["Offset of field: io_uring_buf_status::buf_group"]
        [::std::mem::offset_of!(io_uring_buf_status, buf_group) - 0usize];
    ["Offset of field: io_uring_buf_status::head"]
        [::std::mem::offset_of!(io_uring_buf_status, head) - 4usize];
    ["Offset of field: io_uring_buf_status::resv"]
        [::std::mem::offset_of!(io_uring_buf_status, resv) - 8usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_napi"][::std::mem::size_of::<io_uring_napi>() - 16usize];
    ["Alignment of io_uring_napi"][::std::mem::align_of::<io_uring_napi>() - 8usize];
    ["Offset of field: io_uring_napi::busy_poll_to"]
        [::std::mem::offset_of!(io_uring_napi, busy_poll_to) - 0usize];
    ["Offset of field: io_uring_napi::prefer_busy_poll"]
        [::std::mem::offset_of!(io_uring_napi, prefer_busy_poll) - 4usize];
    ["Offset of field: io_uring_napi::pad"][::std::mem::offset_of!(io_uring_napi, pad) - 5usize];
    ["Offset of field: io_uring_napi::resv"][::std::mem::offset_of!(io_uring_napi, resv) - 8usize];
};
pub const IORING_RESTRICTION_REGISTER_OP: io_uring_register_restriction_op = 0;
pub const IORING_RESTRICTION_SQE_OP: io_uring_register_restriction_op = 1;
pub const IORING_RESTRICTION_SQE_FLAGS_ALLOWED: io_uring_register_restriction_op = 2;
pub const IORING_RESTRICTION_SQE_FLAGS_REQUIRED: io_uring_register_restriction_op = 3;
pub const IORING_RESTRICTION_LAST: io_uring_register_restriction_op = 4;
pub const IORING_REG_WAIT_TS: _bindgen_ty_16 = 1;
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_cqwait_reg_arg"][::std::mem::size_of::<io_uring_cqwait_reg_arg>() - 48usize];
    ["Alignment of io_uring_cqwait_reg_arg"]
        [::std::mem::align_of::<io_uring_cqwait_reg_arg>() - 8usize];
    ["Offset of field: io_uring_cqwait_reg_arg::flags"]
        [::std::mem::offset_of!(io_uring_cqwait_reg_arg, flags) - 0usize];
    ["Offset of field: io_uring_cqwait_reg_arg::struct_size"]
        [::std::mem::offset_of!(io_uring_cqwait_reg_arg, struct_size) - 4usize];
    ["Offset of field: io_uring_cqwait_reg_arg::nr_entries"]
        [::std::mem::offset_of!(io_uring_cqwait_reg_arg, nr_entries) - 8usize];
    ["Offset of field: io_uring_cqwait_reg_arg::pad"]
        [::std::mem::offset_of!(io_uring_cqwait_reg_arg, pad) - 12usize];
    ["Offset of field: io_uring_cqwait_reg_arg::user_addr"]
        [::std::mem::offset_of!(io_uring_cqwait_reg_arg, user_addr) - 16usize];
    ["Offset of field: io_uring_cqwait_reg_arg::pad2"]
        [::std::mem::offset_of!(io_uring_cqwait_reg_arg, pad2) - 24usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_reg_wait"][::std::mem::size_of::<io_uring_reg_wait>() - 64usize];
    ["Alignment of io_uring_reg_wait"][::std::mem::align_of::<io_uring_reg_wait>() - 8usize];
    ["Offset of field: io_uring_reg_wait::ts"]
        [::std::mem::offset_of!(io_uring_reg_wait, ts) - 0usize];
    ["Offset of field: io_uring_reg_wait::min_wait_usec"]
        [::std::mem::offset_of!(io_uring_reg_wait, min_wait_usec) - 16usize];
    ["Offset of field: io_uring_reg_wait::flags"]
        [::std::mem::offset_of!(io_uring_reg_wait, flags) - 20usize];
    ["Offset of field: io_uring_reg_wait::sigmask"]
        [::std::mem::offset_of!(io_uring_reg_wait, sigmask) - 24usize];
    ["Offset of field: io_uring_reg_wait::sigmask_sz"]
        [::std::mem::offset_of!(io_uring_reg_wait, sigmask_sz) - 32usize];
    ["Offset of field: io_uring_reg_wait::pad"]
        [::std::mem::offset_of!(io_uring_reg_wait, pad) - 36usize];
    ["Offset of field: io_uring_reg_wait::pad2"]
        [::std::mem::offset_of!(io_uring_reg_wait, pad2) - 48usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_getevents_arg"][::std::mem::size_of::<io_uring_getevents_arg>() - 24usize];
    ["Alignment of io_uring_getevents_arg"]
        [::std::mem::align_of::<io_uring_getevents_arg>() - 8usize];
    ["Offset of field: io_uring_getevents_arg::sigmask"]
        [::std::mem::offset_of!(io_uring_getevents_arg, sigmask) - 0usize];
    ["Offset of field: io_uring_getevents_arg::sigmask_sz"]
        [::std::mem::offset_of!(io_uring_getevents_arg, sigmask_sz) - 8usize];
    ["Offset of field: io_uring_getevents_arg::min_wait_usec"]
        [::std::mem::offset_of!(io_uring_getevents_arg, min_wait_usec) - 12usize];
    ["Offset of field: io_uring_getevents_arg::ts"]
        [::std::mem::offset_of!(io_uring_getevents_arg, ts) - 16usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sync_cancel_reg"]
        [::std::mem::size_of::<io_uring_sync_cancel_reg>() - 64usize];
    ["Alignment of io_uring_sync_cancel_reg"]
        [::std::mem::align_of::<io_uring_sync_cancel_reg>() - 8usize];
    ["Offset of field: io_uring_sync_cancel_reg::addr"]
        [::std::mem::offset_of!(io_uring_sync_cancel_reg, addr) - 0usize];
    ["Offset of field: io_uring_sync_cancel_reg::fd"]
        [::std::mem::offset_of!(io_uring_sync_cancel_reg, fd) - 8usize];
    ["Offset of field: io_uring_sync_cancel_reg::flags"]
        [::std::mem::offset_of!(io_uring_sync_cancel_reg, flags) - 12usize];
    ["Offset of field: io_uring_sync_cancel_reg::timeout"]
        [::std::mem::offset_of!(io_uring_sync_cancel_reg, timeout) - 16usize];
    ["Offset of field: io_uring_sync_cancel_reg::opcode"]
        [::std::mem::offset_of!(io_uring_sync_cancel_reg, opcode) - 32usize];
    ["Offset of field: io_uring_sync_cancel_reg::pad"]
        [::std::mem::offset_of!(io_uring_sync_cancel_reg, pad) - 33usize];
    ["Offset of field: io_uring_sync_cancel_reg::pad2"]
        [::std::mem::offset_of!(io_uring_sync_cancel_reg, pad2) - 40usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_file_index_range"]
        [::std::mem::size_of::<io_uring_file_index_range>() - 16usize];
    ["Alignment of io_uring_file_index_range"]
        [::std::mem::align_of::<io_uring_file_index_range>() - 8usize];
    ["Offset of field: io_uring_file_index_range::off"]
        [::std::mem::offset_of!(io_uring_file_index_range, off) - 0usize];
    ["Offset of field: io_uring_file_index_range::len"]
        [::std::mem::offset_of!(io_uring_file_index_range, len) - 4usize];
    ["Offset of field: io_uring_file_index_range::resv"]
        [::std::mem::offset_of!(io_uring_file_index_range, resv) - 8usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_recvmsg_out"][::std::mem::size_of::<io_uring_recvmsg_out>() - 16usize];
    ["Alignment of io_uring_recvmsg_out"][::std::mem::align_of::<io_uring_recvmsg_out>() - 4usize];
    ["Offset of field: io_uring_recvmsg_out::namelen"]
        [::std::mem::offset_of!(io_uring_recvmsg_out, namelen) - 0usize];
    ["Offset of field: io_uring_recvmsg_out::controllen"]
        [::std::mem::offset_of!(io_uring_recvmsg_out, controllen) - 4usize];
    ["Offset of field: io_uring_recvmsg_out::payloadlen"]
        [::std::mem::offset_of!(io_uring_recvmsg_out, payloadlen) - 8usize];
    ["Offset of field: io_uring_recvmsg_out::flags"]
        [::std::mem::offset_of!(io_uring_recvmsg_out, flags) - 12usize];
};
pub const SOCKET_URING_OP_SIOCINQ: io_uring_socket_op = 0;
pub const SOCKET_URING_OP_SIOCOUTQ: io_uring_socket_op = 1;
pub const SOCKET_URING_OP_GETSOCKOPT: io_uring_socket_op = 2;
pub const SOCKET_URING_OP_SETSOCKOPT: io_uring_socket_op = 3;
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_sq"][::std::mem::size_of::<io_uring_sq>() - 104usize];
    ["Alignment of io_uring_sq"][::std::mem::align_of::<io_uring_sq>() - 8usize];
    ["Offset of field: io_uring_sq::khead"][::std::mem::offset_of!(io_uring_sq, khead) - 0usize];
    ["Offset of field: io_uring_sq::ktail"][::std::mem::offset_of!(io_uring_sq, ktail) - 8usize];
    ["Offset of field: io_uring_sq::kring_mask"]
        [::std::mem::offset_of!(io_uring_sq, kring_mask) - 16usize];
    ["Offset of field: io_uring_sq::kring_entries"]
        [::std::mem::offset_of!(io_uring_sq, kring_entries) - 24usize];
    ["Offset of field: io_uring_sq::kflags"][::std::mem::offset_of!(io_uring_sq, kflags) - 32usize];
    ["Offset of field: io_uring_sq::kdropped"]
        [::std::mem::offset_of!(io_uring_sq, kdropped) - 40usize];
    ["Offset of field: io_uring_sq::array"][::std::mem::offset_of!(io_uring_sq, array) - 48usize];
    ["Offset of field: io_uring_sq::sqes"][::std::mem::offset_of!(io_uring_sq, sqes) - 56usize];
    ["Offset of field: io_uring_sq::sqe_head"]
        [::std::mem::offset_of!(io_uring_sq, sqe_head) - 64usize];
    ["Offset of field: io_uring_sq::sqe_tail"]
        [::std::mem::offset_of!(io_uring_sq, sqe_tail) - 68usize];
    ["Offset of field: io_uring_sq::ring_sz"]
        [::std::mem::offset_of!(io_uring_sq, ring_sz) - 72usize];
    ["Offset of field: io_uring_sq::ring_ptr"]
        [::std::mem::offset_of!(io_uring_sq, ring_ptr) - 80usize];
    ["Offset of field: io_uring_sq::ring_mask"]
        [::std::mem::offset_of!(io_uring_sq, ring_mask) - 88usize];
    ["Offset of field: io_uring_sq::ring_entries"]
        [::std::mem::offset_of!(io_uring_sq, ring_entries) - 92usize];
    ["Offset of field: io_uring_sq::pad"][::std::mem::offset_of!(io_uring_sq, pad) - 96usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring_cq"][::std::mem::size_of::<io_uring_cq>() - 88usize];
    ["Alignment of io_uring_cq"][::std::mem::align_of::<io_uring_cq>() - 8usize];
    ["Offset of field: io_uring_cq::khead"][::std::mem::offset_of!(io_uring_cq, khead) - 0usize];
    ["Offset of field: io_uring_cq::ktail"][::std::mem::offset_of!(io_uring_cq, ktail) - 8usize];
    ["Offset of field: io_uring_cq::kring_mask"]
        [::std::mem::offset_of!(io_uring_cq, kring_mask) - 16usize];
    ["Offset of field: io_uring_cq::kring_entries"]
        [::std::mem::offset_of!(io_uring_cq, kring_entries) - 24usize];
    ["Offset of field: io_uring_cq::kflags"][::std::mem::offset_of!(io_uring_cq, kflags) - 32usize];
    ["Offset of field: io_uring_cq::koverflow"]
        [::std::mem::offset_of!(io_uring_cq, koverflow) - 40usize];
    ["Offset of field: io_uring_cq::cqes"][::std::mem::offset_of!(io_uring_cq, cqes) - 48usize];
    ["Offset of field: io_uring_cq::ring_sz"]
        [::std::mem::offset_of!(io_uring_cq, ring_sz) - 56usize];
    ["Offset of field: io_uring_cq::ring_ptr"]
        [::std::mem::offset_of!(io_uring_cq, ring_ptr) - 64usize];
    ["Offset of field: io_uring_cq::ring_mask"]
        [::std::mem::offset_of!(io_uring_cq, ring_mask) - 72usize];
    ["Offset of field: io_uring_cq::ring_entries"]
        [::std::mem::offset_of!(io_uring_cq, ring_entries) - 76usize];
    ["Offset of field: io_uring_cq::pad"][::std::mem::offset_of!(io_uring_cq, pad) - 80usize];
};
#[allow(clippy::unnecessary_operation, clippy::identity_op)]
const _: () = {
    ["Size of io_uring"][::std::mem::size_of::<io_uring>() - 216usize];
    ["Alignment of io_uring"][::std::mem::align_of::<io_uring>() - 8usize];
    ["Offset of field: io_uring::sq"][::std::mem::offset_of!(io_uring, sq) - 0usize];
    ["Offset of field: io_uring::cq"][::std::mem::offset_of!(io_uring, cq) - 104usize];
    ["Offset of field: io_uring::flags"][::std::mem::offset_of!(io_uring, flags) - 192usize];
    ["Offset of field: io_uring::ring_fd"][::std::mem::offset_of!(io_uring, ring_fd) - 196usize];
    ["Offset of field: io_uring::features"][::std::mem::offset_of!(io_uring, features) - 200usize];
    ["Offset of field: io_uring::enter_ring_fd"]
        [::std::mem::offset_of!(io_uring, enter_ring_fd) - 204usize];
    ["Offset of field: io_uring::int_flags"]
        [::std::mem::offset_of!(io_uring, int_flags) - 208usize];
    ["Offset of field: io_uring::pad"][::std::mem::offset_of!(io_uring, pad) - 209usize];
    ["Offset of field: io_uring::pad2"][::std::mem::offset_of!(io_uring, pad2) - 212usize];
};
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
    pub __bindgen_anon_1: io_uring_sqe__bindgen_ty_2__bindgen_ty_1,
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
    pub waitid_flags: __u32,
    pub futex_flags: __u32,
    pub install_fd_flags: __u32,
    pub nop_flags: __u32,
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
    pub optlen: __u32,
    pub __bindgen_anon_1: io_uring_sqe__bindgen_ty_5__bindgen_ty_1,
}
#[repr(C)]
pub union io_uring_sqe__bindgen_ty_6 {
    pub __bindgen_anon_1: ::std::mem::ManuallyDrop<io_uring_sqe__bindgen_ty_6__bindgen_ty_1>,
    pub optval: ::std::mem::ManuallyDrop<__u64>,
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
