// This must come before the other modules for the documentation.
pub mod fd;

mod asan;
mod sq;
#[cfg(unix)]
mod unix;

pub mod io;
pub mod net;

#[cfg(any(target_os = "android", target_os = "linux"))]
mod io_uring;
#[cfg(any(target_os = "android", target_os = "linux"))]
use io_uring as sys;

#[doc(no_inline)]
pub use fd::AsyncFd;
#[doc(inline)]
pub use sq::SubmissionQueue;

/// Helper macro to execute a system call that returns an `io::Result`.
macro_rules! syscall {
    ($fn: ident ( $($arg: expr),* $(,)? ) ) => {{
        #[allow(unused_unsafe)]
        let res = unsafe { ::libc::$fn($( $arg, )*) };
        if res == -1 {
            ::std::result::Result::Err(::std::io::Error::last_os_error())
        } else {
            ::std::result::Result::Ok(res)
        }
    }};
}

/// Link to online manual.
#[rustfmt::skip]
macro_rules! man_link {
    ($syscall: tt ( $section: tt ) ) => {
        concat!(
            "\n\nAdditional documentation can be found in the ",
            "[`", stringify!($syscall), "(", stringify!($section), ")`]",
            "(https://man7.org/linux/man-pages/man", stringify!($section), "/", stringify!($syscall), ".", stringify!($section), ".html)",
            " manual.\n"
        )
    };
}

macro_rules! new_flag {
    (
        $(
        $(#[$type_meta:meta])*
        $type_vis: vis struct $type_name: ident ( $type_repr: ty ) $(impl BitOr $( $type_or: ty )*)? {
            $(
            $(#[$value_meta:meta])*
            $value_name: ident = $libc: ident :: $value_type: ident,
            )*
        }
        )+
    ) => {
        $(
        $(#[$type_meta])*
        #[derive(Copy, Clone, Eq, PartialEq)]
        $type_vis struct $type_name(pub(crate) $type_repr);

        impl $type_name {
            $(
            $(#[$value_meta])*
            #[allow(trivial_numeric_casts, clippy::cast_sign_loss)]
            $type_vis const $value_name: $type_name = $type_name($libc::$value_type as $type_repr);
            )*
        }

        $crate::debug_detail!(impl for $type_name($type_repr) match $( $(#[$value_meta])* $libc::$value_type ),*);

        $(
        impl std::ops::BitOr for $type_name {
            type Output = Self;

            fn bitor(self, rhs: Self) -> Self::Output {
                $type_name(self.0 | rhs.0)
            }
        }

        $(
        impl std::ops::BitOr<$type_or> for $type_name {
            type Output = Self;

            #[allow(clippy::cast_sign_loss)]
            fn bitor(self, rhs: $type_or) -> Self::Output {
                $type_name(self.0 | rhs as $type_repr)
            }
        }
        )*
        )?
        )+
    };
}

macro_rules! debug_detail {
    (
        // Match a value exactly.
        impl for $type: ident ($type_repr: ty) match
        $( $( #[$meta: meta] )* $libc: ident :: $flag: ident ),* $(,)?
    ) => {
        impl ::std::fmt::Debug for $type {
            #[allow(trivial_numeric_casts, unreachable_patterns, unreachable_code, unused_doc_comments, clippy::bad_bit_mask)]
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                mod consts {
                    $(
                    $(#[$meta])*
                    pub(super) const $flag: $type_repr = $libc :: $flag as $type_repr;
                    )*
                }

                f.write_str(match self.0 {
                    $(
                    $(#[$meta])*
                    consts::$flag => stringify!($flag),
                    )*
                    value => return value.fmt(f),
                })
            }
        }
    };
    (
        // Match a value exactly.
        match $type: ident ($event_type: ty),
        $( $( #[$meta: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
        struct $type($event_type);

        $crate::debug_detail!(impl for $type($event_type) match $( $(#[$meta])* $libc::$flag ),*);
    };
    (
        // Integer bitset.
        bitset $type: ident ($event_type: ty),
        $( $( #[$meta: meta] )* $libc: ident :: $flag: ident ),+ $(,)?
    ) => {
        struct $type($event_type);

        impl fmt::Debug for $type {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut written_one = false;
                $(
                    $(#[$meta])*
                    #[allow(clippy::bad_bit_mask)] // Apparently some flags are zero.
                    {
                        if self.0 & $libc :: $flag != 0 {
                            if !written_one {
                                write!(f, "{}", stringify!($flag))?;
                                written_one = true;
                            } else {
                                write!(f, "|{}", stringify!($flag))?;
                            }
                        }
                    }
                )+
                if !written_one {
                    write!(f, "(empty)")
                } else {
                    Ok(())
                }
            }
        }
    };
}

use {debug_detail, man_link, new_flag, syscall};
