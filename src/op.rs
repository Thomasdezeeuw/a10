use std::task::{self, Poll};

use crate::{AsyncFd, SubmissionQueue};

/// [`Future`] implementation of a operation with access to a
/// [`SubmissionQueue`].
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

/// Same as [`Op`], but with extract output.
pub(crate) trait OpExtract: Op {
    /// Extracted output of the operation.
    type ExtractOutput;

    /// Same as [`Op::poll`].
    fn poll_extract(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Self::ExtractOutput>;
}

/// [`AsyncIterator`] implementation of an [`Op`].
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
pub(crate) trait Iter {
    /// Output of the operation.
    type Output;
    /// See [`OpState::Resources`].
    type Resources;
    /// See [`OpState::Args`].
    type Args;
    /// State of the operation.
    type State: OpState<Resources = Self::Resources, Args = Self::Args>;

    /// See [`AsyncIterator::poll_next`].
    ///
    /// [`AsyncIterator::poll_next`]: std::async_iter::AsyncIterator::poll_next
    fn poll_next(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        sq: &SubmissionQueue,
    ) -> Poll<Option<Self::Output>>;
}

/// [`Future`] implementation of a operation with access to an [`AsyncFd`].
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

/// Same as [`FdOp`], but with extract output.
pub(crate) trait FdOpExtract: FdOp {
    /// Extracted output of the operation.
    type ExtractOutput;

    /// Same as [`FdOp::poll`].
    fn poll_extract(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Self::ExtractOutput>;
}

/// [`AsyncIterator`] implementation of a [`FdOp`].
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
pub(crate) trait FdIter {
    /// Output of the operation.
    type Output;
    /// See [`OpState::Resources`].
    type Resources;
    /// See [`OpState::Args`].
    type Args;
    /// State of the operation.
    type State: OpState<Resources = Self::Resources, Args = Self::Args>;

    /// See [`AsyncIterator::poll_next`].
    ///
    /// [`AsyncIterator::poll_next`]: std::async_iter::AsyncIterator::poll_next
    fn poll_next(
        state: &mut Self::State,
        ctx: &mut task::Context<'_>,
        fd: &AsyncFd,
    ) -> Poll<Option<Self::Output>>;
}

/// State of an operation.
pub(crate) trait OpState {
    /// Resources used in the operation, e.g. a buffer in a read call.
    type Resources;
    /// Arguments in the system call.
    type Args;

    /// Create a new operation state.
    fn new(resources: Self::Resources, args: Self::Args) -> Self;

    /// Mutable reference to the resources if the operation wasn't started yet.
    fn resources_mut(&mut self) -> Option<&mut Self::Resources>;

    /// Reference to the arguments if the operation wasn't started yet.
    fn args(&self) -> &Self::Args;

    /// Mutable reference to the arguments if the operation wasn't started yet.
    fn args_mut(&mut self) -> Option<&mut Self::Args>;

    /// Drop the operation state.
    ///
    /// # SAFETY
    ///
    /// May only be called once in the Drop implementation.
    unsafe fn drop(&mut self, sq: &SubmissionQueue);

    /// Reset the operation state.
    fn reset(&mut self, resources: Self::Resources, args: Self::Args);
}

/// Create a [`Future`] based on [`Op`].
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
            $vis struct $name $( < $( $resources $( : $trait )? )+ $(; const $const_generic : $const_ty )?> )? {
                sq: $crate::SubmissionQueue,
                sys: $sys,
            }
            required: Op,
            impl Future -> $output,
            $( impl Extract using OpExtract -> $extract_output, )?
        );
        )+
    };
}

/// Create an [`AsyncIterator`] based on multishot [`Iter`]s.
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
macro_rules! iter_operation {
    (
        $(
        $(#[ $meta: meta ])*
        $vis: vis struct $name: ident $( < $( $resources: ident $( : $trait: path )? )+ $(; const $const_generic: ident : $const_ty: ty )?> )? ($sys: ty) -> $output: ty ;
        )+
    ) => {
        $(
        $crate::op::new_operation!(
            $(#[ $meta ])*
            $vis struct $name $( < $( $resources $( : $trait )? )+ $(; const $const_generic : $const_ty )?> )? {
                sq: $crate::SubmissionQueue,
                sys: $sys,
            }
            required: Iter,
            impl AsyncIter -> $output,
        );
        )+
    };
}

/// Create a [`Future`] based on [`FdOp`].
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
            $vis struct $name <'fd, $( $( $resources $( : $trait )? ),+ $(; const $const_generic : $const_ty )? )? > {
                fd: &'fd $crate::AsyncFd,
                sys: $sys,
            }
            required: FdOp,
            impl Future -> $output,
            $( impl Extract using FdOpExtract -> $extract_output, )?
        );
        )+
    };
}

