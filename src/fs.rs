//! Asynchronous filesystem manipulation operations.
//!
//! The main type is [`OpenOptions`], which can be used to open a file
//! ([`AsyncFd`]).

use std::ffi::{CString, OsString};
use std::future::Future;
use std::mem::zeroed;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{self, Poll};
use std::time::{Duration, SystemTime};
use std::{fmt, io, str};

use crate::extract::Extractor;
use crate::op::{op_future, poll_state, OpState};
use crate::{libc, AsyncFd, Extract, SubmissionQueue};

/// Flags needed to fill [`Metadata`].
const METADATA_FLAGS: u32 = libc::STATX_TYPE
    | libc::STATX_MODE
    | libc::STATX_SIZE
    | libc::STATX_BLOCKS
    | libc::STATX_ATIME
    | libc::STATX_MTIME
    | libc::STATX_BTIME;

/// Options used to configure how a file ([`AsyncFd`]) is opened.
#[derive(Clone, Debug)]
#[must_use = "no file is opened until `a10::fs::OpenOptions::open` or `open_temp_file` is called"]
pub struct OpenOptions {
    flags: libc::c_int,
    mode: libc::mode_t,
}

impl OpenOptions {
    /// Empty `OpenOptions`, has reading enabled by default.
    pub const fn new() -> OpenOptions {
        OpenOptions {
            flags: libc::O_RDONLY | libc::O_CLOEXEC, // NOTE: `O_RDONLY` is 0.
            mode: 0o666,                             // Same as in std lib.
        }
    }

    /// Enable read access.
    ///
    /// Note that read access is already enabled by default, so this is only
    /// useful if you called [`OpenOptions::write_only`] and want to enable read
    /// access as well.
    #[doc(alias = "O_RDONLY")]
    #[doc(alias = "O_RDWR")]
    pub const fn read(mut self) -> Self {
        if (self.flags & libc::O_ACCMODE) == libc::O_WRONLY {
            self.flags &= !libc::O_ACCMODE;
            self.flags |= libc::O_RDWR;
        } // Else we're already in read mode.
        self
    }

    /// Enable write access.
    #[doc(alias = "O_RDWR")]
    pub const fn write(mut self) -> Self {
        if (self.flags & libc::O_ACCMODE) == libc::O_RDONLY {
            self.flags &= !libc::O_ACCMODE;
            self.flags |= libc::O_RDWR;
        } // Else we're already in write mode.
        self
    }

