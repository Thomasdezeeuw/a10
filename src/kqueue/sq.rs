use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{io, mem, ptr};

use crate::sq::{Cancelled, QueueFull};
use crate::{kqueue, syscall, OperationId, WAKE_ID};

/// NOTE: all the state is in [`Shared`].
#[derive(Debug)]
pub(crate) struct Submissions {
    /// Maximum size of the change list before it's submitted to the kernel,
    /// without waiting on a call to poll.
    max_change_list_size: usize,
}

impl Submissions {
    pub(crate) fn new(max_change_list_size: usize) -> Submissions {
        Submissions {
            max_change_list_size,
        }
    }
}

impl crate::sq::Submissions for Submissions {
    type Shared = kqueue::Shared;
    type Submission = kqueue::Event;

    fn add<F>(
        &self,
        shared: &Self::Shared,
        is_polling: &AtomicBool,
        submit: F,
    ) -> Result<(), QueueFull>
    where
        F: FnOnce(&mut Self::Submission),
    {
        // Create and fill the submission event.
        // SAFETY: all zero is valid for `libc::kevent`.
        let mut event = unsafe { mem::zeroed() };
        submit(&mut event);
        event.0.flags = libc::EV_ADD | libc::EV_RECEIPT | libc::EV_ONESHOT;

        // Add the event to the list of waiting events.
        let mut change_list = shared.change_list.lock().unwrap();
        change_list.push(event);
        // If we haven't collected enough events yet and we're not polling,
        // we're done quickly.
        if change_list.len() < self.max_change_list_size && !is_polling.load(Ordering::Relaxed) {
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

    fn cancel(
        &self,
        shared: &Self::Shared,
        is_polling: &AtomicBool,
        op_id: OperationId,
    ) -> Cancelled {
        // TODO(port): implement.
        todo!("Submissions::cancel")
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
