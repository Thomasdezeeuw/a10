use std::cell::UnsafeCell;
use std::mem::{self, MaybeUninit, replace};
use std::panic::RefUnwindSafe;
use std::ptr::{self, NonNull};
use std::sync::{Mutex};
use std::task::{self, Poll};
use std::{io};

use crate::{SubmissionQueue, AsyncFd, lock};
use crate::io_uring::cq::Completion;
use crate::io_uring::sq::{Submission, QueueFull};
use crate::io_uring::{libc};
use crate::op::OpState;
use crate::asan;

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
///  * how operations are canceled on drop.
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
    /// If the status is [`Status::Running`] the kernel has (mutable) access to
    /// these resources and thus access it not allowed, hence the `UnsafeCell`.
    ///
    /// This is only initialised if the status is not [`Status::Complete`],
    /// hence `MaybeUninit`.
    resources: UnsafeCell<MaybeUninit<R>>,
    /// Arguments for the operation, kept around if it needs to be retried.
    ///
    /// These are not shared with the kernel as we check for
    /// `IORING_FEAT_SUBMIT_STABLE`.
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

    fn resources_mut(&mut self) -> Option<&mut Self::Resources> {
        // SAFETY: when the operation hasn't started we're ensure that we have
        // unique access to all the state date.
        let data = unsafe { self.data.as_ref() };
        if let Status::NotStarted = lock(&data.shared).status {
            Some(unsafe { self.data.as_mut().tail.resources.get_mut().assume_init_mut() })
        } else {
            None
        }
    }

    fn args_mut(&mut self) -> Option<&mut Self::Args> {
        // SAFETY: when the operation hasn't started we're ensure that we have
        // unique access to all the state date.
        let data = unsafe { self.data.as_ref() };
        if let Status::NotStarted = lock(&data.shared).status {
            Some(unsafe { &mut self.data.as_mut().tail.args })
        } else {
            None
        }
    }

    unsafe fn drop(&mut self, sq: &SubmissionQueue) {
        {
            let mut shared = unsafe { lock(&self.data.as_ref().shared) };
            if matches!(&shared.status, Status::Running { .. }) {
                let user_data = self.data.expose_provenance().get() as u64;
                if let Err(err) = sq.submissions().cancel(user_data) {
                    log::debug!("failed to cancel operation, will wait on result: {err}");
                }

                // Operation is still running, mark the status as dropped and
                // delay the dropping until the operation is done. This is done
                // in [`Shared::update`].
                shared.status = Status::Dropped;
                return;
            }
        } // Drop all references to the data.

        // Operation is not running, so we can safely drop it.
        unsafe { drop_state::<T, R, A>(self.data.as_ptr().cast()) };
    }
}

/// Drop `Data` pointed to be `ptr`.
///
/// # SAFETY
///
/// Caller must ensure the point is safe to drop.
unsafe fn drop_state<T, R, A>(ptr: *mut ()) {
    let ptr = ptr.cast::<Data<T, R, A>>();
    {
        // We have to manually drop the resources as it uses MaybeUninit.
        // SAFETY: if we're called we're dropping the value, thus we should have
        // unique acess.
        let data = unsafe { &mut *ptr };
        if !matches!(lock(&data.shared).status, Status::Complete) {
            asan::unpoison(data.tail.resources.get());
            // SAFETY: Resources must always be initialise if the status is not
            // Complete, which we checked above.
            unsafe { data.tail.resources.get_mut().assume_init_drop() }
        }
    } // Drop any (mutable) reference before we call Box::from_raw.

    mem::drop(unsafe { Box::<Data<T, R, A>>::from_raw(ptr) });
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

// SAFETY: UnsafeCell is !Sync, but as long as R is Sync/Send so UnsafeCell<R>.
unsafe impl<T: Send, R: Send, A: Send> Send for State<T, R, A> {}
unsafe impl<T: Sync, R: Sync, A: Sync> Sync for State<T, R, A> {}

// SAFETY: everything is heap allocate and is not moved between the initial
// allocation and deallocation.
impl<T, R, A> Unpin for State<T, R, A> {}

impl<R: RefUnwindSafe, A: RefUnwindSafe> RefUnwindSafe for Tail<R, A> {}

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
            Ok((self.flags, result))
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

    /// Fill a submission to start the operation.
    fn fill_submission(
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut Submission,
    );

    /// Map a completion result to the output of the operation.
    fn map_ok(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        op_return: OpReturn,
    ) -> Self::Output;
}