    /// Only enable write access, disabling read access.
    #[doc(alias = "O_WRONLY")]
    pub const fn write_only(mut self) -> Self {
        self.flags &= !libc::O_ACCMODE;
        self.flags |= libc::O_WRONLY;
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
    pub const fn append(mut self) -> Self {
        self.flags |= libc::O_APPEND;
        self
    }

    /// Truncate the file if it exists.
    #[doc(alias = "O_TRUNC")]
    pub const fn truncate(mut self) -> Self {
        self.flags |= libc::O_TRUNC;
        self
    }

    /// If the file doesn't exist create it.
    pub const fn create(mut self) -> Self {
        self.flags |= libc::O_CREAT;
        self
    }

    /// Force a file to be created, failing if a file already exists.
    ///
    /// This options implies [`OpenOptions::create`].
    #[doc(alias = "O_EXCL")]
    #[doc(alias = "O_CREAT")]
    pub const fn create_new(mut self) -> Self {
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
    pub const fn data_sync(mut self) -> Self {
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
    pub const fn sync(mut self) -> Self {
        self.flags |= libc::O_SYNC;
        self
    }

    /// Sets the mode bits that a new file will be created with.
    pub const fn mode(mut self, mode: libc::mode_t) -> Self {
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
    pub fn open_temp_file(mut self, sq: SubmissionQueue, dir: PathBuf) -> Open {
        self.flags |= libc::O_TMPFILE;
        self.open(sq, dir)
    }

    /// Open `path`.
    #[doc(alias = "openat")]
    pub fn open(self, sq: SubmissionQueue, path: PathBuf) -> Open {
        let path = path.into_os_string().into_vec();
        let path = unsafe { CString::from_vec_unchecked(path) };
        Open {
            path: Some(path),
            sq: Some(sq),
            state: OpState::NotStarted((self.flags, self.mode)),
        }
    }
}

/// [`Future`] to [`open`] an asynchronous file ([`AsyncFd`]).
///
/// [`open`]: OpenOptions::open
#[derive(Debug)]
#[must_use = "`Future`s do nothing unless polled"]
pub struct Open {
    /// Path used to open the file, need to stay in memory so the kernel can
    /// access it safely.
    path: Option<CString>,
    sq: Option<SubmissionQueue>,
    state: OpState<(libc::c_int, libc::mode_t)>,
}

impl Future for Open {
    type Output = io::Result<AsyncFd>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        let op_index = poll_state!(
            Open,
            self.state,
            self.sq.as_ref().unwrap(),
            ctx,
            |submission, (flags, mode)| unsafe {
                // SAFETY: `path` is only removed after the state is set to `Done`.
                let path = self.path.as_ref().unwrap();
                submission.open_at(libc::AT_FDCWD, path.as_ptr(), flags, mode);
            }
        );

        match self.sq.as_ref().unwrap().poll_op(ctx, op_index) {
            Poll::Ready(result) => {
                self.state = OpState::Done;
                match result {
                    Ok((_, fd)) => Poll::Ready(Ok(AsyncFd {
                        fd,
                        // SAFETY: unwrapped `sq` above already.
                        sq: self.sq.take().unwrap(),
                    })),
                    Err(err) => Poll::Ready(Err(err)),
                }
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Extract for Open {}

impl Future for Extractor<Open> {
    type Output = io::Result<(AsyncFd, PathBuf)>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.fut).poll(ctx) {
            Poll::Ready(Ok(file)) => {
                let path_bytes = self.fut.path.take().unwrap().into_bytes();
                let path = OsString::from_vec(path_bytes).into();
                Poll::Ready(Ok((file, path)))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            match self.state {
                // SAFETY: only in the `Drop` state is `selfsq` `None`.
                OpState::Running(op_index) => self.sq.as_ref().unwrap().drop_op(op_index, path),
                // NOTE: `Done` should not be reachable, but no point in
                // creating another branch.
                OpState::NotStarted(_) | OpState::Done => drop(path),
            }
        }
    }
}

/// File(system) related system calls.
impl AsyncFd {
    /// Sync all OS-internal metadata to disk.
    ///
    /// # Notes
    ///
    /// Any uncompleted writes may not be synced to disk.
    #[doc(alias = "fsync")]
    pub const fn sync_all<'fd>(&'fd self) -> SyncData<'fd> {
        SyncData::new(self, 0)
    }

    /// This function is similar to [`sync_all`], except that it may not
    /// synchronize file metadata to the filesystem.
    ///
    /// This is intended for use cases that must synchronize content, but don’t
    /// need the metadata on disk. The goal of this method is to reduce disk
    /// operations.
    ///
    /// [`sync_all`]: AsyncFd::sync_all
    ///
    /// # Notes
    ///
    /// Any uncompleted writes may not be synced to disk.
    #[doc(alias = "fdatasync")]
    pub const fn sync_data<'fd>(&'fd self) -> SyncData<'fd> {
        SyncData::new(self, libc::IORING_FSYNC_DATASYNC)
    }

    /// Retrieve metadata about the file.
    #[doc(alias = "statx")]
    pub fn metadata<'fd>(&'fd self) -> Stat<'fd> {
        let metadata = Box::new(Metadata {
            // SAFETY: all zero values are valid representations.
            inner: unsafe { zeroed() },
        });
        Stat::new(self, metadata, ())
    }

