//! Filesystem notifications.

use std::borrow::{Borrow, Cow};
use std::collections::HashMap;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io, mem, ptr};

use crate::fs::notify::{self, Events, Interest, Recursive, Watcher};
use crate::kqueue::fd::OpKind;
use crate::kqueue::op::{Evented, FdIter};
use crate::kqueue::{self, kqueue};
use crate::op::{FdIter as _, OpState};
use crate::{AsyncFd, SubmissionQueue, syscall};

/// The watch descriptors (wds) and the path to the file or directory they are
/// watching.
pub(crate) type Watching = HashMap<WatchedFd, PathBufWithNull>;

/// A valid [`PathBuf`] null terminated string, encoding is OS specific, but
/// it's always a valid [`PathBuf`]/[`Path`].
type PathBufWithNull = CString;

/// Regular fd that is opened only for being monitored.
pub(crate) struct WatchedFd(OwnedFd);

impl WatchedFd {
    fn open(path: &CStr) -> io::Result<WatchedFd> {
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        #[cfg(any(
            target_os = "dragonfly",
            target_os = "ios",
            target_os = "macos",
            target_os = "netbsd",
            target_os = "tvos",
            target_os = "visionos",
            target_os = "watchos",
        ))]
        let flags = flags | libc::O_EVTONLY;
        let fd = syscall!(openat(libc::AT_FDCWD, path.as_ptr(), flags))?;
        Ok(WatchedFd(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
}

// NOTE: needed for HashMap in Watching.
impl Eq for WatchedFd {}

// NOTE: needed for HashMap in Watching.
impl PartialEq for WatchedFd {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_raw_fd() == other.0.as_raw_fd()
    }
}

// NOTE: needed for HashMap in Watching.
impl Hash for WatchedFd {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.as_raw_fd().hash(state)
    }
}

// NOTE: needed for HashMap in Watching.
impl Borrow<RawFd> for WatchedFd {
    fn borrow(&self) -> &RawFd {
        // SAFETY: cast is safe due to repr(transparent) on OwnedFd.
        unsafe { &*ptr::from_ref(self).cast::<RawFd>() }
    }
}

impl fmt::Debug for WatchedFd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.as_raw_fd().fmt(f)
    }
}

impl Watcher {
    pub(crate) fn new_sys(sq: SubmissionQueue) -> io::Result<Watcher> {
        let kq = kqueue()?;
        let fd = AsyncFd::new(kq, sq);
        Ok(Watcher {
            fd,
            watching: HashMap::new(),
        })
    }
}

pub(crate) fn watch_recursive(
    kq: &AsyncFd,
    watching: &mut Watching,
    dir: PathBuf,
    interest: Interest,
    recursive: Recursive,
    dir_only: bool,
) -> io::Result<()> {
    match std::fs::read_dir(&dir) {
        Ok(read_dir) => {
            for result in read_dir {
                let entry = result?;
                let path = entry.path();
                if let Recursive::All = recursive
                    && entry.file_type()?.is_dir()
                {
                    watch_recursive(kq, watching, path, interest, Recursive::All, true)?;
                } else {
                    watch_path(kq, watching, path, interest)?;
                }
            }
        }
        Err(ref err) if !dir_only && err.kind() == io::ErrorKind::NotADirectory => {
            // Ignore the error.
        }
        Err(err) => return Err(err),
    }

    watch_path(kq, watching, dir, interest)
}

pub(crate) fn watch(
    kq: &AsyncFd,
    watching: &mut Watching,
    path: PathBuf,
    interest: Interest,
) -> io::Result<()> {
    // To minic inotify we need to watch all files and directories within a
    // watched directory, so we use watch_recursive to do that for us.
    watch_recursive(kq, watching, path, interest, Recursive::No, false)
}

fn watch_path(
    kq: &AsyncFd,
    watching: &mut Watching,
    path: PathBuf,
    interest: Interest,
) -> io::Result<()> {
    let path =
        unsafe { PathBufWithNull::from_vec_unchecked(OsString::from(path).into_encoded_bytes()) };
    let fd = WatchedFd::open(&path)?;
    let change = libc::kevent {
        ident: fd.0.as_raw_fd().cast_unsigned() as _,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD,
        fflags: interest.0,
        // SAFETY: all zeros is valid for `kevent`.
        ..unsafe { mem::zeroed() }
    };
    syscall!(kevent(
        kq.fd(),
        &raw const change,
        1,
        ptr::null_mut(),
        0,
        ptr::null(),
    ))?;
    // NOTE: it's possible the `wd` is already watched, we'll overwrite the
    // path, the watched interested is combined (within the kernel).
    _ = watching.insert(fd, path);
    Ok(())
}

pub(crate) const INTEREST_ALL: u32 = 0
    | INTEREST_ACCESS
    | INTEREST_MODIFY
    | INTEREST_METADATA
    | INTEREST_CLOSE
    | INTEREST_OPEN
    | INTEREST_MOVE
    | INTEREST_CREATE
    | INTEREST_DELETE
    | INTEREST_DELETE_SELF
    | INTEREST_MOVE_SELF;
