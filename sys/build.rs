fn main() {
    println!("cargo:rerun-if-changed=include/liburing.h");

    let bindings = bindgen::Builder::default()
        // From `liburing/src/include/liburing.h`.
        .header("include/liburing.h")
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .impl_debug(false)
        .impl_partialeq(false)
        .derive_copy(true)
        .derive_debug(false)
        .derive_default(false)
        .derive_hash(false)
        .derive_partialord(false)
        .derive_ord(false)
        .derive_partialeq(false)
        .derive_eq(false)
        .merge_extern_blocks(true)
        .default_non_copy_union_style(bindgen::NonCopyUnionStyle::ManuallyDrop)
        .prepend_enum_name(false)
        .rustfmt_bindings(true)
        .sort_semantically(true)
        // Limit to io_uring types and constants.
        .allowlist_type("io_uring.*")
        .allowlist_var("IORING.*")
        // We define the function ourselves, since no definitions exist in libc
        // yet (otherwise this wasn't needed at all!).
        .ignore_functions()
        // We'll use the libc definition.
        .blocklist_item("sigset_t")
        // Add our header with the `syscall!` macro and module docs.
        .raw_line(HEADER.trim())
        .disable_header_comment()
        .generate()
        .expect("failed to generate bindings");

    bindings
        .write_to_file("src/sys.rs")
        .expect("failed to write generated bindings");
}

/// Code added at the top of the generated file.
const HEADER: &str = "
//! Code that should be moved to libc once C libraries have a wrapper.

#![allow(dead_code, non_camel_case_types)]
#![allow(clippy::unreadable_literal, clippy::missing_safety_doc)]

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

pub use syscall;
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
";
