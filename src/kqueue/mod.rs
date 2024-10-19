//! kqueue implementation.
//!
//! Manuals:
//! * <https://man.freebsd.org/cgi/man.cgi?query=kqueue>
//! * <https://man.openbsd.org/kqueue>
//! * <https://www.dragonflybsd.org/cgi/web-man/?command=kqueue>
//! * <https://man.netbsd.org/kqueue.2>

use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Mutex;
use std::time::Duration;
use std::{cmp, fmt, mem, ptr};

use crate::fd::{AsyncFd, Descriptor};
use crate::op::OpResult;
use crate::sq::QueueFull;
use crate::{debug_detail, syscall, OperationId, WAKE_ID};

pub(crate) mod config;
pub(crate) mod io;

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

#[derive(Debug)]
pub(crate) struct Completions {
    events: Vec<Event>,
}

impl Completions {
    fn new(events_capacity: usize) -> Completions {
        let events = Vec::with_capacity(events_capacity);
        Completions { events }
    }
}

impl crate::cq::Completions for Completions {
    type Shared = Shared;
    type Event = Event;

    fn poll<'a>(
        &'a mut self,
        shared: &Self::Shared,
        timeout: Option<Duration>,
    ) -> io::Result<impl Iterator<Item = &'a Self::Event>> {
        self.events.clear();

        let timeout = timeout.map(|to| libc::timespec {
            tv_sec: cmp::min(to.as_secs(), libc::time_t::MAX as u64) as libc::time_t,
            // `Duration::subsec_nanos` is guaranteed to be less than one
            // billion (the number of nanoseconds in a second), making the
            // cast to i32 safe. The cast itself is needed for platforms
            // where C's long is only 32 bits.
            tv_nsec: libc::c_long::from(to.subsec_nanos() as i32),
        });

        // Submit any submissions (changes) to the kernel.
        let mut change_list = shared.change_list.lock().unwrap();
        let mut changes = if change_list.is_empty() {
            Vec::new() // No point in taking an empty vector.
        } else {
            mem::replace(&mut *change_list, Vec::new())
        };
        drop(change_list); // Unlock, to not block others.

        let result = syscall!(kevent(
            shared.kq.as_raw_fd(),
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            changes.as_ptr().cast(),
            changes.capacity() as _,
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            self.events.as_mut_ptr().cast(),
            self.events.capacity() as _,
            timeout
                .as_ref()
                .map(|s| s as *const _)
                .unwrap_or(ptr::null_mut()),
        ));
        let mut result_err = None;
        match result {
            // SAFETY: `kevent` ensures that `n` events are written.
            Ok(n) => unsafe { self.events.set_len(n as usize) },
            Err(err) => {
                // According to the manual page of FreeBSD: "When kevent() call
                // fails with EINTR error, all changes in the changelist have been
                // applied", so we can safely ignore it. We'll have zero
                // completions though.
                if err.raw_os_error() != Some(libc::EINTR) {
                    if !changes.is_empty() {
                        // TODO: do we want to put in fake error events or
                        // something to ensure the Futures don't stall?
                        log::warn!(change_list:? = changes; "failed to submit change list: {err}, dropping changes");
                    }
                    result_err = Some(err);
                }
            }
        }

        changes.clear();
        shared.merge_change_list(changes);

        if let Some(err) = result_err {
            Err(err)
        } else {
            Ok(self.events.iter())
        }
    }
}

/// NOTE: all the state is in [`Shared`].
#[derive(Debug)]
pub(crate) struct Submissions {
    /// Maximum size of the change list before it's submitted to the kernel,
    /// without waiting on a call to poll.
    max_change_list_size: usize,
}

impl Submissions {
    fn new(max_change_list_size: usize) -> Submissions {
        Submissions {
            max_change_list_size,
        }
    }
}

impl crate::sq::Submissions for Submissions {
    type Shared = Shared;
    type Submission = Event;

    fn add<F>(&self, shared: &Self::Shared, submit: F) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Self::Submission),
    {
        // Create and fill the submission event.
        // SAFETY: all zero is valid for `libc::kevent`.
        let mut event = unsafe { mem::zeroed() };
        submit(&mut event);
        event.0.flags = libc::EV_ADD | libc::EV_RECEIPT | libc::EV_ONESHOT;
        // TODO: see if we can replace `EV_ONESHOT` with `EV_DISPATCH`, might be
        // a cheaper operation.

        // Add the event to the list of waiting events.
        let mut change_list = shared.change_list.lock().unwrap();
        change_list.push(event);
        // If we haven't collected enough events yet we're done quickly.
        if change_list.len() < self.max_change_list_size {
            drop(change_list); // Unlock first.
            return Ok(());
        }

        // Take ownership of the change list to submit it to the kernel.
        let mut changes = mem::replace(&mut *change_list, Vec::new());
        drop(change_list); // Unlock, to not block others.

        // Submit the all changes to the kernel.
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let result = syscall!(kevent(
            shared.kq.as_raw_fd(),
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            changes.as_ptr().cast(),
            changes.len() as _,
            // SAFETY: casting `Event` to `libc::kevent` is safe due to
            // `repr(transparent)` on `Event`.
            changes.as_mut_ptr().cast(),
            changes.capacity() as _,
            &timeout,
        ));
        if let Err(err) = result {
            // According to the manual page of FreeBSD: "When kevent() call
            // fails with EINTR error, all changes in the changelist have been
            // applied", so we can safely ignore it.
            if err.raw_os_error() != Some(libc::EINTR) {
                // TODO: do we want to put in fake error events or something to
                // ensure the Futures don't stall?
                log::warn!(change_list:? = changes; "failed to submit change list: {err}, dropping changes");
            }
        }
        // Check all events for possible errors and log them.
        for event in &changes {
            // NOTE: this can happen if one of the file descriptors was closed
            // before the change was submitted to the kernel. We'll log it, but
            // otherwise ignore it.
            if let Some(err) = event.error() {
                // TODO: see if we can some how get this error to the operation
                // that submitted it or something to ensure the Future doesn't
                // stall.
                log::warn!(kevent:? = event; "submitted change has an error: {err}, dropping it");
            }
        }

        // Reuse the change list allocation (if it makes sense).
        changes.clear();
        shared.merge_change_list(changes);
        Ok(())
    }

    fn wake(&self, shared: &Self::Shared) -> io::Result<()> {
        let mut kevent = libc::kevent {
            ident: 0,
            filter: libc::EVFILT_USER,
            flags: libc::EV_ADD | libc::EV_RECEIPT,
            fflags: libc::NOTE_TRIGGER,
            udata: WAKE_ID as _,
            // SAFETY: all zeros is valid for `libc::kevent`.
            ..unsafe { mem::zeroed() }
        };
        let kq = shared.kq.as_raw_fd();
        syscall!(kevent(kq, &kevent, 1, &mut kevent, 1, ptr::null()))?;
        if (kevent.flags & libc::EV_ERROR) != 0 && kevent.data != 0 {
            Err(io::Error::from_raw_os_error(kevent.data as i32))
        } else {
            Ok(())
        }
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
