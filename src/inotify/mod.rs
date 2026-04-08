//! Filesystem notifications based on [`inotify(7)`].
//!
//! [`inotify(7)`]: https://man7.org/linux/man-pages/man7/inotify.7.html

// NOTE: currently `Watcher` always uses a regular file descriptor as
// `inotify_add_watch` (in `watch`) only works with regular file descriptors,
// not direct ones.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{CString, OsStr, OsString};
use std::mem::replace;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io, ptr, slice};

use crate::fs::notify::{self, Events, Interest, Recursive, Watcher};
use crate::io::Read;
use crate::{AsyncFd, SubmissionQueue, syscall};

/// The watch descriptors (wds) and the path to the file or directory they are
/// watching.
pub(crate) type Watching = HashMap<WatchFd, PathBufWithNull>;

/// A valid [`PathBuf`] null terminated string, encoding is OS specific.
type PathBufWithNull = CString;

/// Watch descriptor for the `Watcher` instance.
type WatchFd = std::os::fd::RawFd;

const BUF_SIZE: usize = size_of::<libc::inotify_event>() + libc::NAME_MAX as usize + 1 /* NULL. */;

impl Watcher {
    pub(crate) fn new_sys(sq: SubmissionQueue) -> io::Result<Watcher> {
        let ifd = syscall!(inotify_init1(libc::IN_CLOEXEC))?;
        // SAFETY: we've just created the `ifd` above, so it's valid.
        let fd = unsafe { AsyncFd::from_raw_fd(ifd, sq) };
        Ok(Watcher {
            fd,
            watching: HashMap::new(),
        })
    }
}

pub(crate) fn watch_recursive(
    fd: &AsyncFd,
    watching: &mut Watching,
    dir: PathBuf,
    interest: Interest,
    recursive: Recursive,
    dir_only: bool,
) -> io::Result<()> {
    if let Recursive::All = recursive {
        match std::fs::read_dir(&dir) {
            Ok(read_dir) => {
                for result in read_dir {
                    let entry = result?;
                    if entry.file_type()?.is_dir() {
                        let dir = entry.path();
                        watch_recursive(fd, watching, dir, interest, Recursive::All, true)?;
                    }
                }
            }
            Err(ref err) if !dir_only && err.kind() == io::ErrorKind::NotADirectory => {
                // Ignore the error.
            }
            Err(err) => return Err(err),
        }
    }

    let interest = Interest(interest.0 | if dir_only { libc::IN_ONLYDIR } else { 0 });
    watch(fd, watching, dir, interest)
}

pub(crate) fn watch(
    fd: &AsyncFd,
    watching: &mut Watching,
    path: PathBuf,
    interest: Interest,
) -> io::Result<()> {
    let path =
        unsafe { PathBufWithNull::from_vec_unchecked(OsString::from(path).into_encoded_bytes()) };
    let mask = interest.0
        // Don't follow symbolic links.
        | libc::IN_DONT_FOLLOW
        // When files are moved out of a watched directory don't generate
        // events for them.
        | libc::IN_EXCL_UNLINK
        // Instead of replacing a watch combine the watched events.
        | libc::IN_MASK_ADD;
    // SAFETY: created this from a PathBuf above, so it's a valid Path.
    let lpath =
        unsafe { Path::new(OsStr::from_encoded_bytes_unchecked(path.as_bytes())).display() };
    log::trace!(path:% = lpath, mask:?; "watching path");
    let wd = syscall!(inotify_add_watch(fd.fd(), path.as_ptr(), mask))?;
    // NOTE: it's possible the `wd` is already watched, we'll overwrite the
    // path, the watched interested is combined (within the kernel).
    _ = watching.insert(wd, path);
    Ok(())
}

