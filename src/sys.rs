//! Code that should be moved to libc once C libraries have a wrapper.

#![allow(
    non_camel_case_types,
    non_upper_case_globals,
    non_snake_case,
    dead_code
)]
#![allow(clippy::unreadable_literal, clippy::missing_safety_doc)]
#![cfg_attr(test, allow(deref_nullptr, unaligned_references))]

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

pub(crate) use syscall;

pub use libc::*;

pub const IORING_ENTER_GETEVENTS: u32 = 1;
pub const IORING_ENTER_SQ_WAKEUP: u32 = 2;
pub const IORING_ENTER_SQ_WAIT: u32 = 4;
pub const IORING_ENTER_EXT_ARG: u32 = 8;
pub const IORING_ENTER_REGISTERED_RING: u32 = 16;

pub const IORING_SQ_NEED_WAKEUP: u32 = 1;
pub const IORING_SQ_CQ_OVERFLOW: u32 = 2;
pub const IORING_SQ_TASKRUN: u32 = 4;

pub const IORING_OP_NOP: libc::c_int = 0;
pub const IORING_OP_READV: libc::c_int = 1;
pub const IORING_OP_WRITEV: libc::c_int = 2;
pub const IORING_OP_FSYNC: libc::c_int = 3;
pub const IORING_OP_READ_FIXED: libc::c_int = 4;
pub const IORING_OP_WRITE_FIXED: libc::c_int = 5;
pub const IORING_OP_POLL_ADD: libc::c_int = 6;
pub const IORING_OP_POLL_REMOVE: libc::c_int = 7;
pub const IORING_OP_SYNC_FILE_RANGE: libc::c_int = 8;
pub const IORING_OP_SENDMSG: libc::c_int = 9;
pub const IORING_OP_RECVMSG: libc::c_int = 10;
pub const IORING_OP_TIMEOUT: libc::c_int = 11;
pub const IORING_OP_TIMEOUT_REMOVE: libc::c_int = 12;
pub const IORING_OP_ACCEPT: libc::c_int = 13;
pub const IORING_OP_ASYNC_CANCEL: libc::c_int = 14;
pub const IORING_OP_LINK_TIMEOUT: libc::c_int = 15;
pub const IORING_OP_CONNECT: libc::c_int = 16;
pub const IORING_OP_FALLOCATE: libc::c_int = 17;
pub const IORING_OP_OPENAT: libc::c_int = 18;
pub const IORING_OP_CLOSE: libc::c_int = 19;
pub const IORING_OP_RSRC_UPDATE: libc::c_int = 20;
pub const IORING_OP_FILES_UPDATE: libc::c_int = 20;
pub const IORING_OP_STATX: libc::c_int = 21;
pub const IORING_OP_READ: libc::c_int = 22;
pub const IORING_OP_WRITE: libc::c_int = 23;
pub const IORING_OP_FADVISE: libc::c_int = 24;
pub const IORING_OP_MADVISE: libc::c_int = 25;
pub const IORING_OP_SEND: libc::c_int = 26;
pub const IORING_OP_RECV: libc::c_int = 27;
pub const IORING_OP_OPENAT2: libc::c_int = 28;
pub const IORING_OP_EPOLL_CTL: libc::c_int = 29;
pub const IORING_OP_SPLICE: libc::c_int = 30;
pub const IORING_OP_PROVIDE_BUFFERS: libc::c_int = 31;
pub const IORING_OP_REMOVE_BUFFERS: libc::c_int = 32;
pub const IORING_OP_TEE: libc::c_int = 33;
pub const IORING_OP_SHUTDOWN: libc::c_int = 34;
pub const IORING_OP_RENAMEAT: libc::c_int = 35;
pub const IORING_OP_UNLINKAT: libc::c_int = 36;
pub const IORING_OP_MKDIRAT: libc::c_int = 37;
pub const IORING_OP_SYMLINKAT: libc::c_int = 38;
pub const IORING_OP_LINKAT: libc::c_int = 39;
pub const IORING_OP_MSG_RING: libc::c_int = 40;
pub const IORING_OP_FSETXATTR: libc::c_int = 41;
pub const IORING_OP_SETXATTR: libc::c_int = 42;
pub const IORING_OP_FGETXATTR: libc::c_int = 43;
pub const IORING_OP_GETXATTR: libc::c_int = 44;
pub const IORING_OP_SOCKET: libc::c_int = 45;
pub const IORING_OP_URING_CMD: libc::c_int = 46;
pub const IORING_OP_SENDZC_NOTIF: libc::c_int = 47;
pub const IORING_OP_LAST: libc::c_int = 48;

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
pub const IORING_FSYNC_DATASYNC: u32 = 1;
pub const IORING_TIMEOUT_ABS: u32 = 1;
pub const IORING_TIMEOUT_UPDATE: u32 = 2;
pub const IORING_TIMEOUT_BOOTTIME: u32 = 4;
pub const IORING_TIMEOUT_REALTIME: u32 = 8;
pub const IORING_LINK_TIMEOUT_UPDATE: u32 = 16;
pub const IORING_TIMEOUT_ETIME_SUCCESS: u32 = 32;
pub const IORING_TIMEOUT_CLOCK_MASK: u32 = 12;
pub const IORING_TIMEOUT_UPDATE_MASK: u32 = 18;
pub const SPLICE_F_FD_IN_FIXED: u32 = 2147483648;
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
pub const IORING_RECVSEND_NOTIF_FLUSH: u32 = 8;
pub const IORING_ACCEPT_MULTISHOT: u32 = 1;
pub const IORING_MSG_RING_CQE_SKIP: u32 = 1;
pub const IORING_CQE_F_BUFFER: u32 = 1;
pub const IORING_CQE_F_MORE: u32 = 2;
pub const IORING_CQE_F_SOCK_NONEMPTY: u32 = 4;
pub const IORING_OFF_SQ_RING: u32 = 0;
pub const IORING_OFF_CQ_RING: u32 = 134217728;
pub const IORING_OFF_SQES: u32 = 268435456;
pub const IORING_CQ_EVENTFD_DISABLED: u32 = 1;
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