#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const INTEREST_ACCESS: u32 = libc::NOTE_READ;
#[cfg(not(any(target_os = "freebsd", target_os = "netbsd")))]
const INTEREST_ACCESS: u32 = 0; // Not supported.
pub(crate) const INTEREST_MODIFY: u32 = libc::NOTE_WRITE;
pub(crate) const INTEREST_METADATA: u32 = libc::NOTE_ATTRIB;
#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const INTEREST_CLOSE_WRITE: u32 = libc::NOTE_CLOSE_WRITE;
#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const INTEREST_CLOSE_NOWRITE: u32 = libc::NOTE_CLOSE;
#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const INTEREST_CLOSE: u32 = INTEREST_CLOSE_WRITE | INTEREST_CLOSE_NOWRITE;
#[cfg(not(any(target_os = "freebsd", target_os = "netbsd")))]
const INTEREST_CLOSE: u32 = 0; // Not supported.
#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const INTEREST_OPEN: u32 = libc::NOTE_OPEN;
#[cfg(not(any(target_os = "freebsd", target_os = "netbsd")))]
const INTEREST_OPEN: u32 = 0; // Not supported.
pub(crate) const INTEREST_MOVE: u32 = libc::NOTE_RENAME;
pub(crate) const INTEREST_CREATE: u32 = libc::NOTE_EXTEND;
pub(crate) const INTEREST_DELETE: u32 = libc::NOTE_DELETE | libc::NOTE_LINK;
pub(crate) const INTEREST_DELETE_SELF: u32 = libc::NOTE_DELETE;
pub(crate) const INTEREST_MOVE_SELF: u32 = libc::NOTE_RENAME;

#[derive(Debug)]
pub(crate) struct EventsState<'w> {
    state: kqueue::op::State<Evented, Event, ()>,
    _unused: PhantomData<&'w ()>,
}

impl<'w> EventsState<'w> {
    pub(crate) fn new(_: &'w AsyncFd) -> EventsState<'w> {
        EventsState {
            // SAFETY: all zeros is valid for `libc::kevent`.
            state: kqueue::op::State::new(Event(unsafe { mem::zeroed() }), ()),
            _unused: PhantomData,
        }
    }
}

impl<'w> Events<'w> {
    pub(crate) fn path_for_sys<'a>(&'a self, event: &'a Event) -> Cow<'a, Path> {
        let fd = event.0.ident as RawFd;
        if let Some(path) = self.watching.get(&fd) {
            // SAFETY: the path was passed to us as a valid `PathBuf`, so it
            // must be a valid `Path`.
            let path = unsafe { OsStr::from_encoded_bytes_unchecked(path.as_bytes()) };
            return Cow::Borrowed(Path::new(path));
        }
        panic!("unknown fd in kevent")
    }

    pub(crate) fn poll_sys(
        mut self: Pin<&mut Self>,
        ctx: &mut task::Context<'_>,
    ) -> Poll<Option<io::Result<&'w notify::Event>>> {
        let Events {
            fd: kq,
            state: EventsState { state, .. },
            ..
        } = &mut *self;
        NotifyOp::poll_next(state, ctx, kq)
    }
}

pub(crate) struct NotifyOp<'a>(PhantomData<&'a ()>);

impl<'a> FdIter for NotifyOp<'a> {
    type Output = &'a notify::Event;
    type Resources = Event;
    type Args = ();
    type OperationOutput = ();

    const OP_KIND: OpKind = OpKind::Read;

    fn try_run(
        kq: &AsyncFd,
        event: &mut Self::Resources,
        (): &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        // No blocking.
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let n = syscall!(kevent(
            kq.fd(),
            ptr::null(),
            0,
            &raw mut event.0,
            1,
            &raw const timeout,
        ))?;
        if n == 0 {
            // Wait for another readiness event.
            Err(io::ErrorKind::WouldBlock.into())
        } else {
            debug_assert!(n == 1);
            debug_assert_eq!(event.0.filter, libc::EVFILT_VNODE);
            Ok(())
        }
    }

    fn is_complete((): &Self::OperationOutput) -> bool {
        false
    }

    fn map_next(_: &AsyncFd, event: &Self::Resources, (): Self::OperationOutput) -> Self::Output {
        // SAFETY: cast is safe due to repr(transparent) on notify::Event.
        unsafe { &*ptr::from_ref(event).cast::<notify::Event>() }
    }
}

pub(crate) use crate::kqueue::Event;

impl Event {
    pub(crate) fn file_path(&self) -> &Path {
        panic!(
            "a10::fs::notify::Event::file_path doesn't work with kqueue, use Events::path_for instead",
        )
    }

    pub(crate) const fn mask(&self) -> u32 {
        self.0.fflags
    }
}

#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const EVENT_ACCESSED: u32 = libc::NOTE_READ;
#[cfg(target_os = "openbsd")]
pub(crate) const EVENT_MODIFIED: u32 = libc::NOTE_WRITE | libc::NOTE_EXTEND | libc::NOTE_TRUNCATE;
#[cfg(not(target_os = "openbsd"))]
pub(crate) const EVENT_MODIFIED: u32 = libc::NOTE_WRITE | libc::NOTE_EXTEND;
pub(crate) const EVENT_METADATA_CHANGED: u32 = libc::NOTE_ATTRIB;
#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const EVENT_CLOSED_WRITE: u32 = libc::NOTE_CLOSE_WRITE;
#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const EVENT_CLOSED_NO_WRITE: u32 = libc::NOTE_CLOSE;
#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const EVENT_CLOSED: u32 = libc::NOTE_CLOSE_WRITE | libc::NOTE_CLOSE;
#[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
pub(crate) const EVENT_OPENED: u32 = libc::NOTE_OPEN;
pub(crate) const EVENT_DELETED: u32 = libc::NOTE_DELETE;
pub(crate) const EVENT_MOVED: u32 = libc::NOTE_RENAME;
pub(crate) const EVENT_UNMOUNTED: u32 = libc::NOTE_REVOKE;
