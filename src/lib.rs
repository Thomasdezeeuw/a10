// This must come before the other modules for the documentation.
pub mod fd;

mod sq;

#[cfg(any(target_os = "android", target_os = "linux"))]
mod io_uring;
#[cfg(any(target_os = "android", target_os = "linux"))]
use io_uring as sys;

#[doc(no_inline)]
pub use fd::AsyncFd;
#[doc(inline)]
pub use sq::SubmissionQueue;

/// Helper macro to execute a system call that returns an `io::Result`.
macro_rules! syscall {
    ($fn: ident ( $($arg: expr),* $(,)? ) ) => {{
        #[allow(unused_unsafe)]
        let res = unsafe { ::libc::$fn($( $arg, )*) };
        if res == -1 {
            ::std::result::Result::Err(::std::io::Error::last_os_error())
        } else {
            ::std::result::Result::Ok(res)
        }
    }};
}

#[allow(unused_imports)] // Not used on all OS.
use syscall;
