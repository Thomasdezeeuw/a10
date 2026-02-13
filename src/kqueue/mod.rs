//! kqueue implementation.
//!
//! Manuals:
//! * <https://man.freebsd.org/cgi/man.cgi?query=kqueue>
//! * <https://man.openbsd.org/kqueue>
//! * <https://www.dragonflybsd.org/cgi/web-man/?command=kqueue>
//! * <https://man.netbsd.org/kqueue.2>

use std::mem::{drop as unlock, swap};
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Mutex;
use std::{fmt, ptr, task};

use crate::{PollingState, debug_detail, lock, syscall};

pub(crate) mod config;
mod cq;
pub(crate) mod fd;
pub(crate) mod fs;
pub(crate) mod io;
pub(crate) mod mem;
pub(crate) mod net;
pub(crate) mod op;
pub(crate) mod pipe;
pub(crate) mod poll;
pub(crate) mod process;
mod sq;

pub(crate) use config::Config;
pub(crate) use cq::Completions;
pub(crate) use sq::Submissions;

use cq::WAKE_USER_DATA;

#[derive(Debug)]
pub(crate) struct Shared {
    /// Maximum size of the change list before it's submitted to the kernel,
    /// without waiting on a call to poll.
    max_change_list_size: u32,
    /// Batched events to register.
    change_list: Mutex<Vec<Event>>,
    polling: PollingState,
    /// kqueue(2) file descriptor.
    kq: OwnedFd,
}

impl Shared {
    /// Reuse the allocation of a change list.
    ///
    /// Reusing allocations (if it makes sense).
    fn reuse_change_list(&self, mut changes: Vec<Event>) {
        if changes.is_empty() && changes.capacity() == 0 {
            return;
        }

        let mut change_list = lock(&self.change_list);
        if changes.capacity() >= change_list.capacity() {
            swap(&mut *change_list, &mut changes); // Reuse allocation.
        }
        change_list.append(&mut changes);
        unlock(change_list); // Unlock before any deallocations.
    }