    /// Predeclare an access pattern for file data.
    ///
    /// Announce an intention to access file data in a specific pattern in the
    /// future, thus allowing the kernel to perform appropriate optimizations.
    ///
    /// The advice applies to a (not necessarily existent) region starting at
    /// offset and extending for len bytes (or until the end of the file if len
    /// is 0). The advice is not binding; it merely constitutes an expectation
    /// on behalf of the application.
    #[doc(alias = "fadvise")]
    #[doc(alias = "posix_fadvise")]
    pub const fn advise<'fd>(
        &'fd self,
        offset: u64,
        length: u32,
        advice: libc::c_int,
    ) -> Advise<'fd> {
        Advise::new(self, (offset, length, advice))
    }

    /// Manipulate file space.
    ///
    /// Manipulate the allocated disk space for the file referred for the byte
    /// range starting at `offset` and continuing for `length` bytes.
    #[doc(alias = "fallocate")]
    #[doc(alias = "posix_fallocate")]
    pub const fn allocate<'fd>(
        &'fd self,
        offset: u64,
        length: u32,
        mode: libc::c_int,
    ) -> Allocate<'fd> {
        Allocate::new(self, (offset, length, mode))
    }
}

// SyncData.
op_future! {
    fn AsyncFd::sync_all -> (),
    struct SyncData<'fd> {
        // Doesn't need any fields.
    },
    setup_state: flags: u32,
    setup: |submission, fd, (), flags| unsafe {
        submission.fsync(fd.fd, flags);
    },
    map_result: |n| Ok(debug_assert!(n == 0)),
}

// Metadata.
op_future! {
    fn AsyncFd::metadata -> Box<Metadata>,
    struct Stat<'fd> {
        /// Buffer to write the statx data into.
        metadata: Box<Metadata>,
    },
    setup_state: _unused: (),
    setup: |submission, fd, (metadata,), ()| unsafe {
        submission.statx_file(fd.fd, &mut metadata.inner, METADATA_FLAGS);
    },
    map_result: |this, (metadata,), n| {
        debug_assert!(n == 0);
        debug_assert!(metadata.inner.stx_mask & METADATA_FLAGS == METADATA_FLAGS);
        Ok(metadata)
    },
}

// Advise.
op_future! {
    fn AsyncFd::advise -> (),
    struct Advise<'fd> {
        // Doesn't need any fields.
    },
    setup_state: flags: (u64, u32, libc::c_int),
    setup: |submission, fd, (), (offset, length, advise)| unsafe {
        submission.fadvise(fd.fd, offset, length, advise);
    },
    map_result: |this, (), res| {
        debug_assert!(res == 0);
        Ok(())
    },
}

// Allocate.
op_future! {
    fn AsyncFd::allocate -> (),
    struct Allocate<'fd> {
        // Doesn't need any fields.
    },
    setup_state: flags: (u64, u32, libc::c_int),
    setup: |submission, fd, (), (offset, length, mode)| unsafe {
        submission.fallocate(fd.fd, offset, length, mode);
    },
    map_result: |this, (), res| {
        debug_assert!(res == 0);
        Ok(())
    },
}

/// Metadata information about a file.
///
/// See [`AsyncFd::metadata`] and [`Stat`].
#[repr(transparent)]
pub struct Metadata {
    inner: libc::statx,
}

impl Metadata {
    /// Returns the file type for this metadata.
    pub const fn file_type(&self) -> FileType {
        FileType(self.inner.stx_mode)
    }

    /// Returns `true` if this represents a directory.
    #[doc(alias = "S_IFDIR")]
    pub const fn is_dir(&self) -> bool {
        self.file_type().is_dir()
    }

    /// Returns `true` if this represents a file.
    #[doc(alias = "S_IFREG")]
    pub const fn is_file(&self) -> bool {
        self.file_type().is_file()
    }

