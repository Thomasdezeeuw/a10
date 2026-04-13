//! kqueue implementation.
//!
//! Manuals:
//! * <https://man.freebsd.org/cgi/man.cgi?query=kqueue>
//! * <https://man.openbsd.org/kqueue>
//! * <https://www.dragonflybsd.org/cgi/web-man/?command=kqueue>
//! * <https://man.netbsd.org/kqueue.2>

use std::mem::{drop as unlock, swap};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Mutex;
use std::{fmt, ptr, task};

use crate::{PollingState, debug_detail, lock, syscall};

pub(crate) mod config;
mod cq;
pub(crate) mod fd;
pub(crate) mod fs;
pub(crate) mod fs_notify;
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

fn kqueue() -> io::Result<OwnedFd> {
    // SAFETY: `kqueue(2)` ensures the fd is valid.
    let kq = unsafe { OwnedFd::from_raw_fd(syscall!(kqueue())?) };
    syscall!(fcntl(kq.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC))?;
    Ok(kq)
}

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

            if event.0.flags & libc::EV_ERROR != 0 && event.0.data == 0 {
                // If the error number (event.data) is zero it means the event
                // was succesfully submitted and can be safely ignored.
                continue;
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

impl Event {
    #[allow(clippy::too_many_lines)] // The helper types and cfg attributes make this long.
    pub(crate) fn fmt<'a, 'b, 'f>(
        &self,
        f: &'f mut fmt::DebugStruct<'a, 'b>,
    ) -> &'f mut fmt::DebugStruct<'a, 'b> {
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
            bitset IoFlagsDetails(libc::c_uint), // For EVFILT_{READ|WRITE}.
            libc::NOTE_LOWAT,
            #[cfg(target_os = "freebsd")]
            libc::NOTE_FILE_POLL,
        );

        debug_detail!(
            bitset UserFlagsDetails(libc::c_uint), // For EVFILT_USER.
            libc::NOTE_TRIGGER,
            libc::NOTE_FFNOP,
            libc::NOTE_FFAND,
            libc::NOTE_FFOR,
            libc::NOTE_FFCOPY,
            libc::NOTE_FFCTRLMASK,
            libc::NOTE_FFLAGSMASK,
        );

        debug_detail!(
            bitset ProcFlagsDetails(libc::c_uint), // For EVFILT_PROC.
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
            libc::NOTE_TRACK,
            libc::NOTE_TRACKERR,
            libc::NOTE_CHILD,
        );

        debug_detail!(
            bitset SignalFlagsDetails(libc::c_uint), // For EVFILT_SIGNAL.
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::NOTE_SIGNAL,
        );

        debug_detail!(
            bitset VnodeFlagsDetails(libc::c_uint), // For EVFILT_VNODE.
            libc::NOTE_ATTRIB,
            #[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
            libc::NOTE_CLOSE,
            #[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
            libc::NOTE_CLOSE_WRITE,
            libc::NOTE_DELETE,
            libc::NOTE_EXTEND,
            libc::NOTE_LINK,
            #[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
            libc::NOTE_OPEN,
            #[cfg(any(target_os = "freebsd", target_os = "netbsd"))]
            libc::NOTE_READ,
            libc::NOTE_RENAME,
            libc::NOTE_REVOKE,
            libc::NOTE_WRITE,
            #[cfg(target_os = "openbsd")]
            libc::NOTE_TRUNCATE,
        );

        // Can't reference fields in packed structures.
        let udata = self.0.udata;
        let ident = self.0.ident;
        let data = self.0.data;
        let fflags = self.0.fflags;
        f.field("udata", &udata)
            .field("ident", &ident)
            .field("filter", &FilterDetails(self.0.filter))
            .field("flags", &FlagsDetails(self.0.flags))
            .field(
                "fflags",
                match self.0.filter {
                    libc::EVFILT_READ | libc::EVFILT_WRITE => IoFlagsDetails::from_ref(&fflags),
                    libc::EVFILT_USER => UserFlagsDetails::from_ref(&fflags),
                    libc::EVFILT_PROC => ProcFlagsDetails::from_ref(&fflags),
                    libc::EVFILT_SIGNAL => SignalFlagsDetails::from_ref(&fflags),
                    libc::EVFILT_VNODE => VnodeFlagsDetails::from_ref(&fflags),
                    _ => &fflags,
                },
            )
            .field("data", &data)
    }
}

impl fmt::Debug for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut f = f.debug_struct("kqueue::Event");
        self.fmt(&mut f).finish()
    }
}
