use std::ffi::CString;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::fs::{FileType, Permissions, path_from_cstring};
use crate::kqueue::op::{DirectOp, DirectOpEtract};
use crate::{AsyncFd, SubmissionQueue, fd, syscall};

pub(crate) struct OpenOp;

impl DirectOp for OpenOp {
    type Output = AsyncFd;
    type Resources = (CString, fd::Kind); // path.
    type Args = (libc::c_int, libc::mode_t); // flags, mode.

    fn run(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        args: Self::Args,
    ) -> io::Result<Self::Output> {
        Ok(Self::run_extract(sq, resources, args)?.0)
    }
}

impl DirectOpEtract for OpenOp {
    type ExtractOutput = (AsyncFd, PathBuf);

    fn run_extract(
        sq: &SubmissionQueue,
        (path, kind): Self::Resources,
        (mut flags, mode): Self::Args,
    ) -> io::Result<Self::ExtractOutput> {
        let fd::Kind::File = kind;

        flags |= libc::O_NONBLOCK; // NOTE: O_CLOEXEC is already set.
        let fd = syscall!(open(path.as_ptr(), flags, mode as libc::c_int))?;
        // SAFETY: just created the fd above.
        let fd = unsafe { AsyncFd::from_raw(fd, kind, sq.clone()) };
        Ok((fd, path_from_cstring(path)))
    }
}

pub(crate) struct CreateDirOp;

impl DirectOp for CreateDirOp {
    type Output = ();
    type Resources = CString; // path.
    type Args = ();

    fn run(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        args: Self::Args,
    ) -> io::Result<Self::Output> {
        Self::run_extract(sq, resources, args)?;
        Ok(())
    }
}

impl DirectOpEtract for CreateDirOp {
    type ExtractOutput = PathBuf;

    fn run_extract(
        sq: &SubmissionQueue,
        path: Self::Resources,
        (): Self::Args,
    ) -> io::Result<Self::ExtractOutput> {
        let mode = 0o777; // Same as used by the standard library.
        syscall!(mkdirat(libc::AT_FDCWD, path.as_ptr(), mode))?;
        Ok(path_from_cstring(path))
    }
}

pub(crate) use libc::stat as Stat;

pub(crate) const fn file_type(stat: &Stat) -> FileType {
    FileType(stat.st_mode)
}

pub(crate) const fn len(stat: &Stat) -> u64 {
    stat.st_size as _
}

pub(crate) const fn block_size(stat: &Stat) -> u32 {
    stat.st_blksize as _
}

pub(crate) const fn permissions(stat: &Stat) -> Permissions {
    Permissions(stat.st_mode)
}

pub(crate) fn modified(stat: &Stat) -> SystemTime {
    timestamp(stat.st_mtime, stat.st_mtime_nsec)
}

pub(crate) fn accessed(stat: &Stat) -> SystemTime {
    timestamp(stat.st_atime, stat.st_atime_nsec)
}

pub(crate) fn created(stat: &Stat) -> SystemTime {
    timestamp(stat.st_birthtime, stat.st_birthtime_nsec)
}

#[allow(clippy::cast_sign_loss)] // Checked.
fn timestamp(secs: libc::time_t, nsec: libc::c_long) -> SystemTime {
    let mut time = SystemTime::UNIX_EPOCH;
    let dur = Duration::from_secs(secs as u64);
    let nanos = Duration::from_nanos(nsec as u64);
    if secs.is_positive() {
        time += dur;
    } else {
        time -= dur;
    }
    if nsec.is_positive() {
        time += nanos;
    } else {
        time -= nanos;
    }
    time
}
