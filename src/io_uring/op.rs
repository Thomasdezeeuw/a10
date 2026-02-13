use std::cell::UnsafeCell;
use std::mem::{self, MaybeUninit, drop as unlock, replace};
use std::panic::RefUnwindSafe;
use std::ptr::NonNull;
use std::sync::Mutex;
use std::task::{self, Poll};
use std::{fmt, io};

use crate::asan;
use crate::io::BufId;
use crate::io_uring::cq::{Completion, MULTISHOT_TAG, SINGLESHOT_TAG};
use crate::io_uring::libc;
use crate::io_uring::sq::{QueueFull, Submission};
use crate::op::OpState;
use crate::{AsyncFd, OpPollResult, SubmissionQueue, debug_detail, get_mut, lock};

// # Usage
//
// A new State is created for each operation. Part of the state is shared
// between the operation and the completion event handler, this data is stored
// in Shared. For safe shared access this is wrapped in a Mutex. The following
// describes the common flow.
//
// 1. The State is created, with status NotStarted, the operation has unique
//    access to all of the state.
//
// 2. Once the operation is submitted the status is changed to Running. A
//    pointer to Mutex<Shared> is set a the user_data of a Submission (later to
//    return as the user_data of a Completion). While the operation is Running,
//    the data in Shared is shared between the kernel and the operation and is
//    thus read-only for both. The ownership of the resources, stored in
//    Tai::resources, is also changed. For singleshot operations the kernel has
//    unique/mutable access to the resources. For multishot opererations the
//    access is shared, thus both having read-only access.
//
// 3. Once the operation proceses result (via Completion events) they are stored
//    in the shared Status. Once the kernel send the final Completion for the
//    operation the status is set to Done. The relevant code to process a
//    completion is in Completion::process and Shared::update. When the Status
//    is Done th operation has unique access to all of the state (including the
//    resources) again.
//
// 4. Once the future is polled again with the Status set to Done it process the
//    remaining results (for multishot operations, for singleshot this will be
//    one result). For singleshot operations we set the status to Complete,
//    allowing us to safely read the resources (effectively removing them from
//    the state).
//
// # Dropping
//
// What happens When an operation is dropped depends on the Status. If the
// Status is
//  * NotStarted, then it's easy as the entire state can be dropped as normal,
//    nothing is shared with the kernel yet. The resources have to be manually
//    dropped, see drop_state.
//  * Running, this is tricky. The kernel has access to the state (see above),
//    so we can't deallocate the state yet. The operation sets the Status to
//    Dropped, indicating that the operation side is dropped and doesn't have
//    access any more. During the processing of completions (in Shared::update)
//    we check if the the status. If it's Dropped and no more completions are
//    coming the update function returns StatusUpdate::Drop which is used to
//    drop the entire State.
//  * Done, this is an easy one again, the kernel doesn't have access so we can
//    safely drop the State.
//  * Dropped, this shouldn't be reachable as it means the operation is already
//    Dropped.
//  * Complete, same as Done, but this time we don't need to drop the resources.

/// State of an operation.
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
    Running { results: T },
    /// Operation is done.
    Done { results: T },
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
            Some(unsafe {
                self.data
                    .as_mut()
                    .tail
                    .resources
                    .get_mut()
                    .assume_init_mut()
            })
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
                unlock(shared);
                return;
            }
            unlock(shared);
        } // Drop all references to the data.

        // Operation is not running, so we can safely drop it.
        unsafe { drop_state::<T, R, A>(self.data.as_ptr().cast()) };
    }

    fn reset(&mut self, resources: Self::Resources, args: Self::Args) {
        let data = unsafe { &mut *self.data.as_ptr() };
        let mut shared = lock(&data.shared);
        assert!(matches!(shared.status, Status::Complete));
        shared.status = Status::NotStarted;
        data.tail.resources = UnsafeCell::new(MaybeUninit::new(resources));
        data.tail.args = args;
        drop(shared);
    }
}

impl<T: fmt::Debug, R: fmt::Debug, A: fmt::Debug> fmt::Debug for State<T, R, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unsafe { self.data.as_ref() }.fmt(f)
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
        let shared = get_mut(&mut data.shared);
        if !matches!(shared.status, Status::Complete) {
            asan::unpoison(data.tail.resources.get());
            // SAFETY: Resources must always be initialise if the status is not
            // Complete, which we checked above.
            unsafe { data.tail.resources.get_mut().assume_init_drop() }
        }
    } // Drop any (mutable) reference before we call Box::from_raw.

    mem::drop(unsafe { Box::<Data<T, R, A>>::from_raw(ptr) });
}

