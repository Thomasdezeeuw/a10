//! Asynchronous filesystem manipulation operations.
//!
//! The main type is [`File`], which can be opened using [`OpenOptions`]. All
//! functions on `File` are asynchronous and return a [`Future`].

use std::ffi::{CString, OsString};
use std::future::Future;
use std::mem::{forget as leak, take, zeroed};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::io::RawFd;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{self, Poll};
use std::time::{Duration, SystemTime};
use std::{fmt, io, str};

use crate::extract::Extractor;
use crate::op::{SharedOperationState, NO_OFFSET};
use crate::{libc, Extract, QueueFull, SubmissionQueue};

const METADATA_FLAGS: u32 = libc::STATX_TYPE
    | libc::STATX_MODE
    | libc::STATX_SIZE
    | libc::STATX_BLOCKS
    | libc::STATX_ATIME
    | libc::STATX_MTIME
    | libc::STATX_BTIME;

/// Options used to configure how a [`File`] is opened.
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
    pub const fn read(mut self) -> Self {
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
    pub const fn write(mut self) -> Self {
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
            path: Some(path),
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
            file: self,
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
            file: self,
        })
    }

    /// Sync all OS-internal metadata to disk.
    ///
    /// # Notes
    ///
    /// Any uncompleted writes may not be synced to disk.
    #[doc(alias = "fsync")]
    pub fn sync_all<'f>(&'f self) -> Result<SyncAll<'f>, QueueFull> {
        self.state
            .start(|submission| unsafe { submission.sync_all(self.fd) })?;

        Ok(SyncAll { file: self })
    }

    /// This function is similar to [`sync_all`], except that it may not
    /// synchronize file metadata to the filesystem.
    ///
    /// This is intended for use cases that must synchronize content, but don’t
    /// need the metadata on disk. The goal of this method is to reduce disk
    /// operations.
    ///
    /// [`sync_all`]: File::sync_all
    ///
    /// # Notes
    ///
    /// Any uncompleted writes may not be synced to disk.
    #[doc(alias = "fdatasync")]
    pub fn sync_data<'f>(&'f self) -> Result<SyncData<'f>, QueueFull> {
        self.state
            .start(|submission| unsafe { submission.sync_data(self.fd) })?;

        Ok(SyncData { file: self })
    }

    /// Retrieve metadata about the file.
    #[doc(alias = "statx")]
    pub fn metadata<'f>(&'f self) -> Result<Stat<'f>, QueueFull> {
        let mut metadata = Box::new(Metadata {
            // SAFETY: all zero values are valid representations.
            inner: unsafe { zeroed() },
        });
        self.state.start(|submission| unsafe {
            submission.statx_file(self.fd, &mut metadata.inner, METADATA_FLAGS)
        })?;

        Ok(Stat {
            metadata: Some(metadata),
            file: self,
        })
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
    path: Option<CString>,
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

impl Extract for Open {}

impl Future for Extractor<Open> {
    type Output = io::Result<(File, PathBuf)>;

    fn poll(mut self: Pin<&mut Self>, ctx: &mut task::Context<'_>) -> Poll<Self::Output> {
        self.fut.state.as_ref().unwrap().poll(ctx).map_ok(|fd| {
            let file = File {
                fd,
                state: self.fut.state.take().unwrap(),
            };
            let path_bytes = self.fut.path.take().unwrap().into_bytes();
            let path = OsString::from_vec(path_bytes).into();
            (file, path)
        })
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        if self.state.is_some() {
            let path = self.path.take().unwrap();
            log::debug!("dropped `a10::fs::Open` before completion, leaking path");
            leak(path);
        }
    }
}

op_future! {
    fn File::read -> Vec<u8>,
    struct Read<'f> {
        /// Buffer to write into, needs to stay in memory so the kernel can
        /// access it safely.
        buf: Option<Vec<u8>>, "dropped `a10::fs::Read` before completion, leaking buffer",
    },
    |this, n| {
        let mut buf = this.buf.take().unwrap();
        unsafe { buf.set_len(buf.len() + n as usize) };
        Ok(buf)
    },
}

op_future! {
    fn File::write -> usize,
    struct Write<'f> {
        /// Buffer to read from, needs to stay in memory so the kernel can
        /// access it safely.
        buf: Option<Vec<u8>>, "dropped `a10::fs::Write` before completion, leaking buffer",
    },
    |n| Ok(n as usize),
    extract: |this, n| -> (Vec<u8>, usize) {
        let buf = this.buf.take().unwrap();
        Ok((buf, n as usize))
    }
}

op_future! {
    fn File::sync_all -> (),
    struct SyncAll<'a> {
        // Doesn't need any fields.
    },
    |n| Ok(debug_assert!(n == 0)),
}

op_future! {
    fn File::sync_data -> (),
    struct SyncData<'a> {
        // Doesn't need any fields.
    },
    |n| Ok(debug_assert!(n == 0)),
}

/// Metadata information about a [`File`].
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

fn timestamp(ts: &libc::statx_timestamp) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::new(ts.tv_sec as u64, ts.tv_nsec)
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
        } else {
            if self.is_dir() {
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
            }
        };
        f.debug_tuple("FileType").field(&ty).finish()
    }
}

/// Access permissions.
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
            buf[0] = b'r'
        }
        if self.owner_can_write() {
            buf[1] = b'w'
        }
        if self.owner_can_execute() {
            buf[2] = b'x'
        }
        if self.group_can_read() {
            buf[3] = b'r'
        }
        if self.group_can_write() {
            buf[4] = b'w'
        }
        if self.group_can_execute() {
            buf[5] = b'x'
        }
        if self.others_can_read() {
            buf[6] = b'r'
        }
        if self.others_can_write() {
            buf[7] = b'w'
        }
        if self.others_can_execute() {
            buf[8] = b'x'
        }
        let permissions = str::from_utf8(&buf).unwrap();
        f.debug_tuple("Permissions").field(&permissions).finish()
    }
}

op_future! {
    fn File::metadata -> Box<Metadata>,
    struct Stat<'f> {
        /// Buffer to write the statx data into.
        metadata: Option<Box<Metadata>>, "dropped `a10::fs::Stat` before completion, leaking metadata buffer",
    },
    |this, n| {
        debug_assert!(n == 0);
        let metadata = this.metadata.take().unwrap();
        assert!(metadata.inner.stx_mask & METADATA_FLAGS == METADATA_FLAGS);
        Ok(metadata)
    },
}
