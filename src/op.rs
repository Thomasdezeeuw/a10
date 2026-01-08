use std::fmt;
use std::marker::PhantomData;
use std::task::{self, Poll};

use crate::{AsyncFd, SubmissionQueue};

/// Generic [`Future`] that powers other I/O operation futures.
///
/// [`Future`]: std::future::Future
pub(crate) struct Operation<O: Op> {
    sq: SubmissionQueue,
    state: O::State,
}

impl<O: Op> Operation<O> {
    /// Create a new `Operation`.
    pub(crate) fn new(sq: SubmissionQueue, resources: O::Resources, args: O::Args) -> Operation<O> {
        Operation {
            sq,
            state: O::State::new(resources, args),
        }
    }

    /// Poll the future.
    pub(crate) fn poll(&mut self, ctx: &mut task::Context<'_>) -> Poll<O::Output> {
        O::poll(&mut self.state, ctx, &self.sq)
    }

    pub(crate) fn resources_mut(&mut self) -> Option<&mut O::Resources> {
        self.state.resources_mut()
    }
}

impl<O: Op> Operation<O>
where
    O::State: fmt::Debug,
{
    pub(crate) fn fmt_dbg(&self, name: &'static str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(name)
            .field("sq", &self.sq)
            .field("state", &self.state)
            .finish()
    }
}

/// Implementation of a [`Operation`].
pub(crate) trait Op {
    /// Output of the operation.
    type Output;
    /// See [`OpState::Resources`].
    type Resources;
    /// See [`OpState::Args`].
    type Args;
    /// State of the operation.
    type State: OpState<Resources = Self::Resources, Args = Self::Args>;

    /// See [`Future::poll`].
    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::Output>;
}

/// Generic [`Future`] that powers other I/O operation futures on a file
/// descriptor.
///
/// [`Future`]: std::future::Future
pub(crate) struct FdOperation<'fd, O: FdOp> {
    fd: &'fd AsyncFd,
    state: O::State,
}

impl<'fd, O: FdOp> FdOperation<'fd, O> {
    /// Create a new `FdOperation`.
    pub(crate) fn new(
        fd: &'fd AsyncFd,
        resources: O::Resources,
        args: O::Args,
    ) -> FdOperation<'fd, O> {
        FdOperation {
            fd,
            state: O::State::new(resources, args),
        }
    }

    /// Poll the future.
    pub(crate) fn poll(&mut self, ctx: &mut task::Context<'_>) -> Poll<O::Output> {
        O::poll(&mut self.state, ctx, self.fd)
    }
}

impl<'fd, O: FdOp> FdOperation<'fd, O>
where
    O::State: fmt::Debug,
{
    pub(crate) fn fmt_dbg(&self, name: &'static str, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(name)
            .field("fd", &self.fd)
            .field("state", &self.state)
            .finish()
    }
}

/// Implementation of a [`FdOperation`].
pub(crate) trait FdOp {
    /// Output of the operation.
    type Output;
    /// See [`OpState::Resources`].
    type Resources;
    /// See [`OpState::Args`].
    type Args;
    /// State of the operation.
    type State: OpState<Resources = Self::Resources, Args = Self::Args>;

    /// See [`Future::poll`].
    fn poll(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Self::Output>;
}

/// State of an [`Operation`] or [`FdOperation`].
pub(crate) trait OpState {
    /// Resources used in the operation, e.g. a buffer in a read call.
    type Resources;
    /// Arguments in the system call.
    type Args;

    /// Create a new operation state.
    fn new(resources: Self::Resources, args: Self::Args) -> Self;

    /// Mutable reference to the resources if the operation wasn't started yet.
    fn resources_mut(&mut self) -> Option<&mut Self::Resources>;

