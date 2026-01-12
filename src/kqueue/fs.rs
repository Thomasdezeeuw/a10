use std::time::{Duration, SystemTime};

use crate::fs::{FileType, Permissions};

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
