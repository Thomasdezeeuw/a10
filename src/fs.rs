//! Asynchronous filesystem manipulation operations.

use std::ffi::CString;
use std::future::Future;
use std::io;
use std::mem::{forget as leak, take};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::io::RawFd;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{self, Poll};

use crate::op::{SharedOperationState, NO_OFFSET};
use crate::{libc, QueueFull, SubmissionQueue};

/// Options used to configure how a file is opened.
#[derive(Clone, Debug)]
pub struct OpenOptions {
    flags: libc::c_int,
    mode: libc::mode_t,
}

impl OpenOptions {
    /// Empty `OpenOptions`.
    const fn new() -> OpenOptions {
        OpenOptions {
            flags: libc::O_CLOEXEC,
            mode: 0o666,
        }
    }

    /// Enable read access.
    #[doc(alias = "O_RDONLY")]
    #[doc(alias = "O_RDWR")]
    pub fn read(mut self) -> Self {
        if self.flags & libc::O_WRONLY != 0 {
            self.flags &= !libc::O_WRONLY;
            self.flags |= libc::O_RDWR;
        } else {
            self.flags |= libc::O_RDONLY;
        }
        self
    }

    /// Enable write access.
    #[doc(alias = "O_WRONLY")]
    #[doc(alias = "O_RDWR")]
    pub fn write(mut self) -> Self {
        if self.flags & libc::O_RDONLY != 0 {
            self.flags &= !libc::O_RDONLY;
            self.flags |= libc::O_RDWR;
        } else {
            self.flags |= libc::O_WRONLY;
        }
        self
    }

    /// Set writing to append only mode.
    ///
    /// # Notes
    ///
    /// This requires [writing access] to be enabled.
    ///
    /// [writing access]: OpenOptions::write
    #[doc(alias = "O_APPEND")]
    pub fn append(mut self) -> Self {
        self.flags |= libc::O_APPEND;
        self
    }

    /// Truncate the file if it exists.
    #[doc(alias = "O_TRUNC")]
    pub fn truncate(mut self) -> Self {
        self.flags |= libc::O_TRUNC;
        self
    }

    /// If the file doesn't exist create it.
    pub fn create(mut self) -> Self {
        self.flags |= libc::O_CREAT;
        self
    }

    /// Force a file to be created, failing if a file already exists.
    ///
    /// This options implies [`OpenOptions::create`].
    #[doc(alias = "O_EXCL")]
    #[doc(alias = "O_CREAT")]
    pub fn create_new(mut self) -> Self {
        self.flags |= libc::O_CREAT | libc::O_EXCL;
        self
    }

    /// Write operations on the file will complete according to the requirements
    /// of synchronized I/O *data* integrity completion.
    ///
    /// By the time `write(2)` (and similar) return, the output data has been
    /// transferred to the underlying hardware, along with any file metadata
    /// that would be required to retrieve that data (i.e., as though each
    /// `write(2)` was followed by a call to `fdatasync(2)`).
    #[doc(alias = "O_DSYNC")]
    pub fn data_sync(mut self) -> Self {
        self.flags |= libc::O_DSYNC;
        self
    }

    /// Write operations on the file will complete according to the requirements
    /// of synchronized I/O *file* integrity completion (by contrast with the
    /// synchronized I/O data integrity completion provided by
    /// [`OpenOptions::data_sync`].)
    ///
    /// By the time `write(2)` (or similar) returns, the output data and
    /// associated file metadata have been transferred to the underlying
    /// hardware (i.e., as though each `write(2)` was followed by a call to
    /// `fsync(2)`).
    #[doc(alias = "O_SYNC")]
    pub fn sync(mut self) -> Self {
        self.flags |= libc::O_SYNC;
        self
    }

    /// Sets the mode bits that a new file will be created with.
    pub fn mode(mut self, mode: libc::mode_t) -> Self {
        self.mode = mode;
        self
    }

    /// Create an unnamed temporary regular file. The `dir` argument specifies a
    /// directory; an unnamed inode will be created in that directory's
    /// filesystem. Anything written to the resulting file will be lost when the
    /// last file descriptor is closed, unless the file is given a name.
    ///
    /// [`OpenOptions::write`] must be set. The `linkat(2)` system call can be
    /// used to make the temporary file permanent.
    #[doc(alias = "O_TMPFILE")]
    pub fn open_temp_file(
        mut self,
        queue: SubmissionQueue,
        dir: PathBuf,
    ) -> Result<Open, QueueFull> {
        self.flags |= libc::O_TMPFILE;
        self.open(queue, dir)
    }

    /// Open `path`.
    #[doc(alias = "openat")]
    pub fn open(self, queue: SubmissionQueue, path: PathBuf) -> Result<Open, QueueFull> {
        let path = path.into_os_string().into_vec();
        let path = unsafe { CString::from_vec_unchecked(path) };

        let state = SharedOperationState::new(queue);
        state.start(|submission| unsafe {
            submission.open_at(libc::AT_FDCWD, path.as_ptr(), self.flags, self.mode)
        })?;

        Ok(Open {
            path,
            state: Some(state),
        })
    }
}

/// A reference to an open file on the filesystem.
///
/// See [`std::fs::File`] for more documentation.
#[derive(Debug)]
pub struct File {
    fd: RawFd,
    state: SharedOperationState,
}

impl File {
    /// Configure how to open a file.
    pub const fn config() -> OpenOptions {
        OpenOptions::new()
    }

    /// Open `path` for reading only.
    pub fn open(queue: SubmissionQueue, path: PathBuf) -> Result<Open, QueueFull> {
        File::config().read().open(queue, path)
    }