    /// Returns `true` if this represents a symbolic link.
    #[doc(alias = "S_IFLNK")]
    pub const fn is_symlink(&self) -> bool {
        self.file_type().is_symlink()
    }

    /// Returns the size of the file, in bytes, this metadata is for.
    #[allow(clippy::len_without_is_empty)] // Makes no sense.
    pub const fn len(&self) -> u64 {
        self.inner.stx_size
    }

    /// The "preferred" block size for efficient filesystem I/O.
    pub const fn block_size(&self) -> u32 {
        self.inner.stx_blksize
    }

    /// Returns the permissions of the file this metadata is for.
    pub const fn permissions(&self) -> Permissions {
        Permissions(self.inner.stx_mode)
    }

    /// Returns the time this file was last modified.
    pub fn modified(&self) -> SystemTime {
        timestamp(&self.inner.stx_mtime)
    }

    /// Returns the time this file was last accessed.
    ///
    /// # Notes
    ///
    /// It's possible to disable keeping track of this access time, which makes
    /// this function return an invalid value.
    pub fn accessed(&self) -> SystemTime {
        timestamp(&self.inner.stx_atime)
    }

    /// Returns the time this file was created.
    pub fn created(&self) -> SystemTime {
        timestamp(&self.inner.stx_btime)
    }
}

#[allow(clippy::cast_sign_loss)] // Checked.
fn timestamp(ts: &libc::statx_timestamp) -> SystemTime {
    let dur = Duration::new(ts.tv_sec as u64, ts.tv_nsec);
    if ts.tv_sec.is_negative() {
        SystemTime::UNIX_EPOCH - dur
    } else {
        SystemTime::UNIX_EPOCH + dur
    }
}

impl fmt::Debug for Metadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Metadata")
            .field("file_type", &self.file_type())
            .field("len", &self.len())
            .field("block_size", &self.block_size())
            .field("permissions", &self.permissions())
            .field("modified", &self.modified())
            .field("accessed", &self.accessed())
            .field("created", &self.created())
            .finish()
    }
}

/// A structure representing a type of file with accessors for each file type.
///
/// See [`Metadata`].
#[derive(Copy, Clone)]
pub struct FileType(u16);

impl FileType {
    /// Returns `true` if this represents a directory.
    #[doc(alias = "S_IFDIR")]
    pub const fn is_dir(&self) -> bool {
        (self.0 & libc::S_IFMT as u16) == libc::S_IFDIR as u16
    }

    /// Returns `true` if this represents a file.
    #[doc(alias = "S_IFREG")]
    pub const fn is_file(&self) -> bool {
        (self.0 & libc::S_IFMT as u16) == libc::S_IFREG as u16
    }

    /// Returns `true` if this represents a symbolic link.
    #[doc(alias = "S_IFLNK")]
    pub const fn is_symlink(&self) -> bool {
        (self.0 & libc::S_IFMT as u16) == libc::S_IFLNK as u16
    }

    /// Returns `true` if this represents a socket.
    #[doc(alias = "S_IFSOCK")]
    pub const fn is_socket(&self) -> bool {
        (self.0 & libc::S_IFMT as u16) == libc::S_IFSOCK as u16
    }

    /// Returns `true` if this represents a block device.
    #[doc(alias = "S_IFBLK")]
    pub const fn is_block_device(&self) -> bool {
        (self.0 & libc::S_IFMT as u16) == libc::S_IFBLK as u16
    }

    /// Returns `true` if this represents a character device.
    #[doc(alias = "S_IFCHR")]
    pub const fn is_character_device(&self) -> bool {
        (self.0 & libc::S_IFMT as u16) == libc::S_IFCHR as u16
    }

    /// Returns `true` if this represents a named fifo pipe.
    #[doc(alias = "S_IFIFO")]
    pub const fn is_named_pipe(&self) -> bool {
        (self.0 & libc::S_IFMT as u16) == libc::S_IFIFO as u16
    }
}