#[allow(private_bounds)]
impl<T: OpResult> Shared<T> {
    /// Update the operation based on a `completion` event.
    ///
    /// Returns true if the operation data should be dropped by the caller.
    pub(super) fn update(&mut self, completion: &Completion) -> StatusUpdate {
        match &mut self.status {
            Status::Running { results } | Status::Done { results } => {
                let completion_result = CompletionResult {
                    result: completion.0.res,
                    flags: CompletionFlags(completion.0.flags),
                };
                let completion_flags = completion.0.flags;
                results.update(completion_result, completion_flags);

                let done = if completion.complete() {
                    if let Status::Running { results } | Status::Done { results } =
                        replace(&mut self.status, Status::Complete)
                    {
                        self.status = Status::Done { results };
                    }
                    true
                } else {
                    false
                };

                // Only wake up the Future if the operation is done or it's a
                // multishot operation (which processes results before the
                // operation is completed).
                if (done || T::IS_MULTISHOT)
                    && let Some(waker) = self.waker.take()
                {
                    StatusUpdate::Wake(waker)
                } else {
                    StatusUpdate::Ok
                }
            }
            Status::Dropped => {
                if completion.complete() {
                    // Future is dropped and the operation is complete, we can
                    // safely drop the state.
                    StatusUpdate::Drop { drop: self.drop }
                } else {
                    // More completions are coming, so we can't deallocate yet.
                    StatusUpdate::Ok
                }
            }
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
    /// Call `drop` with pointer to `SingleShared` or `MultiShared`.
    Drop { drop: unsafe fn(*mut ()) },
}

// SAFETY: If Data is Send/Sync we can safely mark State as Send/Sync as well.
unsafe impl<T, R, A> Send for State<T, R, A> where Data<T, R, A>: Send {}
unsafe impl<T, R, A> Sync for State<T, R, A> where Data<T, R, A>: Sync {}

// SAFETY: UnsafeCell is !Sync, but as long as R is Sync/Send so UnsafeCell<R>.
//unsafe impl<T: Send, R: Send, A: Send> Send for State<T, R, A> {}
unsafe impl<R: Sync, A: Sync> Sync for Tail<R, A> {}

// SAFETY: Same as the Sync implementation, see above.
impl<R: RefUnwindSafe, A: RefUnwindSafe> RefUnwindSafe for Tail<R, A> {}

/// Container for the [`CompletionResult`]. Either [`Singleshot`] or
/// [`Multishot`].
trait OpResult {
    /// Create an empty result.
    fn empty() -> Self;

    /// Update the result of the operation.
    fn update(&mut self, result: CompletionResult, completion_flags: u32);

    /// Whether or not the operation if a multishot operation. Is used to
    /// determine if we need to wake if more completion events are expected.
    const IS_MULTISHOT: bool;

    /// Return the next result.
    fn next(&mut self) -> Option<CompletionResult>;

    /// Returns true if there is another result.
    /// NOTE: only works for multishot.
    fn has_next(&self) -> bool {
        false
    }
}

/// Completed result of an operation.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) struct CompletionResult {
    flags: CompletionFlags,
    /// The result of an operation; negative is a (negative) errno, positive a
    /// successful result. The meaning is depended on the operation itself.
    result: i32,
}

impl CompletionResult {
    /// Returns itself as checked result.
    pub(crate) fn check_result(self) -> io::Result<u32> {
        if let Ok(result) = u32::try_from(self.result) {
            Ok(result)
        } else {
            // If the result is negative then we return an error.
            Err(io::Error::from_raw_os_error(-self.result))
        }
    }
}

/// The `io_uring_cqe.flags`.
#[derive(Copy, Clone, Eq, PartialEq)]
pub(crate) struct CompletionFlags(u32);

impl CompletionFlags {
    pub(super) const fn empty() -> CompletionFlags {
        CompletionFlags(0)
    }

    /* Currently not used.
    /// Returns the flags.
    pub(super) const fn operation_flags(self) -> u16 {
        // Lower 16 bits contain the flags.
        self.0 as u16
    }
    */