    /// Read from this file into `buf`.
    ///
    /// # Notes
    ///
    /// This leave the current contents of `buf` untouched and only uses the
    /// spare capacity.
    pub fn read<'f>(&'f self, buf: Vec<u8>) -> Result<Read<'f>, QueueFull> {
        self.read_at(buf, NO_OFFSET)
    }

    /// Read from this file into `buf` starting at `offset`.
    ///
    /// The current file cursor is not affected by this function. This means
    /// that a call `read_at(buf, 1024)` with a buffer of 1kb will **not**
    /// continue reading at 2kb in the next call to `read`.
    ///
    /// # Notes
    ///
    /// This leave the current contents of `buf` untouched and only uses the
    /// spare capacity.
    pub fn read_at<'f>(&'f self, mut buf: Vec<u8>, offset: u64) -> Result<Read<'f>, QueueFull> {
        self.state.start(|submission| unsafe {
            submission.read_at(self.fd, buf.spare_capacity_mut(), offset)
        })?;

        Ok(Read {
            buf: Some(buf),
            file: &self,
        })
    }

    /// Write `buf` to this file.
    pub fn write<'f>(&'f self, buf: Vec<u8>) -> Result<Write<'f>, QueueFull> {
        self.write_at(buf, NO_OFFSET)
    }

    /// Write `buf` to this file.
    ///
    /// The current file cursor is not affected by this function.
    pub fn write_at<'f>(&'f self, buf: Vec<u8>, offset: u64) -> Result<Write<'f>, QueueFull> {
        self.state
            .start(|submission| unsafe { submission.write_at(self.fd, &buf, offset) })?;

        Ok(Write {
            buf: Some(buf),
            file: &self,
        })
    }

    /// Sync all OS-internal metadata to disk.
    ///
    /// # Notes
    ///
    /// Any un-completed writes may not be synced to disk.
    pub fn sync_all<'f>(&'f self) -> Result<SyncAll<'f>, QueueFull> {
        self.state
            .start(|submission| unsafe { submission.sync_all(self.fd) })?;

        Ok(SyncAll { file: &self })
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let result = self
            .state
            .start(|submission| unsafe { submission.close_fd(self.fd) });
        if let Err(err) = result {
            log::error!("error closing file: {}", err);
        }
    }
}

/// [`Future`] to [`open`] a [`File`].
///
/// [`open`]: File::open
#[derive(Debug)]
pub struct Open {
    /// Path used to open the file, need to stay in memory so the kernel can
    /// access it safely.
    path: CString,
    state: Option<SharedOperationState>,
}

impl Future for Open {
    type Output = io::Result<File>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.state.as_ref().unwrap().poll(ctx).map_ok(|fd| File {
            fd,
            state: self.state.take().unwrap(),
        })
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        if self.state.is_some() {
            let path = take(&mut self.path);
            log::debug!("dropped `a10::fs::Open` before completion, leaking path");
            leak(path);
        }
    }
}

macro_rules! op_future {
    (
        // Function name.
        fn $fn: ident -> $result: ty,
        // Future structure.
        struct $name: ident {
            $(
                // Field passed to I/O uring, must be an `Option`.
                // Syntax is the same a struct definition, with `$drop_msg`
                // being the message logged when leaking `$field`.
                $(#[ $field_doc: meta ])*
                $field: ident : $value: ty, $drop_msg: expr,
            )*
        },
        // Mapping functin for `SharedOperationState::poll` result.
        |$self: ident, $n: ident| $map_result: expr,
    ) => {
        #[doc = concat!("[`Future`] to [`", stringify!($fn), "`] from a [`File`].")]
        #[doc = ""]
        #[doc = concat!("[`", stringify!($fn), "`]: File::", stringify!($fn))]
        #[derive(Debug)]
        pub struct $name<'f> {
            $(
                $(#[ $field_doc ])*
                $field: $value,
            )*
            file: &'f File,
        }

        impl<'f> Future for $name<'f> {
            type Output = io::Result<$result>;

            fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
                self.file.state.poll(ctx).map_ok(|$n| {
                    let $self = &mut self;
                    $map_result
                })
            }
        }

        impl<'f> Drop for $name<'f> {
            fn drop(&mut self) {
                $(
                if let Some($field) = take(&mut self.$field) {
                    log::debug!($drop_msg);
                    leak($field);
                }
                )*
            }
        }
    };
    (
        fn $fn: ident -> $result: ty,
        struct $name: ident {
            $(
                $(#[ $field_doc: meta ])*
                $field: ident : $value: ty, $drop_msg: expr,
            )*
        },
        |$n: ident| $map_result: expr,
    ) => {
        op_future!{
            fn $fn -> $result,
            struct $name {
                $(
                    $(#[ $field_doc ])*
                    $field: $value, $drop_msg
                )*
            },
            |_unused_this, $n| $map_result,
        }
    };
}

op_future! {
    fn read -> Vec<u8>,
    struct Read {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: Option<Vec<u8>>, "dropped `a10::fs::Read` before completion, leaking buffer",
    },
    |this, n| {
        let mut buf = this.buf.take().unwrap();
        unsafe { buf.set_len(buf.len() + n as usize) };
        buf
    },
}

op_future! {
    fn write -> (Vec<u8>, usize),
    struct Write {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: Option<Vec<u8>>, "dropped `a10::fs::Write` before completion, leaking buffer",
    },
    |this, n| {
        let buf = this.buf.take().unwrap();
        (buf, n as usize)
    },
}

op_future! {
    fn sync_all -> (),
    struct SyncAll {
        // Doesn't need any fields.
    },
    |n| debug_assert!(n == 0),
}