pub unsafe fn io_uring_enter(
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

/* automatically generated by rust-bindgen 0.60.1 */

#[repr(C)]
#[derive(Debug, Copy, Clone)]
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

#[test]
fn bindgen_test_layout_io_uring_params() {
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
    fn test_field_sq_entries() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).sq_entries) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(sq_entries)
            )
        );
    }
    test_field_sq_entries();
    fn test_field_cq_entries() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).cq_entries) as usize - ptr as usize
            },
            4usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(cq_entries)
            )
        );
    }
    test_field_cq_entries();
    fn test_field_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize
            },
            8usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(flags)
            )
        );
    }
    test_field_flags();
    fn test_field_sq_thread_cpu() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).sq_thread_cpu) as usize - ptr as usize
            },
            12usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(sq_thread_cpu)
            )
        );
    }
    test_field_sq_thread_cpu();
    fn test_field_sq_thread_idle() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).sq_thread_idle) as usize - ptr as usize
            },
            16usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(sq_thread_idle)
            )
        );
    }
    test_field_sq_thread_idle();
    fn test_field_features() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).features) as usize - ptr as usize
            },
            20usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(features)
            )
        );
    }
    test_field_features();
    fn test_field_wq_fd() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).wq_fd) as usize - ptr as usize
            },
            24usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(wq_fd)
            )
        );
    }
    test_field_wq_fd();
    fn test_field_resv() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).resv) as usize - ptr as usize
            },
            28usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(resv)
            )
        );
    }
    test_field_resv();
    fn test_field_sq_off() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).sq_off) as usize - ptr as usize
            },
            40usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(sq_off)
            )
        );
    }
    test_field_sq_off();
    fn test_field_cq_off() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_params>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).cq_off) as usize - ptr as usize
            },
            80usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_params),
                "::",
                stringify!(cq_off)
            )
        );
    }
    test_field_cq_off();
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
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