    /// Mutable reference to the arguments if the operation wasn't started yet.
    fn args_mut(&mut self) -> Option<&mut Self::Args>;
}

/// Create a [`Future`] based on [`Operation`].
///
/// [`Future`]: std::future::Future
macro_rules! operation {
    (
        $(
        $(#[ $meta: meta ])*
        $vis: vis struct $name: ident $( < $( $resources: ident $( : $trait: path )? )+ $(; const $const_generic: ident : $const_ty: ty )?> )? ($sys: ty) -> $output: ty $( , impl Extract -> $extract_output: ty )? ;
        )+
    ) => {
        $(
        $crate::op::new_operation!(
            $(#[ $meta ])*
            $vis struct $name $( < $( $resources $( : $trait )? )+ $(; const $const_generic : $const_ty )?> )? (Operation($sys))
              impl Future -> $output,
              $( impl Extract -> $extract_output, )?
        );
        )+
    };
}

/// Create a [`Future`] based on [`FdOperation`].
///
/// [`Future`]: std::future::Future
macro_rules! fd_operation {
    (
        $(
        $(#[ $meta: meta ])*
        $vis: vis struct $name: ident $( < $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )?> )? ($sys: ty) -> $output: ty $( , impl Extract -> $extract_output: ty )? ;
        )+
    ) => {
        $(
        $crate::op::new_operation!(
            $(#[ $meta ])*
            $vis struct $name <'fd, $( $( $resources $( : $trait )? ),+ $(; const $const_generic : $const_ty )? )? > (FdOperation($sys))
              impl Future -> $output,
              $( impl Extract -> $extract_output, )?
        );
        )+
    };
}

/// Helper macro for [`operation`] and [`fd_operation`], use those instead.
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
macro_rules! new_operation {
    (
        $(#[ $meta: meta ])*
        $vis: vis struct $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path )? > )? ($op_type: ident ( $sys: ty ) )
          $( impl Future -> $future_output: ty , )?
          $( impl AsyncIter -> $iter_output: ty , )?
          $( impl Extract -> $extract_output: ty , )?
    ) => {
        // NOTE: the weird meta ordering is required here.
        $(
        $crate::op::new_operation!(ignore $future_output);
        #[doc = "\n\n[`Future`]: std::future::Future"]
        #[must_use = "`Future`s do nothing unless polled"]
        )?
        $(
        $crate::op::new_operation!(ignore $iter_output);
        #[doc = "\n\n[`AsyncIterator`]: std::async_iter::AsyncIterator"]
        #[must_use = "`AsyncIterator`s do nothing unless polled"]
        )?
        $(#[ $meta ])*
        $vis struct $name<$( $( $lifetime, )* $( $( $resources $( : $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait = $gen_default )? )?>($crate::op::$op_type<$( $( $lifetime, )* )? $sys $( $(, $gen )? )? >);

        $crate::op::new_operation!(Future for $name $( <$( $lifetime, )* $( $( $resources $( : $trait )? ),+ $(; const $const_generic: $const_ty )? )? $(;; $gen : $gen_trait = $gen_default )? > )? -> $( $future_output )?);
        $crate::op::new_operation!(AsyncIter for $name $( <$( $lifetime, )* $( $( $resources $( : $trait )? ),+ $(; const $const_generic: $const_ty )? )? $(;; $gen : $gen_trait = $gen_default )? > )? -> $( $iter_output )?);
        $crate::op::new_operation!(Extract for $name $( <$( $lifetime, )* $( $( $resources $( : $trait )? ),+ $(; const $const_generic: $const_ty )? )? $(;; $gen : $gen_trait = $gen_default )? > )? -> $( $extract_output )?);

        impl<$( $( $lifetime, )* $( $( $resources: $( $trait + )? ::std::fmt::Debug, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait )? )?> ::std::fmt::Debug for $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                self.0.fmt_dbg(::std::concat!("a10::", ::std::stringify!($name)), f)
            }
        }
    };
    (
        Future for $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path)? > )? -> $output: ty
    ) => {
        impl<$( $( $lifetime, )* $( $( $resources $( : $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait )? )?> ::std::future::Future for $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {
            type Output = $output;

            fn poll(self: ::std::pin::Pin<&mut Self>, ctx: &mut ::std::task::Context<'_>) -> ::std::task::Poll<Self::Output> {
                // SAFETY: not moving self.
                unsafe { ::std::pin::Pin::get_unchecked_mut(self).0.poll(ctx) }
            }
        }
    };
    (
        AsyncIter for $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path)? > )? -> $output: ty
    ) => {
    };
    (
        Extract for $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path)? > )? -> $output: ty
    ) => {
    };
    (
        $trait_name: ident for $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path)? > )? ->
    ) => {
        // No `$trait_name` implementation.
    };
    (ignore $( $tt: tt )*) => {
        // Ignore.
    };
}

pub(crate) use {fd_operation, new_operation, operation};
