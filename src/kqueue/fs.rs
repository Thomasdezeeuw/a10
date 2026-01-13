use std::ffi::CString;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::fs::{
    FileType, Metadata, MetadataInterest, Permissions, RemoveFlag, SyncDataFlag, path_from_cstring,
};
use crate::kqueue::op::{DirectFdOp, DirectOp, DirectOpEtract, impl_fd_op};
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

pub(crate) struct RenameOp;

impl DirectOp for RenameOp {
    type Output = ();
    type Resources = (CString, CString); // from path, to path
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

impl DirectOpEtract for RenameOp {
    type ExtractOutput = (PathBuf, PathBuf);

    fn run_extract(
        sq: &SubmissionQueue,
        (from, to): Self::Resources,
        (): Self::Args,
    ) -> io::Result<Self::ExtractOutput> {
        syscall!(renameat(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr()
        ))?;
        Ok((path_from_cstring(from), path_from_cstring(to)))
    }
}

pub(crate) struct DeleteOp;

impl DirectOp for DeleteOp {
    type Output = ();
    type Resources = CString; // path
    type Args = RemoveFlag;

    fn run(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        args: Self::Args,
    ) -> io::Result<Self::Output> {
        Self::run_extract(sq, resources, args)?;
        Ok(())
    }
}

impl DirectOpEtract for DeleteOp {
    type ExtractOutput = PathBuf;

    fn run_extract(
        sq: &SubmissionQueue,
        path: Self::Resources,
        flags: Self::Args,
    ) -> io::Result<Self::ExtractOutput> {
        let flags = match flags {
            RemoveFlag::File => 0,
            RemoveFlag::Directory => libc::AT_REMOVEDIR,
        };
        syscall!(unlinkat(libc::AT_FDCWD, path.as_ptr(), flags))?;
        Ok(path_from_cstring(path))
    }
}

pub(crate) struct SyncDataOp;

impl DirectFdOp for SyncDataOp {
    type Output = ();
    type Resources = ();
    type Args = SyncDataFlag;

    fn run(fd: &AsyncFd, (): Self::Resources, flag: Self::Args) -> io::Result<Self::Output> {
        match flag {
            #[cfg(not(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            )))]
            SyncDataFlag::All => syscall!(fsync(fd.fd()))?,
            #[cfg(not(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            )))]
            SyncDataFlag::Data => syscall!(fdatasync(fd.fd()))?,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            SyncDataFlag::All | SyncDataFlag::Data => {
                syscall!(fcntl(fd.fd(), libc::F_FULLFSYNC, 0))?
            }
        };
        Ok(())
    }
}

impl_fd_op!(SyncDataOp);

pub(crate) const fn default_metadata_interest() -> MetadataInterest {
    MetadataInterest(0)
}

pub(crate) struct StatOp;

impl DirectFdOp for StatOp {
    type Output = Metadata;
    type Resources = Metadata;
    type Args = MetadataInterest;

    fn run(fd: &AsyncFd, mut metadata: Self::Resources, _: Self::Args) -> io::Result<Self::Output> {
        syscall!(fstat(fd.fd(), &mut metadata.0))?;
        Ok(metadata)
    }
}

impl_fd_op!(StatOp);

pub(crate) struct TruncateOp;

impl DirectFdOp for TruncateOp {
    type Output = ();
    type Resources = ();
    type Args = u64; // length

    fn run(fd: &AsyncFd, (): Self::Resources, length: Self::Args) -> io::Result<Self::Output> {
        syscall!(ftruncate(fd.fd(), length.cast_signed()))?;
        Ok(())
    }
}

impl_fd_op!(TruncateOp);

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