    #[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
    // False positive, see
    // <https://github.com/rust-lang/rust-clippy/issues/4737>
    #[allow(clippy::debug_assert_with_mut_call)]
    fn kevent(
        &self,
        changes: &mut Vec<Event>,
        mut events: UseEvents<'_>,
        timeout: Option<&libc::timespec>,
    ) {
        // SAFETY: casting `Event` to `libc::kevent` is safe due to
        // `repr(transparent)` on `Event`.
        let (events_ptr, events_len) = match events {
            UseEvents::Some(ref mut events) => {
                debug_assert!(events.is_empty());
                (events.as_mut_ptr().cast(), events.capacity() as _)
            }
            UseEvents::UseChangeList => (changes.as_mut_ptr().cast(), changes.len() as _),
            UseEvents::UseChangeListWithWakeUp => {
                // When we submit the changes with a wakeup event we do NOT want
                // to process the wakeup event itself, as that would mean we
                // don't wake up the thread currently calling Ring::poll. Which
                // would defeat the entire purpose of the wake up event.
                //
                // We do this by providing 1 less space in the events list than
                // the amount of changes we submit (changes.len() - 1).
                //
                // Make sure the last event is a wakeup event.
                debug_assert!(
                    matches!(changes.last(), Some(e) if e.0.filter == libc::EVFILT_USER && e.0.udata == cq::WAKE_USER_DATA)
                );
                (changes.as_mut_ptr().cast(), (changes.len() - 1) as _)
            }
        };
        let result = syscall!(kevent(
            self.kq.as_raw_fd(),
            changes.as_ptr().cast(),
            changes.len() as _,
            events_ptr,
            events_len,
            timeout.map_or(ptr::null_mut(), ptr::from_ref),
        ));
        let events = match result {
            // SAFETY: `kevent` ensures that `n` events are written.
            Ok(n) => {
                let events = match events {
                    UseEvents::Some(events) => {
                        changes.clear();
                        events
                    }
                    UseEvents::UseChangeList | UseEvents::UseChangeListWithWakeUp => changes,
                };
                unsafe { events.set_len(n as usize) }
                events
            }
            Err(err) => {
                // According to the manual page of FreeBSD: "When kevent() call
                // fails with EINTR error, all changes in the changelist have
                // been applied", so we can safely ignore it. We'll have zero
                // completions though.
                if err.raw_os_error() != Some(libc::EINTR) && !changes.is_empty() {
                    log::warn!(changes:?; "failed to submit change list: {err}, dropping changes");
                }
                if let UseEvents::Some(events) = events {
                    events.clear();
                }
                changes.clear();
                return;
            }
        };

        for event in events.iter() {
            log::trace!(event:?; "got event");

            if event.0.flags & libc::EV_ERROR != 0 {
                // Check for the error flag, the actual error will be in the `data`
                // field.
                //
                // If the error number (event.data) is zero it means the event
                // was succesfully submitted and can be safely ignored.
                //
                // Older versions of macOS (OS X 10.11 and 10.10 have been
                // witnessed) can return EPIPE when registering a pipe file
                // descriptor where the other end has already disappeared. For
                // example code that creates a pipe, closes a file descriptor,
                // and then registers the other end will see an EPIPE returned
                // from `register`.
                //
                // It also turns out that kevent will still report events on the
                // file descriptor, telling us that it's readable/hup at least
                // after we've done this registration. As a result we just
                // ignore `EPIPE` here instead of propagating it.
                //
                // More info can be found at https://github.com/tokio-rs/mio#582.
                //
                // The ENOENT error informs us that a filter we're trying to remove
                // wasn't there in first place, but we don't really care since our goal
                // is accomplished.
                let errno = event.0.data as i32;
                if errno == 0 {
                    continue;
                } else if !matches!(errno, libc::EPIPE | libc::ENOENT) {
                    let err = io::Error::from_raw_os_error(errno);
                    log::warn!(event:?; "submitted change has an error: {err}, dropping it");
                    continue;
                }
            }

            match event.0.filter {
                libc::EVFILT_USER if event.0.udata == WAKE_USER_DATA => {}
                libc::EVFILT_USER => {
                    let ptr = event.0.udata.cast::<fd::SharedState>();
                    debug_assert!(!ptr.is_null());
                    // SAFETY: see fd::State::drop.
                    unsafe { ptr::drop_in_place(ptr) };
                }
                libc::EVFILT_READ | libc::EVFILT_WRITE => {
                    let ptr = event.0.udata.cast::<fd::SharedState>();
                    debug_assert!(!ptr.is_null());
                    // SAFETY: in kqueue::op we ensure that the pointer is
                    // always valid (the kernel should copy it over for us).
                    lock(unsafe { &*ptr }).wake(event);
                }
                libc::EVFILT_PROC => {
                    // In some cases a second EVFILT_PROC event is returned, at
                    // least on macOS, that would case a use after free if we
                    // tried to use the same user_data to wake the Waker again.
                    // So we only continue here if EV_EOF or EV_ERROR is set,
                    // which should always be the last event.
                    if event.0.flags & (libc::EV_EOF | libc::EV_ERROR) == 0 {
                        log::trace!(event:?; "skipping event");
                        continue;
                    }

                    // Wake the future that was waiting for the result.
                    // SAFETY: WaitIdOp set this pointer for us.
                    unsafe { Box::<task::Waker>::from_raw(event.0.udata.cast()).wake() };
                }
                _ => log::debug!(event:?; "unexpected event, ignoring it"),
            }
        }
        events.clear();
    }
}

enum UseEvents<'a> {
    Some(&'a mut Vec<Event>),
    UseChangeList,
    UseChangeListWithWakeUp,
}

/// Wrapper around `libc::kevent` to implementation traits and methods.
///
/// This is both a submission and a completion event.
#[repr(transparent)] // Requirement for `kevent` calls.
pub(crate) struct Event(libc::kevent);