pub(crate) const INTEREST_ALL: u32 = libc::IN_ALL_EVENTS;
pub(crate) const INTEREST_ACCESS: u32 = libc::IN_ACCESS;
pub(crate) const INTEREST_MODIFY: u32 = libc::IN_MODIFY;
pub(crate) const INTEREST_METADATA: u32 = libc::IN_ATTRIB;
pub(crate) const INTEREST_CLOSE_WRITE: u32 = libc::IN_CLOSE_WRITE;
pub(crate) const INTEREST_CLOSE_NOWRITE: u32 = libc::IN_CLOSE_NOWRITE;
pub(crate) const INTEREST_CLOSE: u32 = libc::IN_CLOSE;
pub(crate) const INTEREST_OPEN: u32 = libc::IN_OPEN;
pub(crate) const INTEREST_MOVE_FROM: u32 = libc::IN_MOVED_FROM;
pub(crate) const INTEREST_MOVE_INTO: u32 = libc::IN_MOVED_TO;
pub(crate) const INTEREST_MOVE: u32 = libc::IN_MOVE;
pub(crate) const INTEREST_CREATE: u32 = libc::IN_CREATE;
pub(crate) const INTEREST_DELETE: u32 = libc::IN_DELETE;
pub(crate) const INTEREST_DELETE_SELF: u32 = libc::IN_DELETE_SELF;
pub(crate) const INTEREST_MOVE_SELF: u32 = libc::IN_MOVE_SELF;

#[derive(Debug)]
pub(crate) enum EventsState<'w> {
    /// Currently reading.
    Reading(Read<'w, Vec<u8>>),
    /// Processing read events.
    Processing {
        buf: Vec<u8>,
        processed: usize,
        fd: &'w AsyncFd,
    },
    /// No more events.
    Done,
}

impl<'w> EventsState<'w> {
    pub(crate) fn new(fd: &'w AsyncFd) -> EventsState<'w> {
        EventsState::Reading(fd.read(Vec::with_capacity(BUF_SIZE)))
    }
}

impl<'w> Events<'w> {
    pub(crate) fn path_for_sys<'a>(&'a self, event: &'a Event) -> Cow<'a, Path> {
        let file_path = event.file_path();
        let watched_path = self.watching.get(&event.event.wd).map(move |path| {
            // SAFETY: the path was passed to us as a valid `PathBuf`, so it
            // must be a valid `Path`.
            let path = unsafe { OsStr::from_encoded_bytes_unchecked(path.as_bytes()) };
            Path::new(path)
        });
        match watched_path {
            Some(path) if file_path.as_os_str().is_empty() => Cow::Borrowed(path),
            Some(path) => Cow::Owned(path.join(file_path)),
            None => Cow::Borrowed(file_path),
        }
    }

    pub(crate) fn poll_sys(
        mut self: Pin<&mut Self>,
        ctx: &mut task::Context<'_>,
    ) -> Poll<Option<io::Result<&'w notify::Event>>> {
        let this = &mut *self;
        loop {
            match &mut this.state {
                EventsState::Processing { buf, processed, .. } => {
                    if buf.len() > *processed {
                        // SAFETY: the kernel writes zero or more `inotify_event` to
                        // `buf` so we should always be get an inotify_event we reach
                        // this code.
                        debug_assert!(buf.len() >= *processed + size_of::<libc::inotify_event>());
                        #[allow(clippy::cast_ptr_alignment)]
                        let event_ptr = unsafe {
                            buf.as_ptr()
                                .byte_add(*processed)
                                .cast::<libc::inotify_event>()
                        };

                        // Length of the events' path is dynamic.
                        let len = unsafe { (&*event_ptr).len as usize };
                        *processed += size_of::<libc::inotify_event>() + len;
                        debug_assert!(buf.len() >= *processed);

                        // `IN_IGNORED` means the file is no longer watched. An
                        // event before this should contain the information why
                        // (e.g. the file was deleted).
                        let mask = unsafe { (&*event_ptr).mask };
                        if mask & libc::IN_IGNORED != 0 {
                            let wd = unsafe { (&*event_ptr).wd };
                            _ = this.watching.remove(&wd);
                            continue; // Continue to the next event.
                        }

                        if mask & libc::IN_Q_OVERFLOW != 0 {
                            log::warn!("inotify event queue overflowed");
                            continue;
                        }

                        // The path can contain null bytes as padding for alignment,
                        // remove those.
                        let path = unsafe {
                            slice::from_raw_parts(
                                event_ptr
                                    .byte_add(size_of::<libc::inotify_event>())
                                    .cast::<u8>(),
                                len,
                            )
                        };
                        let path_len = path.iter().rposition(|b| *b != 0).map_or(len, |n| n + 1);

                        // SAFETY: this is not really safe. This should use
                        // `ptr::from_raw_parts`, but that's unstable (has been
                        // since 2021)
                        // <https://github.com/rust-lang/rust/issues/81513>.
                        #[allow(clippy::cast_ptr_alignment)]
                        let event: &'w notify::Event = unsafe {
                            &*(ptr::slice_from_raw_parts(event_ptr.cast::<u8>(), path_len)
                                as *const notify::Event)
                        };
                        return Poll::Ready(Some(Ok(event)));
                    }

                    // Processed all events in the buffer, switch to reading
                    // again.
                    let (mut buf, fd) = match replace(&mut this.state, EventsState::Done) {
                        EventsState::Processing {
                            buf,
                            processed: _,
                            fd,
                        } => (buf, fd),
                        EventsState::Reading(_) | EventsState::Done => unreachable!(),
                    };
                    buf.clear();
                    this.state = EventsState::Reading(fd.read(buf));
                    // Going to start the read in the next loop iteration.
                }
                EventsState::Reading(read) => {
                    match Pin::new(&mut *read).poll(ctx) {
                        Poll::Ready(Ok(buf)) => {
                            if buf.is_empty() {
                                this.state = EventsState::Done;
                                return Poll::Ready(None);
                            }

                            this.state = EventsState::Processing {
                                buf,
                                processed: 0,
                                fd: read.fd(),
                            };
                            // Processing the events in the next loop iteration.
                        }
                        Poll::Ready(Err(err)) => {
                            // Ensure that we don't poll the read future again.
                            this.state = EventsState::Done;
                            return Poll::Ready(Some(Err(err)));
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                EventsState::Done => return Poll::Ready(None),
            }
        }
    }
}

#[repr(C)]
pub(crate) struct Event {
    event: libc::inotify_event,
    path: [u8],
}

impl Event {
    pub(crate) fn file_path(&self) -> &Path {
        // SAFETY: the path comes from the OS, so it should be a valid OS
        // string.
        Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(&self.path) })
    }