// TODO: DRY this with the Op like impls.
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
        let mut shared = lock(&data.shared);
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
                    // While the kernel has access to the resources (to use in
                    // the operation) we can't access them.
                    asan::poison(resources);
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
                asan::unpoison(data.tail.resources.get());
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

pub(crate) trait OpExtract: Op  {
    type ExtractOutput;

    /// Same as [`Op::map_ok`], returning the extract output.
    fn map_ok_extract(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        op_return: OpReturn,
    ) -> Self::ExtractOutput;
}

// TODO: DRY this with the Op like impls.
impl<T: Op + OpExtract> crate::op::OpExtract for T {
    type ExtractOutput = io::Result<T::ExtractOutput>;

    fn poll_extract(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::ExtractOutput> {
        let data = unsafe { state.data.as_mut() };
        let mut shared = lock(&data.shared);
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
                    // While the kernel has access to the resources (to use in
                    // the operation) we can't access them.
                    asan::poison(resources);
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
                asan::unpoison(data.tail.resources.get());
                let resources =
                    unsafe { data.tail.resources.get().cast::<Self::Resources>().read() };
                let op_return = result.as_op_return()?;
                Poll::Ready(Ok(T::map_ok_extract(sq, resources, op_return)))
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



pub(crate) trait FdOp {
    type Output;
    type Resources;
    type Args;

    /// See [`Op::fill_submission`].
    fn fill_submission(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut Submission,
    );

    /// See [`Op::map_ok`].
    fn map_ok(fd: &AsyncFd, resources: Self::Resources, op_return: OpReturn) -> Self::Output;
}

// TODO: DRY this with the Op like impls.
impl<T: FdOp> crate::op::FdOp for T {
    type Output = io::Result<T::Output>;
    type Resources = T::Resources;
    type Args = T::Args;
    type State = State<Singleshot, T::Resources, T::Args>;

    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Self::Output> {
        let data = unsafe { state.data.as_mut() };
        let mut shared = lock(&data.shared);
        match &mut shared.status {
            Status::NotStarted => {
                let submissions = fd.sq().submissions();
                let result = submissions.add(|submission| {
                    // SAFETY: the resources are initialised as the status not
                    // set to Complete. Furtermore we have unique access as the
                    // status is not Running.
                    let resources = unsafe { data.tail.resources.get_mut().assume_init_mut() };
                    let args = &mut data.tail.args;
                    T::fill_submission(fd, resources, args, submission);
                    // While the kernel has access to the resources (to use in
                    // the operation) we can't access them.
                    asan::poison(resources);
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
                asan::unpoison(data.tail.resources.get());
                let resources =
                    unsafe { data.tail.resources.get().cast::<Self::Resources>().read() };
                let op_return = result.as_op_return()?;
                Poll::Ready(Ok(T::map_ok(fd, resources, op_return)))
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

pub(crate) trait FdOpExtract: FdOp  {
    type ExtractOutput;

    /// Same as [`Op::map_ok`], returning the extract output.
    fn map_ok_extract(
        fd: &AsyncFd,
        resources: Self::Resources,
        op_return: OpReturn,
    ) -> Self::ExtractOutput;
}

// TODO: DRY this with the Op like impls.
impl<T: FdOp + FdOpExtract> crate::op::FdOpExtract for T {
    type ExtractOutput = io::Result<T::ExtractOutput>;

    fn poll_extract(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Self::ExtractOutput> {
        let data = unsafe { state.data.as_mut() };
        let mut shared = lock(&data.shared);
        match &mut shared.status {
            Status::NotStarted => {
                let submissions = fd.sq().submissions();
                let result = submissions.add(|submission| {
                    // SAFETY: the resources are initialised as the status not
                    // set to Complete. Furtermore we have unique access as the
                    // status is not Running.
                    let resources = unsafe { data.tail.resources.get_mut().assume_init_mut() };
                    let args = &mut data.tail.args;
                    T::fill_submission(fd, resources, args, submission);
                    // While the kernel has access to the resources (to use in
                    // the operation) we can't access them.
                    asan::poison(resources);
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
                asan::unpoison(data.tail.resources.get());
                let resources =
                    unsafe { data.tail.resources.get().cast::<Self::Resources>().read() };
                let op_return = result.as_op_return()?;
                Poll::Ready(Ok(T::map_ok_extract(fd, resources, op_return)))
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

pub(crate) trait FdIter {
    type Output;
    type Resources;
    type Args;

    /// Same as [`FdOp::fill_submission`].
    fn fill_submission(
        fd: &AsyncFd,
        resources: &mut Self::Resources,
        args: &mut Self::Args,
        submission: &mut Submission,
    );

    /// Similar to [`FdOp::map_ok`], but this processes one of the results.
    /// Meaning it only have a reference to the resources and doesn't take
    /// ownership of it.
    fn map_next(
        fd: &AsyncFd,
        resources: &Self::Resources,
        op_return: OpReturn,
    ) -> Self::Output;
}

// TODO: DRY this with the Op like impls.
impl<T: FdIter> crate::op::FdIter for T {
    type Output = io::Result<T::Output>;
    type Resources = T::Resources;
    type Args = T::Args;
    type State = State<Multishot, T::Resources, T::Args>;

    fn poll_next(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Option<Self::Output>> {
        let data = unsafe { state.data.as_mut() };
        let mut shared = lock(&data.shared);
        match &mut shared.status {
            Status::NotStarted => {
                let submissions = fd.sq().submissions();
                let result = submissions.add(|submission| {
                    // SAFETY: the resources are initialised as the status not
                    // set to Complete. Furtermore we have unique access as the
                    // status is not Running.
                    let resources = unsafe { data.tail.resources.get_mut().assume_init_mut() };
                    let args = &mut data.tail.args;
                    T::fill_submission(fd, resources, args, submission);
                    // NOTE: we do NOT poison the resources as we need read only
                    // access to them while the kernel is also reading them.
                    submission.0.user_data = state.data.expose_provenance().get() as u64;
                });
                match result {
                    Ok(()) => {
                        // Make sure we get awoken when the operation is ready.
                        shared.waker = Some(ctx.waker().clone());
                        shared.status = Status::Running {
                            result: Multishot::empty(),
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
            Status::Running { result } => {
                if result.0.is_empty() {
                    // Make sure we wake using the correct waker.
                    match &mut shared.waker {
                        Some(waker) if waker.will_wake(ctx.waker()) => { /* Nothing to do. */ }
                        Some(waker) => *waker = ctx.waker().clone(),
                        None => shared.waker = Some(ctx.waker().clone()),
                    }
                    return Poll::Pending;
                }
                let op_return = result.0.remove(0).as_op_return()?;
                drop(shared);
                // SAFETY: we share the resources with the kernel, so we can
                // only read them.
                let resources = unsafe { &*data.tail.resources.get().cast::<Self::Resources>() };
                Poll::Ready(Some(Ok(T::map_next(fd, resources, op_return))))
            }
            Status::Done { result } => {
                if result.0.is_empty() {
                    // Processed all results.
                    shared.status = Status::Complete;
                    drop(shared);
                    // SAFETY: this is only safe because we set the status to
                    // Complete above.
                    unsafe { data.tail.resources.get().cast::<Self::Resources>().drop_in_place() };
                    return Poll::Ready(None);
                }
                let op_return = result.0.remove(0).as_op_return()?;
                drop(shared);
                // SAFETY: the operation is done, so the kernel doesn't access
                // the resources any more. This gives us unique access to them.
                let resources = unsafe { &*data.tail.resources.get().cast::<Self::Resources>() };
                Poll::Ready(Some(Ok(T::map_next(fd, resources, op_return))))
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