// SAFETY: `libc::kevent` is thread safe.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl fmt::Debug for Event {
    #[allow(clippy::too_many_lines)] // The helper types and cfg attributes make this long.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        debug_detail!(
            match FilterDetails(libc::c_short),
            libc::EVFILT_READ,
            libc::EVFILT_WRITE,
            libc::EVFILT_AIO,
            libc::EVFILT_VNODE,
            libc::EVFILT_PROC,
            libc::EVFILT_SIGNAL,
            libc::EVFILT_TIMER,
            #[cfg(target_os = "freebsd")]
            libc::EVFILT_PROCDESC,
            #[cfg(any(
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            libc::EVFILT_FS,
            #[cfg(target_os = "freebsd")]
            libc::EVFILT_LIO,
            #[cfg(any(
                target_os = "freebsd",
                target_os = "dragonfly",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            libc::EVFILT_USER,
            #[cfg(target_os = "freebsd")]
            libc::EVFILT_SENDFILE,
            #[cfg(target_os = "freebsd")]
            libc::EVFILT_EMPTY,
            #[cfg(target_os = "dragonfly")]
            libc::EVFILT_EXCEPT,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::EVFILT_MACHPORT,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::EVFILT_VM,
        );

        debug_detail!(
            bitset FlagsDetails(libc::c_ushort),
            libc::EV_ADD,
            libc::EV_DELETE,
            libc::EV_ENABLE,
            libc::EV_DISABLE,
            libc::EV_ONESHOT,
            libc::EV_CLEAR,
            libc::EV_RECEIPT,
            libc::EV_DISPATCH,
            #[cfg(target_os = "freebsd")]
            libc::EV_DROP,
            libc::EV_FLAG1,
            libc::EV_ERROR,
            libc::EV_EOF,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::EV_OOBAND,
            #[cfg(target_os = "dragonfly")]
            libc::EV_NODATA,
        );

        debug_detail!(
            bitset FflagsDetails(libc::c_uint),
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            libc::NOTE_TRIGGER,
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            libc::NOTE_FFNOP,
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            libc::NOTE_FFAND,
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            libc::NOTE_FFOR,
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            libc::NOTE_FFCOPY,
            libc::NOTE_LOWAT,
            libc::NOTE_DELETE,
            libc::NOTE_WRITE,
            #[cfg(target_os = "dragonfly")]
            libc::NOTE_OOB,
            #[cfg(target_os = "openbsd")]
            libc::NOTE_EOF,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_EXTEND,
            libc::NOTE_ATTRIB,
            libc::NOTE_LINK,
            libc::NOTE_RENAME,
            libc::NOTE_REVOKE,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_NONE,
            #[cfg(any(target_os = "openbsd"))]
            libc::NOTE_TRUNCATE,
            libc::NOTE_EXIT,
            libc::NOTE_FORK,
            libc::NOTE_EXEC,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_SIGNAL,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_EXITSTATUS,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_EXIT_DETAIL,
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd",
            ))]
            libc::NOTE_TRACK,
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd",
            ))]
            libc::NOTE_TRACKERR,
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "netbsd",
                target_os = "openbsd",
            ))]
            libc::NOTE_CHILD,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_EXIT_DECRYPTFAIL,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_EXIT_MEMORY,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_EXIT_CSERROR,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_VM_PRESSURE,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_VM_PRESSURE_TERMINATE,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_VM_PRESSURE_SUDDEN_TERMINATE,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_VM_ERROR,
            #[cfg(any(
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_SECONDS,
            #[cfg(any(target_os = "freebsd"))]
            libc::NOTE_MSECONDS,
            #[cfg(any(
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_USECONDS,
            #[cfg(any(
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_NSECONDS,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_ABSOLUTE,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_LEEWAY,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_CRITICAL,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_BACKGROUND,
        );

        // Can't reference fields in packed structures.
        let udata = self.0.udata;
        let ident = self.0.ident;
        let data = self.0.data;
        f.debug_struct("kqueue::Event")
            .field("udata", &udata)
            .field("ident", &ident)
            .field("filter", &FilterDetails(self.0.filter))
            .field("flags", &FlagsDetails(self.0.flags))
            .field("fflags", &FflagsDetails(self.0.fflags))
            .field("data", &data)
            .finish()
    }
}