#[test]
fn bindgen_test_layout_io_sqring_offsets() {
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
    fn test_field_head() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_sqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).head) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_sqring_offsets),
                "::",
                stringify!(head)
            )
        );
    }
    test_field_head();
    fn test_field_tail() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_sqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).tail) as usize - ptr as usize
            },
            4usize,
            concat!(
                "Offset of field: ",
                stringify!(io_sqring_offsets),
                "::",
                stringify!(tail)
            )
        );
    }
    test_field_tail();
    fn test_field_ring_mask() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_sqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).ring_mask) as usize - ptr as usize
            },
            8usize,
            concat!(
                "Offset of field: ",
                stringify!(io_sqring_offsets),
                "::",
                stringify!(ring_mask)
            )
        );
    }
    test_field_ring_mask();
    fn test_field_ring_entries() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_sqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).ring_entries) as usize - ptr as usize
            },
            12usize,
            concat!(
                "Offset of field: ",
                stringify!(io_sqring_offsets),
                "::",
                stringify!(ring_entries)
            )
        );
    }
    test_field_ring_entries();
    fn test_field_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_sqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize
            },
            16usize,
            concat!(
                "Offset of field: ",
                stringify!(io_sqring_offsets),
                "::",
                stringify!(flags)
            )
        );
    }
    test_field_flags();
    fn test_field_dropped() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_sqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).dropped) as usize - ptr as usize
            },
            20usize,
            concat!(
                "Offset of field: ",
                stringify!(io_sqring_offsets),
                "::",
                stringify!(dropped)
            )
        );
    }
    test_field_dropped();
    fn test_field_array() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_sqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).array) as usize - ptr as usize
            },
            24usize,
            concat!(
                "Offset of field: ",
                stringify!(io_sqring_offsets),
                "::",
                stringify!(array)
            )
        );
    }
    test_field_array();
    fn test_field_resv1() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_sqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).resv1) as usize - ptr as usize
            },
            28usize,
            concat!(
                "Offset of field: ",
                stringify!(io_sqring_offsets),
                "::",
                stringify!(resv1)
            )
        );
    }
    test_field_resv1();
    fn test_field_resv2() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_sqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize
            },
            32usize,
            concat!(
                "Offset of field: ",
                stringify!(io_sqring_offsets),
                "::",
                stringify!(resv2)
            )
        );
    }
    test_field_resv2();
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
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

