use std::cell::UnsafeCell;
use std::mem::{self, MaybeUninit, replace};
use std::panic::RefUnwindSafe;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::Mutex;
use std::task::{self, Poll};
use std::{fmt, io};

use crate::SubmissionQueue;
use crate::io_uring::cq::Completion;
use crate::io_uring::{self, libc};
use crate::op::OpState;

/// State of an operation.
///
/// # SAFETY
///
/// This state is shared between the future that holds runs the operation and
/// the completion event handler (via the `io_uring_cqe::user_data`, see
/// [`Completion::process`]). This is stored in the `shared` field.
///
/// The `tail` holds the resources needed for operation. These must stay alive
/// while the operation is ongoing (stored in `Shared::status`/`Status`).
///
/// TODO: doc:
///  * usage & interactions.
///  * drop function
///  * how to keep the resources alive.
#[derive(Debug)]
pub(crate) struct State<T, R, A> {
    data: NonNull<Data<T, R, A>>,
}

#[derive(Debug)]
#[repr(C)] // Needed for the drop function.
struct Data<T, R, A> {
    /// MUST be [`SingleShared`] or [`MultiShared`].
    shared: Mutex<Shared<T>>,
    tail: Tail<R, A>,
}

pub(super) type SingleShared = Mutex<Shared<Singleshot>>;
pub(super) type MultiShared = Mutex<Shared<Multishot>>;

#[derive(Debug)]
pub(super) struct Shared<T> {
    status: Status<T>,
    /// Waker to wake when the operation is done or made actionable progress.
    waker: Option<task::Waker>,
    /// Function to drop [`Data`], see `Data` docs for safety.
    drop: unsafe fn(*mut ()),
}

#[derive(Debug)]
enum Status<T> {
    /// Operation has not started yet, no submission has been made.
    NotStarted,
    /// Operation has been submitted and is running.
    Running { result: T },
    /// Operation is done.
    Done { result: T },
    /// The connected `Future`/`AsyncIterator` is dropped and thus no longer
    /// will retrieve the result.
    Dropped,
    /// Last state where the operation was fully cleaned up.
    Complete,
}

#[derive(Debug)]
struct Tail<R, A> {
    /// Resources shared with the kernel.
    ///
    /// This is only initialised if the status is not Complete.
    resources: UnsafeCell<MaybeUninit<R>>,
    /// Arguments for the operation, kept around if it needs to be retried.
    args: A,
}

impl<T, R, A> OpState for State<T, R, A> {
    type Resources = R;
    type Args = A;

    fn new(resources: R, args: A) -> State<T, R, A> {
        let data = Box::new(Data {
            shared: Mutex::new(Shared {
                status: Status::<T>::NotStarted,
                waker: None,
                drop: drop_state::<T, R, A>,
            }),
            tail: Tail {
                resources: UnsafeCell::new(MaybeUninit::new(resources)),
                args,
            },
        });
        // SAFETY: `Box::into_raw` always returns a valid pointer.
        let data = unsafe { NonNull::new_unchecked(Box::into_raw(data)) };
        State { data }
    }
}

impl<T: OpResult> Shared<T> {
    /// Update the operation based on a `completion` event.
    ///
    /// Returns true if the operation data should be dropped by the caller.
    pub(super) fn update(&mut self, completion: &Completion) -> StatusUpdate {
        match &mut self.status {
            Status::Running { result } | Status::Done { result } => {
                let completion_result = CompletionResult {
                    result: completion.0.res,
                    flags: completion.operation_flags(),
                };
                let completion_flags = completion.0.flags;
                result.update(completion_result, completion_flags);

                // IORING_CQE_F_MORE indicates that more completions are coming
                // for this operation.
                if completion_flags & libc::IORING_CQE_F_MORE == 0
                    && let Status::Running { result } | Status::Done { result } =
                        replace(&mut self.status, Status::Complete)
                {
                    self.status = Status::Done { result };

                    if let Some(waker) = self.waker.take() {
                        StatusUpdate::Wake(waker)
                    } else {
                        StatusUpdate::Ok
                    }
                } else {
                    StatusUpdate::Ok
                }
            }
            Status::Dropped => StatusUpdate::Drop {
                drop: self.drop,
                ptr: ptr::from_mut(self).cast(),
            },
            Status::NotStarted | Status::Complete => unreachable!(),
        }
    }
}

/// What to do after a [`Shared::update`].
#[derive(Debug)]
pub(super) enum StatusUpdate {
    /// Nothing to be done.
    Ok,
    /// Wake the task.
    Wake(task::Waker),
    /// Call `drop` with `ptr`.
    Drop {
        drop: unsafe fn(*mut ()),
        ptr: *mut (),
    },
}

// SAFETY: `UnsafeCell` is `!Sync`, but as long as `R` is `Sync` so is the
// `Status`.
unsafe impl<R: Send, A: Send> Send for Tail<R, A> {}
unsafe impl<R: Sync, A: Sync> Sync for Tail<R, A> {}

impl<R: RefUnwindSafe, A: RefUnwindSafe> RefUnwindSafe for Tail<R, A> {}

impl<T, R, A> Drop for State<T, R, A> {
    fn drop(&mut self) {
        {
            let mut shared = unsafe { self.data.as_ref().shared.lock().unwrap() };
            shared.waker = None;
            if !matches!(&shared.status, Status::Done { .. }) {
                // Operation is not done, mark the status as dropped and delay
                // the dropping until the operation is done. This is done in
                // [`Shared::update`].
                shared.status = Status::Dropped;
                return;
            }
        } // Drop all references to the data.

        // Operation is complete, so we can safely drop it.
        unsafe { drop_state::<T, R, A>(self.data.as_ptr().cast()) };
    }
}

