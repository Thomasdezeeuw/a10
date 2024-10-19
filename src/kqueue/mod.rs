//! kqueue implementation.
//!
//! Manuals:
//! * <https://man.freebsd.org/cgi/man.cgi?query=kqueue>
//! * <https://man.openbsd.org/kqueue>
//! * <https://www.dragonflybsd.org/cgi/web-man/?command=kqueue>
//! * <https://man.netbsd.org/kqueue.2>

use std::os::fd::OwnedFd;
use std::sync::Mutex;
use std::{fmt, mem};

use crate::fd::{AsyncFd, Descriptor};
use crate::op::OpResult;
use crate::{debug_detail, OperationId};

pub(crate) mod config;
mod cq;
pub(crate) mod io;
mod sq;

pub(crate) use cq::Completions;
pub(crate) use sq::Submissions;

/// kqueue implementation.
pub(crate) enum Implementation {}

impl crate::Implementation for Implementation {
    type Shared = Shared;
    type Submissions = Submissions;
    type Completions = Completions;
}

#[derive(Debug)]
pub(crate) struct Shared {
    /// kqueue(2) file descriptor.
    kq: OwnedFd,
    change_list: Mutex<Vec<Event>>,
}

impl Shared {
    /// Merge the change list.
    ///
    /// Reusing allocations (if it makes sense).
    fn merge_change_list(&self, mut changes: Vec<Event>) {
        if changes.capacity() == 0 {
            return;
        }

        let mut change_list = self.change_list.lock().unwrap();
        if change_list.len() < changes.capacity() {
            // Existing is smaller than `changes` alloc, reuse it.
            mem::swap(&mut *change_list, &mut changes);
        }
        change_list.append(&mut changes);
        drop(change_list); // Unlock before any deallocations.
    }
}

/// kqueue specific [`crate::op::Op`] trait.
pub(crate) trait Op {
    type Output;
    type Resources;
    type Args;
    type OperationOutput;

    fn fill_submission<D: Descriptor>(fd: &AsyncFd<D>, kevent: &mut Event);

    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
    ) -> OpResult<Self::OperationOutput>;

    fn map_ok(resources: Self::Resources, output: Self::OperationOutput) -> Self::Output;
}

impl<T: Op> crate::op::Op for T {
    type Output = T::Output;
    type Resources = T::Resources;
    type Args = T::Args;
    type Submission = Event;
    type CompletionState = CompletionState;
    type OperationOutput = T::OperationOutput;

    fn fill_submission<D: Descriptor>(
        fd: &AsyncFd<D>,
        _: &mut Self::Resources,
        _: &mut Self::Args,
        kevent: &mut Self::Submission,
    ) {
        T::fill_submission(fd, kevent)
    }

    fn check_result<D: Descriptor>(
        fd: &AsyncFd<D>,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        _: &mut Self::CompletionState,
    ) -> OpResult<Self::OperationOutput> {
        T::check_result(fd, resources, args)
    }

    fn map_ok(resources: Self::Resources, output: Self::OperationOutput) -> Self::Output {
        T::map_ok(resources, output)
    }
}

/// Wrapper around `libc::kevent` to implementation traits and methods.
///
/// This is both a submission and a completion event.
#[repr(transparent)] // Requirement for `kevent` calls.
pub(crate) struct Event(libc::kevent);

impl Event {
    /// Returns an error from the event, if any.
    fn error(&self) -> Option<io::Error> {
        // We can't use references to packed structures (in checking the ignored
        // errors), so we need copy the data out before use.
        let data = self.0.data as i64;
        // Check for the error flag, the actual error will be in the `data`
        // field.
        //
        // Older versions of macOS (OS X 10.11 and 10.10 have been witnessed)
        // can return EPIPE when registering a pipe file descriptor where the
        // other end has already disappeared. For example code that creates a
        // pipe, closes a file descriptor, and then registers the other end will
        // see an EPIPE returned from `register`.
        //
        // It also turns out that kevent will still report events on the file
        // descriptor, telling us that it's readable/hup at least after we've
        // done this registration. As a result we just ignore `EPIPE` here
        // instead of propagating it.
        //
        // More info can be found at tokio-rs/mio#582.
        //
        // The ENOENT error informs us that a filter we're trying to remove
        // wasn't there in first place, but we don't really care since our goal
        // is accomplished.
        if (self.0.flags & libc::EV_ERROR != 0)
            && data != 0
            && data != libc::EPIPE as i64
            && data != libc::ENOENT as i64
        {
            Some(io::Error::from_raw_os_error(data as i32))
        } else {
            None
        }
    }
}

impl crate::cq::Event for Event {
    type State = CompletionState;

    fn id(&self) -> OperationId {
        self.0.udata as OperationId
    }

    fn update_state(&self, _: &mut Self::State) -> bool {
        false // Using `EV_ONESHOT`, so expecting one event.
    }
}

/// No additional state is needed.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct CompletionState;

impl crate::sq::Submission for Event {
    fn set_id(&mut self, id: OperationId) {
        self.0.udata = id as _;
    }
}

// SAFETY: `libc::kevent` is thread safe.
unsafe impl Send for Event {}
unsafe impl Sync for Event {}

impl fmt::Debug for Event {
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
            // Not stable across OS versions on OpenBSD.
            #[cfg(not(target_os = "openbsd"))]
            libc::EV_SYSFLAGS,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::EV_FLAG0,
            #[cfg(any(
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos"
            ))]
            libc::EV_POLL,
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
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            libc::NOTE_FFCTRLMASK,
            #[cfg(any(
                target_os = "dragonfly",
                target_os = "freebsd",
                target_os = "ios",
                target_os = "macos",
                target_os = "tvos",
                target_os = "visionos",
                target_os = "watchos",
            ))]
            libc::NOTE_FFLAGSMASK,
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
            libc::NOTE_PDATAMASK,
            libc::NOTE_PCTRLMASK,
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
            libc::NOTE_EXIT_DETAIL_MASK,
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
        let ident = self.0.ident;
        let data = self.0.data;
        f.debug_struct("kqueue::Event")
            .field("id", &crate::cq::Event::id(self))
            .field("ident", &ident)
            .field("filter", &FilterDetails(self.0.filter))
            .field("flags", &FlagsDetails(self.0.flags))
            .field("fflags", &FflagsDetails(self.0.fflags))
            .field("data", &data)
            .finish()
    }
}
