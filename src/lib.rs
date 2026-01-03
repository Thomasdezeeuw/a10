#[cfg(any(target_os = "android", target_os = "linux"))]
mod io_uring;
#[cfg(any(target_os = "android", target_os = "linux"))]
use io_uring as sys;