#[test]
fn bindgen_test_layout_io_cqring_offsets() {
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
    fn test_field_head() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_cqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).head) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_cqring_offsets),
                "::",
                stringify!(head)
            )
        );
    }
    test_field_head();
    fn test_field_tail() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_cqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).tail) as usize - ptr as usize
            },
            4usize,
            concat!(
                "Offset of field: ",
                stringify!(io_cqring_offsets),
                "::",
                stringify!(tail)
            )
        );
    }
    test_field_tail();
    fn test_field_ring_mask() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_cqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).ring_mask) as usize - ptr as usize
            },
            8usize,
            concat!(
                "Offset of field: ",
                stringify!(io_cqring_offsets),
                "::",
                stringify!(ring_mask)
            )
        );
    }
    test_field_ring_mask();
    fn test_field_ring_entries() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_cqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).ring_entries) as usize - ptr as usize
            },
            12usize,
            concat!(
                "Offset of field: ",
                stringify!(io_cqring_offsets),
                "::",
                stringify!(ring_entries)
            )
        );
    }
    test_field_ring_entries();
    fn test_field_overflow() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_cqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).overflow) as usize - ptr as usize
            },
            16usize,
            concat!(
                "Offset of field: ",
                stringify!(io_cqring_offsets),
                "::",
                stringify!(overflow)
            )
        );
    }
    test_field_overflow();
    fn test_field_cqes() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_cqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).cqes) as usize - ptr as usize
            },
            20usize,
            concat!(
                "Offset of field: ",
                stringify!(io_cqring_offsets),
                "::",
                stringify!(cqes)
            )
        );
    }
    test_field_cqes();
    fn test_field_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_cqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize
            },
            24usize,
            concat!(
                "Offset of field: ",
                stringify!(io_cqring_offsets),
                "::",
                stringify!(flags)
            )
        );
    }
    test_field_flags();
    fn test_field_resv1() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_cqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).resv1) as usize - ptr as usize
            },
            28usize,
            concat!(
                "Offset of field: ",
                stringify!(io_cqring_offsets),
                "::",
                stringify!(resv1)
            )
        );
    }
    test_field_resv1();
    fn test_field_resv2() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_cqring_offsets>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).resv2) as usize - ptr as usize
            },
            32usize,
            concat!(
                "Offset of field: ",
                stringify!(io_cqring_offsets),
                "::",
                stringify!(resv2)
            )
        );
    }
    test_field_resv2();
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct io_uring_cqe {
    pub user_data: __u64,
    pub res: __s32,
    pub flags: __u32,
    pub big_cqe: __IncompleteArrayField<__u64>,
}

#[test]
fn bindgen_test_layout_io_uring_cqe() {
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
    fn test_field_user_data() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_cqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).user_data) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_cqe),
                "::",
                stringify!(user_data)
            )
        );
    }
    test_field_user_data();
    fn test_field_res() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_cqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).res) as usize - ptr as usize
            },
            8usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_cqe),
                "::",
                stringify!(res)
            )
        );
    }
    test_field_res();
    fn test_field_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_cqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize
            },
            12usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_cqe),
                "::",
                stringify!(flags)
            )
        );
    }
    test_field_flags();
    fn test_field_big_cqe() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_cqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).big_cqe) as usize - ptr as usize
            },
            16usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_cqe),
                "::",
                stringify!(big_cqe)
            )
        );
    }
    test_field_big_cqe();
}

#[repr(C)]
#[derive(Copy, Clone)]
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
pub union io_uring_sqe__bindgen_ty_1 {
    pub off: __u64,
    pub addr2: __u64,
    pub __bindgen_anon_1: io_uring_sqe__bindgen_ty_1__bindgen_ty_1,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct io_uring_sqe__bindgen_ty_1__bindgen_ty_1 {
    pub cmd_op: __u32,
    pub __pad1: __u32,
}

#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_1__bindgen_ty_1() {
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
    fn test_field_cmd_op() {
        assert_eq!(
            unsafe {
                let uninit =
                    ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_1__bindgen_ty_1>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).cmd_op) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_1__bindgen_ty_1),
                "::",
                stringify!(cmd_op)
            )
        );
    }
    test_field_cmd_op();
    fn test_field___pad1() {
        assert_eq!(
            unsafe {
                let uninit =
                    ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_1__bindgen_ty_1>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).__pad1) as usize - ptr as usize
            },
            4usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_1__bindgen_ty_1),
                "::",
                stringify!(__pad1)
            )
        );
    }
    test_field___pad1();
}

#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_1() {
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
    fn test_field_off() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_1>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).off) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_1),
                "::",
                stringify!(off)
            )
        );
    }
    test_field_off();
    fn test_field_addr2() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_1>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).addr2) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_1),
                "::",
                stringify!(addr2)
            )
        );
    }
    test_field_addr2();
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union io_uring_sqe__bindgen_ty_2 {
    pub addr: __u64,
    pub splice_off_in: __u64,
}

#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_2() {
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
    fn test_field_addr() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_2>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).addr) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_2),
                "::",
                stringify!(addr)
            )
        );
    }
    test_field_addr();
    fn test_field_splice_off_in() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_2>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).splice_off_in) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_2),
                "::",
                stringify!(splice_off_in)
            )
        );
    }
    test_field_splice_off_in();
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
}

