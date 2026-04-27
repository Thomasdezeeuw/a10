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

use crate::fs::Metadata;
use crate::fs::notify::{self, Events, Interest, Recursive, Watcher};
use crate::kqueue::fd::OpKind;
use crate::kqueue::op::{FdIter, Next};
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
        self.0.as_raw_fd().hash(state);
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
    watch_path(kq, watching, dir, interest, recursive, dir_only, None, None)
}

pub(crate) fn watch(
    kq: &AsyncFd,
    watching: &mut Watching,
    path: PathBuf,
    interest: Interest,
) -> io::Result<()> {
    let recursive = Recursive::No;
    watch_path(kq, watching, path, interest, recursive, false, None, None)
}

#[allow(clippy::too_many_arguments)]
fn watch_path(
    kq: &AsyncFd,
    watching: &mut Watching,
    path: PathBuf,
    interest: Interest,
    recursive: Recursive,
    dir_only: bool,
    is_dir: Option<bool>,
    parent: Option<RawFd>,
) -> io::Result<()> {
    // To minic inotify we need to watch all files and directories within a
    // watched directory.

    let path =
        unsafe { PathBufWithNull::from_vec_unchecked(OsString::from(path).into_encoded_bytes()) };
    let fd = WatchedFd::open(&path)?;

    let is_dir = if let Some(false) = is_dir {
        false
    } else if parent.is_some()
        && let Recursive::No = recursive
    {
        // We need to watch all files inside of a watched directory regardless
        // of the recursion requested. If recursion is not requested and the
        // parent is some we stop here.
        // SAFETY: fully zeroed `libc::stat` is valid.
        let mut metadata: Metadata = unsafe { mem::zeroed() };
        syscall!(fstat(fd.0.as_raw_fd(), &raw mut metadata.0))?;
        metadata.is_dir()
    } else {
        let path = unsafe { Path::new(OsStr::from_encoded_bytes_unchecked(path.as_bytes())) };
        let parent = fd.0.as_raw_fd();
        match std::fs::read_dir(path) {
            Ok(read_dir) => {
                for result in read_dir {
                    let entry = result?;
                    let path = entry.path();
                    let is_dir = entry.file_type()?.is_dir();
                    watch_path(
                        kq,
                        watching,
                        path,
                        interest,
                        recursive,
                        false, // dir_only is only for parent dir, so set to false.
                        Some(is_dir),
                        Some(parent),
                    )?;
                }
                true // Is a directory.
            }
            Err(ref err) if !dir_only && err.kind() == io::ErrorKind::NotADirectory => {
                // Ignore the error.
                false // Not a directory.
            }
            Err(err) => return Err(err),
        }
    };

    watch_fd(kq, watching, fd, path, interest, is_dir, parent)
}