impl fmt::Debug for FileType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ty = if f.alternate() {
            if self.is_dir() {
                "directory"
            } else if self.is_file() {
                "file"
            } else if self.is_symlink() {
                "symbolic link"
            } else if self.is_socket() {
                "socket"
            } else if self.is_block_device() {
                "block device"
            } else if self.is_character_device() {
                "character device"
            } else if self.is_named_pipe() {
                "named pipe"
            } else {
                "unknown"
            }
        } else if self.is_dir() {
            "d"
        } else if self.is_file() {
            "-"
        } else if self.is_symlink() {
            "l"
        } else if self.is_socket() {
            "s"
        } else if self.is_block_device() {
            "b"
        } else if self.is_character_device() {
            "c"
        } else if self.is_named_pipe() {
            "p"
        } else {
            "?"
        };
        f.debug_tuple("FileType").field(&ty).finish()
    }
}

/// Access permissions.
///
/// See [`Metadata`].
#[derive(Copy, Clone)]
pub struct Permissions(u16);

impl Permissions {
    /// Return `true` if the owner has read permission.
    #[doc(alias = "S_IRUSR")]
    pub const fn owner_can_read(&self) -> bool {
        self.0 & libc::S_IRUSR as u16 != 0
    }

    /// Return `true` if the owner has write permission.
    #[doc(alias = "S_IWUSR")]
    pub const fn owner_can_write(&self) -> bool {
        self.0 & libc::S_IWUSR as u16 != 0
    }

    /// Return `true` if the owner has execute permission.
    #[doc(alias = "S_IXUSR")]
    pub const fn owner_can_execute(&self) -> bool {
        self.0 & libc::S_IXUSR as u16 != 0
    }

    /// Return `true` if the group the file belongs to has read permission.
    #[doc(alias = "S_IRGRP")]
    pub const fn group_can_read(&self) -> bool {
        self.0 & libc::S_IRGRP as u16 != 0
    }

    /// Return `true` if the group the file belongs to has write permission.
    #[doc(alias = "S_IWGRP")]
    pub const fn group_can_write(&self) -> bool {
        self.0 & libc::S_IWGRP as u16 != 0
    }

    /// Return `true` if the group the file belongs to has execute permission.
    #[doc(alias = "S_IXGRP")]
    pub const fn group_can_execute(&self) -> bool {
        self.0 & libc::S_IXGRP as u16 != 0
    }

    /// Return `true` if others have read permission.
    #[doc(alias = "S_IROTH")]
    pub const fn others_can_read(&self) -> bool {
        self.0 & libc::S_IROTH as u16 != 0
    }

    /// Return `true` if others have write permission.
    #[doc(alias = "S_IWOTH")]
    pub const fn others_can_write(&self) -> bool {
        self.0 & libc::S_IWOTH as u16 != 0
    }

    /// Return `true` if others have execute permission.
    #[doc(alias = "S_IXOTH")]
    pub const fn others_can_execute(&self) -> bool {
        self.0 & libc::S_IXOTH as u16 != 0
    }
}

impl fmt::Debug for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Create the same format as `ls(1)` uses.
        let mut buf = [b'-'; 9];
        if self.owner_can_read() {
            buf[0] = b'r';
        }
        if self.owner_can_write() {
            buf[1] = b'w';
        }
        if self.owner_can_execute() {
            buf[2] = b'x';
        }
        if self.group_can_read() {
            buf[3] = b'r';
        }
        if self.group_can_write() {
            buf[4] = b'w';
        }
        if self.group_can_execute() {
            buf[5] = b'x';
        }
        if self.others_can_read() {
            buf[6] = b'r';
        }
        if self.others_can_write() {
            buf[7] = b'w';
        }
        if self.others_can_execute() {
            buf[8] = b'x';
        }
        let permissions = str::from_utf8(&buf).unwrap();
        f.debug_tuple("Permissions").field(&permissions).finish()
    }
}