#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_3() {
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
    fn test_field_rw_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).rw_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(rw_flags)
            )
        );
    }
    test_field_rw_flags();
    fn test_field_fsync_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).fsync_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(fsync_flags)
            )
        );
    }
    test_field_fsync_flags();
    fn test_field_poll_events() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).poll_events) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(poll_events)
            )
        );
    }
    test_field_poll_events();
    fn test_field_poll32_events() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).poll32_events) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(poll32_events)
            )
        );
    }
    test_field_poll32_events();
    fn test_field_sync_range_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).sync_range_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(sync_range_flags)
            )
        );
    }
    test_field_sync_range_flags();
    fn test_field_msg_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).msg_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(msg_flags)
            )
        );
    }
    test_field_msg_flags();
    fn test_field_timeout_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).timeout_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(timeout_flags)
            )
        );
    }
    test_field_timeout_flags();
    fn test_field_accept_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).accept_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(accept_flags)
            )
        );
    }
    test_field_accept_flags();
    fn test_field_cancel_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).cancel_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(cancel_flags)
            )
        );
    }
    test_field_cancel_flags();
    fn test_field_open_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).open_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(open_flags)
            )
        );
    }
    test_field_open_flags();
    fn test_field_statx_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).statx_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(statx_flags)
            )
        );
    }
    test_field_statx_flags();
    fn test_field_fadvise_advice() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).fadvise_advice) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(fadvise_advice)
            )
        );
    }
    test_field_fadvise_advice();
    fn test_field_splice_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).splice_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(splice_flags)
            )
        );
    }
    test_field_splice_flags();
    fn test_field_rename_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).rename_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(rename_flags)
            )
        );
    }
    test_field_rename_flags();
    fn test_field_unlink_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).unlink_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(unlink_flags)
            )
        );
    }
    test_field_unlink_flags();
    fn test_field_hardlink_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).hardlink_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(hardlink_flags)
            )
        );
    }
    test_field_hardlink_flags();
    fn test_field_xattr_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).xattr_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(xattr_flags)
            )
        );
    }
    test_field_xattr_flags();
    fn test_field_msg_ring_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_3>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).msg_ring_flags) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_3),
                "::",
                stringify!(msg_ring_flags)
            )
        );
    }
    test_field_msg_ring_flags();
}

pub type __kernel_rwf_t = ::std::os::raw::c_int;

#[repr(C, packed)]
#[derive(Copy, Clone)]
pub union io_uring_sqe__bindgen_ty_4 {
    pub buf_index: __u16,
    pub buf_group: __u16,
}

#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_4() {
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
    fn test_field_buf_index() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_4>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).buf_index) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_4),
                "::",
                stringify!(buf_index)
            )
        );
    }
    test_field_buf_index();
    fn test_field_buf_group() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_4>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).buf_group) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_4),
                "::",
                stringify!(buf_group)
            )
        );
    }
    test_field_buf_group();
}

#[repr(C)]
#[derive(Copy, Clone)]
pub union io_uring_sqe__bindgen_ty_5 {
    pub splice_fd_in: __s32,
    pub file_index: __u32,
    pub __bindgen_anon_1: io_uring_sqe__bindgen_ty_5__bindgen_ty_1,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct io_uring_sqe__bindgen_ty_5__bindgen_ty_1 {
    pub notification_idx: __u16,
    pub addr_len: __u16,
}

#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_5__bindgen_ty_1() {
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
    fn test_field_notification_idx() {
        assert_eq!(
            unsafe {
                let uninit =
                    ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_5__bindgen_ty_1>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).notification_idx) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_5__bindgen_ty_1),
                "::",
                stringify!(notification_idx)
            )
        );
    }
    test_field_notification_idx();
    fn test_field_addr_len() {
        assert_eq!(
            unsafe {
                let uninit =
                    ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_5__bindgen_ty_1>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).addr_len) as usize - ptr as usize
            },
            2usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_5__bindgen_ty_1),
                "::",
                stringify!(addr_len)
            )
        );
    }
    test_field_addr_len();
}