unsafe fn drop_state<T, R, A>(ptr: *mut ()) {
    let ptr = ptr.cast::<Data<T, R, A>>();
    // We have to manually drop the resources as it uses MaybeUninit.
    // SAFETY: if we're called we're dropping the value, thus we should have
    // unique acess.
    let data = unsafe { &mut *ptr };
    if let Status::Complete = data
        .shared
        .get_mut()
        .unwrap_or_else(|e| e.into_inner())
        .status
    {
        // Resources already moved.
    } else {
        // SAFETY: Resources must always be initialise if the status is not
        // Complete, which we checked above.
        unsafe { data.tail.resources.get_mut().assume_init_drop() }
    }
    drop(data); // Drop any (mutable) reference before we call Box::from_raw.
    mem::drop(unsafe { Box::from_raw(ptr) });
}

/// Container for the [`CompletionResult`]. Either [`Singleshot`] or
/// [`Multishot`].
trait OpResult {
    /// Create an empty result.
    fn empty() -> Self;

    /// Update the result of the operation.
    fn update(&mut self, result: CompletionResult, completion_flags: u32);
}

/// Completed result of an operation.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) struct CompletionResult {
    /// The 16 upper bits of `io_uring_cqe.flags`, e.g. the index of a buffer in
    /// a buffer pool.
    flags: u16,
    /// The result of an operation; negative is a (negative) errno, positive a
    /// successful result. The meaning is depended on the operation itself.
    result: i32,
}

impl CompletionResult {
    /// Returns itself as operation return value.
    pub(crate) fn as_op_return(self) -> io::Result<OpReturn> {
        if let Ok(result) = u32::try_from(self.result) {
            Ok((self.flags, self.result as u32))
        } else {
            // If the result is negative then we return an error.
            Err(io::Error::from_raw_os_error(-self.result))
        }
    }
}

/// Return value of a system call.
///
/// The flags and positive result of a system call.
pub(super) type OpReturn = (u16, u32);

/// Single shot operation.
#[derive(Debug)]
pub(crate) struct Singleshot(CompletionResult);

impl OpResult for Singleshot {
    fn empty() -> Singleshot {
        Singleshot(CompletionResult {
            flags: 0,
            result: 0,
        })
    }

    fn update(&mut self, result: CompletionResult, completion_flags: u32) {
        if completion_flags & libc::IORING_CQE_F_NOTIF != 0 {
            // Zero copy completed, we can now mark ourselves as done, not
            // overwriting result.
            return;
        }
        self.0 = result;
    }
}

/// Multishot operation.
#[derive(Debug)]
pub(crate) struct Multishot(Vec<CompletionResult>);

impl OpResult for Multishot {
    fn empty() -> Multishot {
        Multishot(Vec::new())
    }

    fn update(&mut self, result: CompletionResult, _: u32) {
        self.0.push(result);
    }
}

pub(crate) trait Op {
    type Output;
    type Resources;
    type Args;

    fn fill_submission(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut io_uring::sq::Submission,
    );

    fn map_ok(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        op_return: OpReturn,
    ) -> Self::Output;
}

impl<T: Op> crate::op::Op for T {
    type Output = io::Result<T::Output>;
    type Resources = T::Resources;
    type Args = T::Args;
    type State = State<Singleshot, T::Resources, T::Args>;

    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::Output> {
        let data = unsafe { state.data.as_mut() };
        let mut shared = data.shared.lock().unwrap();
        match &mut shared.status {
            Status::NotStarted => {
                let submissions = sq.submissions();
                let result = submissions.add(|submission| {
                    // SAFETY: the resources are initialised as the status not
                    // set to Complete. Furtermore we have unique access as the
                    // status is not Running.
                    let resources = unsafe { data.tail.resources.get_mut().assume_init_mut() };
                    let args = &mut data.tail.args;
                    T::fill_submission(resources, args, submission);
                    submission.0.user_data = state.data.expose_provenance().get() as u64;
                });
                match result {
                    Ok(()) => {
                        // Make sure we get awoken when the operation is ready.
                        shared.waker = Some(ctx.waker().clone());
                        shared.status = Status::Running {
                            result: Singleshot::empty(),
                        };
                    }
                    Err(QueueFull) => {
                        // Make sure we get awoken when we can retry submitting
                        // the operation.
                        submissions.wait_for_submission(ctx.waker().clone());
                    }
                }
                Poll::Pending
            }
            Status::Running { .. } => {
                // For a single shot operation we wait until the operation is
                // done so that we can safely move/deallocate the resources.
                // This is needed for zero copy operations (e.g. sends), which
                // returns two completion events, setting the status to Running
                // and Done respectively.

                // Make sure we wake using the correct waker.
                match &mut shared.waker {
                    Some(waker) if waker.will_wake(ctx.waker()) => { /* Nothing to do. */ }
                    Some(waker) => *waker = ctx.waker().clone(),
                    None => shared.waker = Some(ctx.waker().clone()),
                }
                Poll::Pending
            }
            Status::Done { result } => {
                let result = result.0;
                shared.status = Status::Complete;
                drop(shared);

                // SAFETY: this is only safe because we set the status to
                // Complete above.
                let resources =
                    unsafe { data.tail.resources.get().cast::<Self::Resources>().read() };
                let op_return = result.as_op_return()?;
                Poll::Ready(Ok(T::map_ok(sq, resources, op_return)))
            }
            // Only the Future sets the Dropped status, which is also the only
            // one that calls this function, so this should be unreachable.
            Status::Dropped => unreachable!(),
            // Shouldn't be reachable, but if the Future is used incorrectly it
            // can be.
            Status::Complete => panic!("polled Future after completion"),
        }
    }
}
