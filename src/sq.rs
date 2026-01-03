//! Submission Queue.

/// Queue to submit asynchronous operations to.
///
/// This type doesn't have many public methods, but is used by all I/O types, to
/// queue asynchronous operations. The queue can be acquired by using
/// [`Ring::sq`].
///
/// The submission queue can be shared by cloning it, it's a cheap operation.
#[derive(Clone)]
pub struct SubmissionQueue {
}