    pub(crate) const fn mask(&self) -> u32 {
        self.event.mask
    }

    pub(crate) fn modified(&self) -> bool {
        self.mask() & EVENT_MODIFIED != 0
    }

    pub(crate) fn deleted(&self) -> bool {
        self.mask() & EVENT_DELETED != 0
    }

    pub(crate) fn file_created(&self) -> bool {
        self.mask() & EVENT_FILE_CREATED != 0
    }

    pub(crate) fn file_deleted(&self) -> bool {
        self.mask() & EVENT_FILE_DELETED != 0
    }

    pub(crate) fn fmt<'a, 'b, 'f>(
        &self,
        f: &'f mut fmt::DebugStruct<'a, 'b>,
    ) -> &'f mut fmt::DebugStruct<'a, 'b> {
        f.field("wd", &self.event.wd)
            .field("mask", &self.event.mask)
            .field("cookie", &self.event.cookie)
    }
}

pub(crate) const EVENT_IS_DIR: u32 = libc::IN_ISDIR;
pub(crate) const EVENT_ACCESSED: u32 = libc::IN_ACCESS;
pub(crate) const EVENT_MODIFIED: u32 = libc::IN_MODIFY;
pub(crate) const EVENT_METADATA_CHANGED: u32 = libc::IN_ATTRIB;
pub(crate) const EVENT_CLOSED_WRITE: u32 = libc::IN_CLOSE_WRITE;
pub(crate) const EVENT_CLOSED_NO_WRITE: u32 = libc::IN_CLOSE_NOWRITE;
pub(crate) const EVENT_CLOSED: u32 = libc::IN_CLOSE;
pub(crate) const EVENT_OPENED: u32 = libc::IN_OPEN;
pub(crate) const EVENT_DELETED: u32 = libc::IN_DELETE_SELF;
pub(crate) const EVENT_MOVED: u32 = libc::IN_MOVE_SELF;
pub(crate) const EVENT_UNMOUNTED: u32 = libc::IN_UNMOUNT;
pub(crate) const EVENT_FILE_MOVED_FROM: u32 = libc::IN_MOVED_FROM;
pub(crate) const EVENT_FILE_MOVED_INTO: u32 = libc::IN_MOVED_TO;
pub(crate) const EVENT_FILE_MOVED: u32 = libc::IN_MOVE;
pub(crate) const EVENT_FILE_CREATED: u32 = libc::IN_CREATE;
pub(crate) const EVENT_FILE_DELETED: u32 = libc::IN_DELETE;