    /// If `IORING_CQE_F_BUFFER` is set this will return the buffer id.
    pub(super) const fn buf_id(self) -> Option<BufId> {
        if self.0 & libc::IORING_CQE_F_BUFFER != 0 {
            Some(BufId((self.0 >> libc::IORING_CQE_BUFFER_SHIFT) as u16))
        } else {
            None
        }
    }
}

debug_detail!(
    impl bitset for CompletionFlags(u32),
    libc::IORING_CQE_F_BUFFER,
    libc::IORING_CQE_F_MORE,
    libc::IORING_CQE_F_SOCK_NONEMPTY,
    libc::IORING_CQE_F_NOTIF,
    libc::IORING_CQE_F_BUF_MORE,
);

/// Return value of a system call.
///
/// The flags and positive result of a system call.
pub(super) type OpReturn = (CompletionFlags, u32);

/// Single shot operation.
#[derive(Debug)]
pub(crate) struct Singleshot(CompletionResult);

impl OpResult for Singleshot {
    fn empty() -> Singleshot {
        Singleshot(CompletionResult {
            flags: CompletionFlags::empty(),
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

    const IS_MULTISHOT: bool = false;

    fn next(&mut self) -> Option<CompletionResult> {
        Some(self.0)
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

    const IS_MULTISHOT: bool = true;

    fn next(&mut self) -> Option<CompletionResult> {
        if self.0.is_empty() {
            return None;
        }
        Some(self.0.remove(0))
    }

    /// Returns true if there is another result.
    /// NOTE: only works for multishot.
    fn has_next(&self) -> bool {
        !self.0.is_empty()
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

    fn fallback(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        err: io::Error,
    ) -> io::Result<Self::Output> {
        _ = sq;
        _ = resources;
        _ = err;
        Err(fallback(err))
    }
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
        poll(
            sq,
            state,
            ctx,
            |_, resources, args, submission| T::fill_submission(resources, args, submission),
            T::map_ok,
            T::fallback,
        )
    }
}

pub(crate) trait OpExtract: Op {
    type ExtractOutput;

    /// Same as [`Op::map_ok`], returning the extract output.
    fn map_ok_extract(
        sq: &SubmissionQueue,
        resources: Self::Resources,
        op_return: OpReturn,
    ) -> Self::ExtractOutput;
}

impl<T: Op + OpExtract> crate::op::OpExtract for T {
    type ExtractOutput = io::Result<T::ExtractOutput>;

    fn poll_extract(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::ExtractOutput> {
        poll(
            sq,
            state,
            ctx,
            |_, resources, args, submission| T::fill_submission(resources, args, submission),
            T::map_ok_extract,
            |_, _, err| Err(fallback(err)),
        )
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
        poll(
            fd,
            state,
            ctx,
            T::fill_submission,
            T::map_ok,
            |_, _, err| Err(fallback(err)),
        )
    }
}

pub(crate) trait FdOpExtract: FdOp {
    type ExtractOutput;

    /// Same as [`Op::map_ok`], returning the extract output.
    fn map_ok_extract(
        fd: &AsyncFd,
        resources: Self::Resources,
        op_return: OpReturn,
    ) -> Self::ExtractOutput;
}

impl<T: FdOp + FdOpExtract> crate::op::FdOpExtract for T {
    type ExtractOutput = io::Result<T::ExtractOutput>;

    fn poll_extract(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Self::ExtractOutput> {
        poll(
            fd,
            state,
            ctx,
            T::fill_submission,
            T::map_ok_extract,
            |_, _, err| Err(fallback(err)),
        )
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
    fn map_next(fd: &AsyncFd, resources: &Self::Resources, op_return: OpReturn) -> Self::Output;
}

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
        poll_next(
            fd,
            state,
            ctx,
            T::fill_submission,
            T::map_next,
            |_, _, err| Err(fallback(err)),
        )
    }
}

fn poll<T, O, R, A, Out>(
    target: &T,
    state: &mut State<O, R, A>,
    ctx: &mut task::Context<'_>,
    fill_submission: impl Fn(&T, &mut R, &mut A, &mut Submission),
    map_ok: impl Fn(&T, R, OpReturn) -> Out,
    fallback: impl Fn(&T, R, io::Error) -> io::Result<Out>,
) -> Poll<io::Result<Out>>
where
    T: OpTarget,
    O: OpResult,
{
    // SAFETY: this is only safe because we set the status to Complete before we
    // read the resources here.
    let read_resources = |resources_ptr: *mut R| unsafe { resources_ptr.read() };
    poll_inner(
        target,
        state,
        ctx,
        fill_submission,
        read_resources,
        map_ok,
        fallback,
    )
}

#[allow(private_bounds)]
pub(super) fn poll_next<T, O, R, A, Out>(
    target: &T,
    state: &mut State<O, R, A>,
    ctx: &mut task::Context<'_>,
    fill_submission: impl Fn(&T, &mut R, &mut A, &mut Submission),
    map_next: impl Fn(&T, &R, OpReturn) -> Out,
    fallback: impl Fn(&T, &R, io::Error) -> io::Result<Out>,
) -> Poll<Option<io::Result<Out>>>
where
    T: OpTarget,
    O: OpResult,
{
    // SAFETY: this is only safe because we set the status to Complete before we
    // read the resources here.
    let get_resources = |resources_ptr: *mut R| unsafe { &*resources_ptr };
    poll_inner(
        target,
        state,
        ctx,
        fill_submission,
        get_resources,
        map_next,
        fallback,
    )
}

/// A (too large) function that polls a `State` to implement any kind of
/// operation.
///
/// Arguments:
///  * `target` either AsyncFd or SubmissionQueue, need access for the filling
///    of the submissions and submitting it.
///  * `state` state of the operation.
///  * `ctx` needed to get the task::Waker if we can't make progress.
///  * `fill_submission` fill a io_uring submission.
///  * `get_resources` get access to the resources, for Future this will be
///    reading them, for AsyncIter this is a read-only reference.
///  * `map_ok` map a successfull result.
///  * `fallback` function called when the operation errored, to attempt a
///    fallback operation (e.g. a synchronous function call).
#[allow(clippy::needless_pass_by_ref_mut)] // For ctx.
#[allow(clippy::too_many_lines)] // This is true.
fn poll_inner<T, O, R, R2, A, Ok, Res>(
    target: &T,
    state: &mut State<O, R, A>,
    ctx: &mut task::Context<'_>,
    fill_submission: impl Fn(&T, &mut R, &mut A, &mut Submission),
    get_resources: impl Fn(*mut R) -> R2,
    map_ok: impl Fn(&T, R2, OpReturn) -> Ok,
    fallback: impl Fn(&T, R2, io::Error) -> io::Result<Ok>,
) -> Poll<Res>
where
    T: OpTarget,
    O: OpResult,
    Res: OpPollResult<Ok>,
{
    let data = unsafe { state.data.as_mut() };
    let mut shared = lock(&data.shared);
    loop {
        match &mut shared.status {
            Status::NotStarted => {
                let submissions = target.sq().submissions();
                let result = submissions.add(|submission| {
                    // SAFETY: the resources are initialised as the status not set
                    // to Complete. Furtermore we have unique access as the status
                    // is not Running.
                    let resources = unsafe { data.tail.resources.get_mut().assume_init_mut() };
                    let args = &mut data.tail.args;
                    fill_submission(target, resources, args, submission);
                    target.set_flags(submission);

                    submission.0.user_data = state.data.expose_provenance().get() as u64;
                    if O::IS_MULTISHOT {
                        // For multishot operations we do NOT poison the resources
                        // as we need read only access to them while the kernel is
                        // also reading them.
                        submission.0.user_data |= MULTISHOT_TAG as u64;
                    } else {
                        // In singleshot operation we can't access the resources
                        // while the kernel has access to them. E.g. the kernel
                        // might be writing into a buffer.
                        asan::poison(resources);
                        submission.0.user_data |= SINGLESHOT_TAG as u64;
                    }
                });
                match result {
                    Ok(()) => {
                        // Make sure we get awoken when the operation is ready.
                        shared.waker = Some(ctx.waker().clone());
                        shared.status = Status::Running {
                            results: O::empty(),
                        };
                        unlock(shared);
                    }
                    Err(QueueFull) => {
                        unlock(shared);
                        // Make sure we get awoken when we can retry submitting the
                        // operation.
                        submissions.wait_for_submission(ctx.waker().clone());
                    }
                }
                return Poll::Pending;
            }
            Status::Running { results } if O::IS_MULTISHOT => {
                // For multishot operations we can process completions results
                // as they are posted by the kernel.
                let Some(result) = results.next() else {
                    // No completion yet, try again later.
                    // Make sure we wake using the correct waker.
                    set_waker(&mut shared.waker, ctx.waker());
                    unlock(shared);
                    return Poll::Pending;
                };
                unlock(shared);
                let res = match result.check_result() {
                    Ok(res) => res,
                    Err(err) => return Poll::Ready(Res::from_err(err)),
                };
                // SAFETY: we share the resources with the kernel, so we can
                // only read them.
                let resources = get_resources(data.tail.resources.get().cast::<R>());
                let op_return = (result.flags, res);
                return Poll::Ready(Res::from_ok(map_ok(target, resources, op_return)));
            }
            Status::Running { .. } => {
                // For a singleshot operation we wait until the operation is
                // done so that we can safely move/deallocate the resources.
                // This is needed for zero copy operations (e.g. sends), which
                // returns two completion events, setting the status to Running
                // and Done respectively.

                // Make sure we wake using the correct waker.
                set_waker(&mut shared.waker, ctx.waker());
                unlock(shared);
                return Poll::Pending;
            }
            Status::Done { results } => {
                let Some(result) = results.next() else {
                    // NOTE: this is unreachable for singleshot operations.
                    assert!(O::IS_MULTISHOT);

                    // Processed all results.
                    shared.status = Status::Complete;
                    unlock(shared);
                    // SAFETY: this is only safe because we set the status to
                    // Complete above.
                    unsafe { data.tail.resources.get().cast::<R>().drop_in_place() }
                    return Poll::Ready(Res::done());
                };

                if !O::IS_MULTISHOT {
                    // For singlshot operations we set the status to Complete so
                    // that we can safely read the resources below and pass them to
                    // map_ok.
                    shared.status = Status::Complete;
                    // SAFETY: now that the kernel is Done with the operation and
                    // we've marked it as Complete we can safely access the
                    // resources again.
                    asan::unpoison(data.tail.resources.get());
                }

                match result.check_result() {
                    Ok(res) => {
                        unlock(shared);
                        let resources = get_resources(data.tail.resources.get().cast::<R>());
                        let op_return = (result.flags, res);
                        return Poll::Ready(Res::from_ok(map_ok(target, resources, op_return)));
                    }
                    Err(ref err)
                        if matches!(err.raw_os_error(), Some(libc::EINTR | libc::ECANCELED)) =>
                    {
                        // If the operation was interrupted or canceled we
                        // restart it so callers don't have to deal with the
                        // errors.

                        // Sanity checks.
                        if O::IS_MULTISHOT {
                            assert!(
                                matches!(&shared.status, Status::Done { results } if !results.has_next())
                            );
                        } else {
                            assert!(matches!(shared.status, Status::Complete));
                        }
                        shared.status = Status::NotStarted;
                        // Try again in the next iteration.
                        // NOTE: still holding the lock.
                    }
                    Err(err) => {
                        unlock(shared);
                        let resources = get_resources(data.tail.resources.get().cast::<R>());
                        return Poll::Ready(Res::from_res(fallback(target, resources, err)));
                    }
                }
            }
            // Only the Future sets the Dropped status, which is also the only one
            // that calls this function, so this should be unreachable.
            Status::Dropped => {
                unlock(shared);
                unreachable!()
            }
            // Shouldn't be reachable, but if the Future is used incorrectly it can
            // be.
            Status::Complete => {
                unlock(shared);
                panic!("polled Future after completion")
            }
        }
    }
}

fn set_waker(waker: &mut Option<task::Waker>, w: &task::Waker) {
    match waker {
        Some(waker) if waker.will_wake(w) => { /* Nothing to do. */ }
        Some(waker) => waker.clone_from(w),
        None => *waker = Some(w.clone()),
    }
}

/// Either an [`AsyncFd`] or [`SubmissionQueue`].
trait OpTarget {
    fn sq(&self) -> &SubmissionQueue;

    fn set_flags(&self, submission: &mut Submission);
}

impl OpTarget for AsyncFd {
    fn sq(&self) -> &SubmissionQueue {
        self.sq()
    }

    fn set_flags(&self, submission: &mut Submission) {
        self.kind().use_flags(submission);
    }
}

impl OpTarget for SubmissionQueue {
    fn sq(&self) -> &SubmissionQueue {
        self
    }

    fn set_flags(&self, _: &mut Submission) {
        // No flags to set.
    }
}

pub(super) fn fallback(err: io::Error) -> io::Error {
    match err.raw_os_error() {
        Some(libc::EINVAL) => io::Error::new(
            io::ErrorKind::Unsupported,
            "operation not supported, please update your Linux kernel version",
        ),
        _ => err,
    }
}