#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_5() {
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
    fn test_field_splice_fd_in() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_5>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).splice_fd_in) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_5),
                "::",
                stringify!(splice_fd_in)
            )
        );
    }
    test_field_splice_fd_in();
    fn test_field_file_index() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_5>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).file_index) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_5),
                "::",
                stringify!(file_index)
            )
        );
    }
    test_field_file_index();
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct io_uring_sqe__bindgen_ty_6 {
    pub __bindgen_anon_1: __BindgenUnionField<io_uring_sqe__bindgen_ty_6__bindgen_ty_1>,
    pub cmd: __BindgenUnionField<[__u8; 0usize]>,
    pub bindgen_union_field: [u64; 2usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct io_uring_sqe__bindgen_ty_6__bindgen_ty_1 {
    pub addr3: __u64,
    pub __pad2: [__u64; 1usize],
}

#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_6__bindgen_ty_1() {
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
    fn test_field_addr3() {
        assert_eq!(
            unsafe {
                let uninit =
                    ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_6__bindgen_ty_1>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).addr3) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_6__bindgen_ty_1),
                "::",
                stringify!(addr3)
            )
        );
    }
    test_field_addr3();
    fn test_field___pad2() {
        assert_eq!(
            unsafe {
                let uninit =
                    ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_6__bindgen_ty_1>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).__pad2) as usize - ptr as usize
            },
            8usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_6__bindgen_ty_1),
                "::",
                stringify!(__pad2)
            )
        );
    }
    test_field___pad2();
}

#[test]
fn bindgen_test_layout_io_uring_sqe__bindgen_ty_6() {
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
    fn test_field_cmd() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe__bindgen_ty_6>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).cmd) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe__bindgen_ty_6),
                "::",
                stringify!(cmd)
            )
        );
    }
    test_field_cmd();
}

#[test]
fn bindgen_test_layout_io_uring_sqe() {
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
    fn test_field_opcode() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).opcode) as usize - ptr as usize
            },
            0usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe),
                "::",
                stringify!(opcode)
            )
        );
    }
    test_field_opcode();
    fn test_field_flags() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).flags) as usize - ptr as usize
            },
            1usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe),
                "::",
                stringify!(flags)
            )
        );
    }
    test_field_flags();
    fn test_field_ioprio() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).ioprio) as usize - ptr as usize
            },
            2usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe),
                "::",
                stringify!(ioprio)
            )
        );
    }
    test_field_ioprio();
    fn test_field_fd() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).fd) as usize - ptr as usize
            },
            4usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe),
                "::",
                stringify!(fd)
            )
        );
    }
    test_field_fd();
    fn test_field_len() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).len) as usize - ptr as usize
            },
            24usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe),
                "::",
                stringify!(len)
            )
        );
    }
    test_field_len();
    fn test_field_user_data() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).user_data) as usize - ptr as usize
            },
            32usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe),
                "::",
                stringify!(user_data)
            )
        );
    }
    test_field_user_data();
    fn test_field_personality() {
        assert_eq!(
            unsafe {
                let uninit = ::std::mem::MaybeUninit::<io_uring_sqe>::uninit();
                let ptr = uninit.as_ptr();
                ::std::ptr::addr_of!((*ptr).personality) as usize - ptr as usize
            },
            42usize,
            concat!(
                "Offset of field: ",
                stringify!(io_uring_sqe),
                "::",
                stringify!(personality)
            )
        );
    }
    test_field_personality();
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __IncompleteArrayField<T>(::std::marker::PhantomData<T>, [T; 0]);

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct __BindgenUnionField<T>(::std::marker::PhantomData<T>);