/// Create an [`AsyncIterator`] based on multishot [`FdIter`]s.
///
/// [`AsyncIterator`]: std::async_iter::AsyncIterator
macro_rules! fd_iter_operation {
    (
        $(
        $(#[ $meta: meta ])*
        $vis: vis struct $name: ident $( < $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )?> )? ($sys: ty) -> $output: ty ;
        )+
    ) => {
        $(
        $crate::op::new_operation!(
            $(#[ $meta ])*
            $vis struct $name <'fd, $( $( $resources $( : $trait )? ),+ $(; const $const_generic : $const_ty )? )? > {
                fd: &'fd $crate::AsyncFd,
                sys: $sys,
            }
            required: FdIter,
            impl AsyncIter -> $output,
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
        $vis: vis struct $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path )? > )? {
            $( $field_name: ident: $field_type: ty )*,
            sys: $sys: ty,
        }
        required: $trait_bound: ident,
        $( impl Future -> $future_output: ty , )?
        $( impl AsyncIter -> $iter_output: ty , )?
        $( impl Extract using $extract_trait_bound: ident -> $extract_output: ty , )?
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
        $vis struct $name<$( $( $lifetime, )* $( $( $resources $( : $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait = $gen_default )? )?>{
            $( $field_name: $field_type )*,
            state: <$sys $( $(, $gen )? )? as $crate::op::$trait_bound>::State,
        }

        impl<$( $( $lifetime, )* $( $( $resources: $( $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen: $gen_trait )? )?> $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {
            #[allow(private_interfaces)]
            pub(crate) fn new(
                $( $field_name: $field_type )*,
                resources: <$sys $( $(, $gen )? )? as $crate::op::$trait_bound>::Resources,
                args: <$sys $( $(, $gen )? )? as $crate::op::$trait_bound>::Args,
            ) -> $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {
                $name {
                    $( $field_name )*,
                    state: <<$sys $( $(, $gen )? )? as $crate::op::$trait_bound>::State as $crate::op::OpState>::new(resources, args),
                }
            }
        }

        $crate::op::new_operation!(
            Future for $name $( <$( $lifetime, )* $( $( $resources $( : $trait )? ),+ $(; const $const_generic: $const_ty )? )? $(;; $gen : $gen_trait = $gen_default )? > )? -> $( $future_output )?;
            call: <$sys $( $(, $gen )? )? as $crate::op::$trait_bound>::poll,
            fields: $( $field_name ),*,
        );
        $crate::op::new_operation!(
            AsyncIter for $name $( <$( $lifetime, )* $( $( $resources $( : $trait )? ),+ $(; const $const_generic: $const_ty )? )? $(;; $gen : $gen_trait = $gen_default )? > )? -> $( $iter_output )?;
            call: <$sys $( $(, $gen )? )? as $crate::op::$trait_bound>::poll_next,
            fields: $( $field_name ),*,
        );
        $crate::op::new_operation!(
            Extract for $name $( <$( $lifetime, )* $( $( $resources $( : $trait )? ),+ $(; const $const_generic: $const_ty )? )? $(;; $gen : $gen_trait = $gen_default )? > )? -> $( $extract_output )?;
            call: <$sys $( $(, $gen )? )? as $( $crate::op::$extract_trait_bound )?>::poll_extract,
            fields: $( $field_name ),*,
        );

        impl<$( $( $lifetime, )* $( $( $resources: $( $trait + )? ::std::fmt::Debug, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait )? )?> ::std::fmt::Debug for $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                let mut f = f.debug_struct(::std::concat!(::std::stringify!($name)));
                $( f.field(::std::stringify!($field_name), &self.$field_name); )*
                f.field("state", &self.state).finish()
            }
        }

        impl<$( $( $lifetime, )* $( $( $resources $( : $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait )? )?> ::std::ops::Drop for $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {
            fn drop(&mut self) {
                // SAFETY: we're in the drop implementation.
                unsafe { <<$sys $( $(, $gen )? )? as $crate::op::$trait_bound>::State as $crate::op::OpState>::drop(&mut self.state, $( &self.$field_name.sq() ),* ) }
            }
        }
    };
    (
        Future for $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path)? > )? -> $output: ty;
        call: $poll: expr,
        fields: $( $field_name: ident ),*,
    ) => {
        impl<$( $( $lifetime, )* $( $( $resources $( : $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait )? )?> ::std::future::Future for $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {
            type Output = $output;

            fn poll(self: ::std::pin::Pin<&mut Self>, ctx: &mut ::std::task::Context<'_>) -> ::std::task::Poll<Self::Output> {
                // SAFETY: not moving data out of self/this.
                let this = unsafe { ::std::pin::Pin::get_unchecked_mut(self) };
                $poll(&mut this.state, ctx, $( &this.$field_name ),*)
            }
        }
    };
    (
        AsyncIter for $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path)? > )? -> $output: ty;
        call: $poll_next: expr,
        fields: $( $field_name: ident ),*,
    ) => {
        impl<$( $( $lifetime, )* $( $( $resources $( : $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait )? )?> $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {
            /// This is the same as the [`AsyncIterator::poll_next`] function, but
            /// then available on stable Rust.
            ///
            /// [`AsyncIterator::poll_next`]: std::async_iter::AsyncIterator::poll_next
            pub fn poll_next(self: ::std::pin::Pin<&mut Self>, ctx: &mut ::std::task::Context<'_>) -> ::std::task::Poll<Option<$output>> {
                // SAFETY: not moving data out of self/this.
                let this = unsafe { ::std::pin::Pin::get_unchecked_mut(self) };
                $poll_next(&mut this.state, ctx, $( &this.$field_name ),*)
            }
        }

        #[cfg(feature = "nightly")]
        impl<$( $( $lifetime, )* $( $( $resources $( : $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait )? )?> ::std::async_iter::AsyncIterator for $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {
            type Item = $output;

            fn poll_next(self: ::std::pin::Pin<&mut Self>, ctx: &mut ::std::task::Context<'_>) -> ::std::task::Poll<Option<Self::Item>> {
                self.poll_next(ctx)
            }
        }
    };
    (
        Extract for $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path)? > )? -> $output: ty;
        call: $poll_extract: expr,
        fields: $( $field_name: ident ),*,
    ) => {
        impl<$( $( $lifetime, )* $( $( $resources $( : $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait )? )?> $crate::extract::Extract for $name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?> {}

        impl<$( $( $lifetime, )* $( $( $resources $( : $trait )?, )+ $(const $const_generic: $const_ty, )? )? $( $gen : $gen_trait )? )?> ::std::future::Future for $crate::extract::Extractor<$name<$( $( $lifetime, )* $( $( $resources, )+ $( $const_generic, )? )? $( $gen )? )?>> {
            type Output = $output;

            fn poll(self: ::std::pin::Pin<&mut Self>, ctx: &mut ::std::task::Context<'_>) -> ::std::task::Poll<Self::Output> {
                // SAFETY: not moving data out of self/this.
                let this = unsafe { &mut ::std::pin::Pin::get_unchecked_mut(self).fut };
                $poll_extract(&mut this.state, ctx, $( &this.$field_name ),*)
            }
        }
    };
    (
        // NOTE: compared to the actual implementations this doesn't have an
        // output, which indicates that the implementation shouldn't be added.
        $trait_name: ident for $name: ident $( < $( $lifetime: lifetime, )* $( $( $resources: ident $( : $trait: path )? ),+ $(; const $const_generic: ident : $const_ty: ty )? )? $(;; $gen: ident : $gen_trait: path = $gen_default: path)? > )? -> ;
        $( $remainder: tt )*
    ) => {
        // No `$trait_name` implementation.
    };
    (ignore $( $tt: tt )*) => {
        // Ignore.
    };
}

pub(crate) use {fd_iter_operation, fd_operation, iter_operation, new_operation, operation};