fn watch_fd(
    kq: &AsyncFd,
    watching: &mut Watching,
    fd: WatchedFd,
    path: PathBufWithNull,
    interest: Interest,
    is_dir: bool,
    parent: Option<RawFd>,
) -> io::Result<()> {
    // User data passed to decode the events:
    // * the lower 32 bits are used to create additional event data from
    //   multiple events, see the EVENT_EXTRA_* constants.
    //   The first bit we set here, which is an indication if the fd is a file
    //   or directory.
    // * the upper 32 bits, excluding the sign bit, represent the parent fd. Or
    //   zero if the root of the watched path.
    let mut udata: usize = if is_dir {
        EVENT_EXTRA_IS_DIR as usize
    } else {
        0
    };
    #[allow(clippy::cast_sign_loss)] // fd are never negative.
    if let Some(fd) = parent {
        udata |= (fd as usize) << 32;
    }

    // Map the interest to the correct fflags.
    let mut flags = 0;
    #[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
    if interest.0 & INTEREST_ACCESS != 0 {
        flags |= libc::NOTE_READ;
    }
    if interest.0 & INTEREST_MODIFY != 0 {
        flags |= libc::NOTE_WRITE | libc::NOTE_EXTEND;
        #[cfg(target_os = "openbsd")]
        {
            flags |= libc::NOTE_TRUNCATE;
        }
    }
    if interest.0 & INTEREST_METADATA != 0 {
        flags |= libc::NOTE_ATTRIB;
    }
    #[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
    if interest.0 & INTEREST_CLOSE_WRITE != 0 {
        flags |= libc::NOTE_CLOSE_WRITE;
    }
    #[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
    if interest.0 & INTEREST_CLOSE_NOWRITE != 0 {
        flags |= libc::NOTE_CLOSE;
    }
    #[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
    if interest.0 & INTEREST_OPEN != 0 {
        flags |= libc::NOTE_OPEN;
    }
    if interest.0 & INTEREST_MOVE_INTO != 0 {
        // We can't determine if a file/directory is created or moved into a
        // watched directory, kqueue simply doesn't give us enough information.
        // Best we can do it trigger an event that a file/directory is created.
        flags |= libc::NOTE_WRITE;
    }
    if interest.0 & INTEREST_MOVE_FROM != 0 {
        // When a file/directory is moved we get two events with the following
        // flags:
        // [0] NOTE_WRITE   on watched directory.
        // [1] NOTE_RENAME  on moved file/directory.
        if is_dir {
            flags |= libc::NOTE_WRITE;
        }
        if parent.is_some() {
            flags |= libc::NOTE_RENAME;
        }
    }
    if interest.0 & INTEREST_CREATE != 0 && is_dir {
        // When a file/directory is created we only get one event:
        // [0] NOTE_WRITE   on watched directory.
        //
        // Since we aren't watching the newly created we don't know what file is
        // actually created.
        flags |= libc::NOTE_WRITE;
    }
    if interest.0 & INTEREST_DELETE != 0 && parent.is_some() {
        // On each file file inside a watched directory watch for deletion events.
        flags |= libc::NOTE_DELETE;
    }
    if parent.is_none() {
        // The *_SELF interests only apply to the fd that watch was called on,
        // so if the fd has a parent ignore it.
        if interest.0 & INTEREST_DELETE_SELF != 0 {
            flags |= libc::NOTE_DELETE;
        }
        if interest.0 & INTEREST_MOVE_SELF != 0 {
            flags |= libc::NOTE_RENAME;
        }
    }

    let change = Event(libc::kevent {
        ident: fd.0.as_raw_fd().cast_unsigned() as _,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD | libc::EV_CLEAR,
        fflags: flags,
        udata: udata as _,
        // SAFETY: all zeros is valid for `kevent`.
        ..unsafe { mem::zeroed() }
    });
    // SAFETY: created this from a PathBuf above, so it's a valid Path.
    let lpath =
        unsafe { Path::new(OsStr::from_encoded_bytes_unchecked(path.as_bytes())).display() };
    log::trace!(change:?, fd:?, path:% = lpath; "watching path");
    syscall!(kevent(
        kq.fd(),
        &raw const change.0,
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

// Custom bitset, mapped in `watch_fd`.
pub(crate) const INTEREST_ALL: u32 = INTEREST_ACCESS
    | INTEREST_MODIFY
    | INTEREST_METADATA
    | INTEREST_CLOSE
    | INTEREST_OPEN
    | INTEREST_MOVE
    | INTEREST_CREATE
    | INTEREST_DELETE
    | INTEREST_DELETE_SELF
    | INTEREST_MOVE_SELF;
pub(crate) const INTEREST_ACCESS: u32 = 1 << 0;
pub(crate) const INTEREST_MODIFY: u32 = 1 << 1;
pub(crate) const INTEREST_METADATA: u32 = 1 << 2;
pub(crate) const INTEREST_CLOSE_WRITE: u32 = 1 << 3;
pub(crate) const INTEREST_CLOSE_NOWRITE: u32 = 1 << 4;
pub(crate) const INTEREST_CLOSE: u32 = INTEREST_CLOSE_WRITE | INTEREST_CLOSE_NOWRITE;
pub(crate) const INTEREST_OPEN: u32 = 1 << 5;
pub(crate) const INTEREST_MOVE_INTO: u32 = 1 << 6;
pub(crate) const INTEREST_MOVE_FROM: u32 = 1 << 7;
pub(crate) const INTEREST_MOVE: u32 = INTEREST_MOVE_INTO | INTEREST_MOVE_FROM;
pub(crate) const INTEREST_CREATE: u32 = 1 << 8;
pub(crate) const INTEREST_DELETE: u32 = 1 << 9;
pub(crate) const INTEREST_DELETE_SELF: u32 = 1 << 10;
pub(crate) const INTEREST_MOVE_SELF: u32 = 1 << 11;

#[derive(Debug)]
pub(crate) struct EventsState<'w>(<NotifyOp<'w> as crate::op::FdIter>::State);

impl<'w> EventsState<'w> {
    pub(crate) fn new(_: &'w AsyncFd) -> EventsState<'w> {
        EventsState(kqueue::op::State::new((Vec::with_capacity(8), 0), ()))
    }
}

impl<'w> Events<'w> {
    pub(crate) fn path_for_sys<'a>(&'a self, event: &'a Event) -> Cow<'a, Path> {
        #[allow(clippy::cast_possible_wrap)]
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
            state: EventsState(state),
            ..
        } = &mut *self;
        NotifyOp::poll_next(state, ctx, kq)
    }
}

pub(crate) struct NotifyOp<'a>(PhantomData<&'a ()>);

impl<'a> FdIter for NotifyOp<'a> {
    type Output = &'a notify::Event;
    type Resources = (Vec<Event>, usize);
    type Args = ();
    type OperationOutput = ();

    const OP_KIND: OpKind = OpKind::Read;

    #[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
    fn try_run(
        kq: &AsyncFd,
        (events, processed): &mut Self::Resources,
        (): &mut Self::Args,
    ) -> io::Result<Self::OperationOutput> {
        *processed += 1;
        if events.len() > *processed {
            // Got another event ready to be processed, return it to the user.
            return Ok(());
        }

        // No blocking.
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let n = syscall!(kevent(
            kq.fd(),
            ptr::null(),
            0,
            events.as_mut_ptr().cast::<libc::kevent>(),
            events.capacity() as _,
            &raw const timeout,
        ))?;
        if n == 0 {
            // Wait for another readiness event.
            Err(io::ErrorKind::WouldBlock.into())
        } else {
            unsafe { events.set_len(n as _) };
            *processed = 0;
            log::trace!(events:?; "got fs events");
            let mut i = 0;
            while let Some((head, tail)) = events.split_at_mut_checked(i + 1) {
                let event = &head[i];

                if event.is_dir() && (event.0.fflags & libc::NOTE_WRITE != 0) {
                    // NOTE_WRITE on a directory means *something* happened
                    // within the directory, e.g. a file/directory was created
                    // or deleted. But kqueue doesn't tell us what, so we have
                    // to figure that out here.

                    if let Some(idx) = tail.iter().position(|e| {
                        e.parent_fd() == Some(event.0.ident as _)
                            && e.0.fflags & libc::NOTE_RENAME != 0
                    }) {
                        // When a file/directory is moved out of a watched
                        // directory we get two events with the following flags:
                        // [0] NOTE_WRITE   on the watched directory.
                        // [1] NOTE_RENAME  on the moved file/directory.
                        _ = events.remove(i); // Remove the event for the directory.
                        let event = &mut events[i + idx];
                        event.0.udata =
                            (event.0.udata as u64 | u64::from(EVENT_EXTRA_FILE_MOVED_FROM)) as _;
                        event.0.fflags &= !libc::NOTE_RENAME; // Don't trigger Event::moved.
                        continue; // NOTE: don't increment i as we've removed an event.
                    } else if head.iter().any(|e| {
                        e.parent_fd() == Some(event.0.ident as _)
                            && e.0.fflags & libc::NOTE_DELETE != 0
                    }) {
                        // When Interest::DELETE is used alone we get a single
                        // event:
                        // [0] NOTE_DELETE  on the deleted file/directory.
                        //
                        // We don't get an event for the directory. However,
                        // when used in combination with CREATE or MOVE_FROM it
                        // will trigger a second event:
                        // [0] NOTE_DELETE  on the deleted file/directory.
                        // [1] NOTE_WRITE   on the watched directory.
                        //
                        // In this case we remove the event for the directory,
                        // the deletion event doesn't have to be modified.
                        _ = events.remove(i); // Remove the event for the directory.
                    } else {
                        // When a file/directory is create in a watched
                        // directory we get one event with the following flags:
                        // [0] NOTE_WRITE   on the watched directory.
                        let event = &mut head[i];
                        event.0.fflags &= !libc::NOTE_WRITE; // Don't trigger Event::modified.
                        event.0.udata =
                            (event.0.udata as u64 | u64::from(EVENT_EXTRA_FILE_CREATED)) as _;
                        // TODO: get the path of the new file/directory.
                        // TODO: determine if the new file is a file or
                        // directory and set EVENT_EXTRA_IS_DIR.
                    }
                }
                i += 1;
            }
            Ok(())
        }
    }

    fn next(_: &Self::Resources, (): &Self::OperationOutput) -> Next {
        // If we have unprocessed events we need to run again. If we processed
        // all events we need to check if more events are ready to read. So, we
        // always want to run again.
        Next::TryRun
    }

    fn map_next(
        _: &AsyncFd,
        (events, processed): &Self::Resources,
        (): Self::OperationOutput,
    ) -> Self::Output {
        let event = &events[*processed];
        debug_assert_eq!(event.0.filter, libc::EVFILT_VNODE);
        // SAFETY: cast is safe due to repr(transparent) on notify::Event.
        unsafe { &*ptr::from_ref(event).cast::<notify::Event>() }
    }
}

pub(crate) use crate::kqueue::Event;

impl Event {
    #[allow(clippy::unused_self)]
    pub(crate) fn file_path(&self) -> &Path {
        panic!(
            "a10::fs::notify::Event::file_path doesn't work with kqueue, use Events::path_for instead",
        )
    }

    pub(crate) const fn mask(&self) -> u32 {
        self.0.fflags
    }

    fn parent_fd(&self) -> Option<RawFd> {
        let fd = (self.0.udata as isize) >> 32;
        if fd == 0 { None } else { Some(fd as RawFd) }
    }

    fn mask_extra(&self) -> u32 {
        ((self.0.udata as usize) & !((u32::MAX as usize) << 32)) as u32
    }

    pub(crate) fn is_dir(&self) -> bool {
        self.mask_extra() & EVENT_EXTRA_IS_DIR != 0
    }

    pub(crate) fn modified(&self) -> bool {
        if self.is_dir() {
            false // File changes are returned via the events on the file watch.
        } else {
            self.mask() & EVENT_MODIFIED != 0
        }
    }

    pub(crate) fn deleted(&self) -> bool {
        if self.parent_fd().is_some() {
            false
        } else {
            self.mask() & libc::NOTE_DELETE != 0
        }
    }

    pub(crate) fn file_moved_from(&self) -> bool {
        self.mask_extra() & EVENT_EXTRA_FILE_MOVED_FROM != 0
    }

    #[allow(clippy::unused_self)]
    pub(crate) fn file_moved_into(&self) -> bool {
        false // Not supported.
    }

    pub(crate) fn file_moved(&self) -> bool {
        self.mask_extra() & EVENT_EXTRA_FILE_MOVED != 0
    }

    pub(crate) fn file_created(&self) -> bool {
        self.mask_extra() & EVENT_EXTRA_FILE_CREATED != 0
    }

    pub(crate) fn file_deleted(&self) -> bool {
        if self.parent_fd().is_some() {
            self.mask() & libc::NOTE_DELETE != 0
        } else {
            false
        }
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
pub(crate) const EVENT_MOVED: u32 = libc::NOTE_RENAME;
pub(crate) const EVENT_UNMOUNTED: u32 = libc::NOTE_REVOKE;

const EVENT_EXTRA_IS_DIR: u32 = 1 << 0; // NOTE: set on registering the event.
const EVENT_EXTRA_FILE_MOVED_FROM: u32 = 1 << 1;
const EVENT_EXTRA_FILE_MOVED: u32 = EVENT_EXTRA_FILE_MOVED_FROM;
const EVENT_EXTRA_FILE_CREATED: u32 = 1 << 3;
