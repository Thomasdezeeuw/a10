//! Filesystem manipulation operations.
//!
//! To open a file ([`AsyncFd`]) use [`open_file`] or [`OpenOptions`].

use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::time::SystemTime;
use std::{fmt, io, mem, str};

#[cfg(any(target_os = "android", target_os = "linux"))]
use crate::op::OpState;
use crate::op::{fd_operation, operation};
use crate::{AsyncFd, SubmissionQueue, fd, man_link, new_flag, sys};

#[cfg(any(target_os = "android", target_os = "linux"))]
pub mod notify;
#[cfg(any(target_os = "android", target_os = "linux"))]
pub use notify::Watcher;

/// Options used to configure how a file ([`AsyncFd`]) is opened.
#[derive(Clone, Debug)]
#[must_use = "no file is opened until `a10::fs::OpenOptions::open` or `open_temp_file` is called"]
pub struct OpenOptions {
    flags: libc::c_int,
    mode: libc::mode_t,
    kind: fd::Kind,
}

impl OpenOptions {
    /// Empty `OpenOptions`, has reading enabled by default.
    pub const fn new() -> OpenOptions {
        OpenOptions {
            flags: libc::O_RDONLY, // NOTE: `O_RDONLY` is 0.
            mode: 0o666,           // Same as in std lib.
            kind: fd::Kind::File,
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

    /// Try to minimize cache effects of the I/O to and from this file.
    ///
    /// File I/O is done directly to/from user-space buffers. This uses the
    /// `O_DIRECT` flag which on its own makes an effort to transfer data
    /// synchronously, but does not give the guarantees of the `O_SYNC` flag
    /// ([`OpenOptions::sync`]) that data and necessary metadata are
    /// transferred. To guarantee synchronous I/O, `O_SYNC` must be used in
    /// addition to `O_DIRECT`.
    #[doc(alias = "O_DIRECT")]
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "linux",
        target_os = "netbsd"
    ))]
    pub const fn direct(mut self) -> Self {
        self.flags |= libc::O_DIRECT;
        self
    }

    /// Sets the mode bits that a new file will be created with.
    pub const fn mode(mut self, mode: u32) -> Self {
        self.mode = mode as _;
        self
    }

    /// Set the kind of descriptor to use.
    ///
    /// Defaults to a regular [`File`] descriptor.
    ///
    /// [`File`]: fd::Kind::File
    pub const fn kind(mut self, kind: fd::Kind) -> Self {
        self.kind = kind;
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
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn open_temp_file(mut self, sq: SubmissionQueue, dir: PathBuf) -> Open {
        self.flags |= libc::O_TMPFILE;
        self.open(sq, dir)
    }

    /// Open `path`.
    #[doc = man_link!(openat(2))]
    #[doc(alias = "openat")]
    pub fn open(self, sq: SubmissionQueue, path: PathBuf) -> Open {
        let args = (self.flags | self.kind.cloexec_flag(), self.mode);
        Open::new(sq, (path_to_cstring(path), self.kind), args)
    }
}

/// Open a file in read-only mode.
#[doc = man_link!(openat(2))]
pub fn open_file(sq: SubmissionQueue, path: PathBuf) -> Open {
    OpenOptions::new().read().open(sq, path)
}

/// Creates a new, empty directory.
#[doc = man_link!(mkdirat(2))]
pub fn create_dir(sq: SubmissionQueue, path: PathBuf) -> CreateDir {
    CreateDir::new(sq, path_to_cstring(path), ())
}

/// Rename a file or directory to a new name.
#[doc = man_link!(rename(2))]
pub fn rename(sq: SubmissionQueue, from: PathBuf, to: PathBuf) -> Rename {
    let resources = (path_to_cstring(from), path_to_cstring(to));
    Rename::new(sq, resources, ())
}

/// Remove a file.
#[doc = man_link!(unlinkat(2))]
#[doc(alias = "unlink")]
#[doc(alias = "unlinkat")]
pub fn remove_file(sq: SubmissionQueue, path: PathBuf) -> Delete {
    Delete::new(sq, path_to_cstring(path), RemoveFlag::File)
}

/// Remove a directory.
#[doc = man_link!(unlinkat(2))]
#[doc(alias = "rmdir")]
#[doc(alias = "unlinkat")]
pub fn remove_dir(sq: SubmissionQueue, path: PathBuf) -> Delete {
    let path = path_to_cstring(path);
    Delete::new(sq, path, RemoveFlag::Directory)
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum RemoveFlag {
    File,
    Directory,
}

operation!(
    /// [`Future`] behind [`OpenOptions::open`] and [`open_file`].
    pub struct Open(sys::fs::OpenOp) -> io::Result<AsyncFd>,
      impl Extract -> io::Result<(AsyncFd, PathBuf)>;

    /// [`Future`] behind [`create_dir`].
    pub struct CreateDir(sys::fs::CreateDirOp) -> io::Result<()>,
      impl Extract -> io::Result<PathBuf>;

    /// [`Future`] behind [`rename`].
    pub struct Rename(sys::fs::RenameOp) -> io::Result<()>,
      impl Extract -> io::Result<(PathBuf, PathBuf)>;

    /// [`Future`] behind [`remove_file`] and [`remove_dir`].
    pub struct Delete(sys::fs::DeleteOp) -> io::Result<()>,
      impl Extract -> io::Result<PathBuf>;
);

/// File(system) related system calls.
impl AsyncFd {
    /// Sync all OS-internal metadata to disk.
    ///
    /// # Notes
    ///
    /// Any uncompleted writes may not be synced to disk.
    ///
    /// On macOS this uses `fcntl(fd, F_FULLFSYNC, 0)` as `fsync(2)` doesn't
    /// behave as it does on other OS and `F_FULLFSYNC` is.
    #[doc = man_link!(fsync(2))]
    #[doc(alias = "fsync")]
    pub fn sync_all<'fd>(&'fd self) -> SyncData<'fd> {
        SyncData::new(self, (), SyncDataFlag::All)
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
    ///
    /// On macOS this uses `fcntl(fd, F_FULLFSYNC, 0)` as `fsync(2)` doesn't
    /// behave as it does on other OS and `F_FULLFSYNC` is.
    #[doc = man_link!(fsync(2))]
    #[doc(alias = "fdatasync")]
    pub fn sync_data<'fd>(&'fd self) -> SyncData<'fd> {
        SyncData::new(self, (), SyncDataFlag::Data)
    }

    /// Retrieve metadata about the file.
    #[doc = man_link!(statx(2))]
    #[doc(alias = "stat")]
    #[doc(alias = "statx")]
    pub fn metadata<'fd>(&'fd self) -> Stat<'fd> {
        // SAFETY: fully zeroed `libc::statx` and `libc::stat` are valid values.
        let metadata = unsafe { mem::zeroed() };
        let interest = sys::fs::default_metadata_interest();
        Stat::new(self, metadata, interest)
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
    #[doc = man_link!(posix_fadvise(2))]
    #[doc(alias = "fadvise")]
    #[doc(alias = "posix_fadvise")]
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "linux",
        target_os = "netbsd",
    ))]
    pub fn advise<'fd>(&'fd self, offset: u64, length: u32, advice: AdviseFlag) -> Advise<'fd> {
        Advise::new(self, (), (offset, length, advice))
    }

    /// Manipulate file space.
    ///
    /// Manipulate the allocated disk space for the file referred for the byte
    /// range starting at `offset` and continuing for `length` bytes.
    #[doc = man_link!(fallocate(2))]
    #[doc(alias = "fallocate")]
    #[doc(alias = "posix_fallocate")]
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "linux",
        target_os = "netbsd",
    ))]
    pub fn allocate<'fd>(
        &'fd self,
        offset: u64,
        length: u32,
        mode: Option<AllocateFlag>,
    ) -> Allocate<'fd> {
        let mode = match mode {
            Some(mode) => mode,
            None => AllocateFlag(0),
        };
        Allocate::new(self, (), (offset, length, mode))
    }

    /// Truncate the file to `length`.
    ///
    /// If the file previously was larger than this size, the extra data is
    /// lost. If the file previously was shorter, it is extended, and the
    /// extended part reads as null bytes.
    #[doc = man_link!(ftruncate(2))]
    #[doc(alias = "ftruncate")]
    pub fn truncate<'fd>(&'fd self, length: u64) -> Truncate<'fd> {
        Truncate::new(self, (), length)
    }
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum SyncDataFlag {
    All,
    Data,
}

new_flag!(
    /// Interest in specific metadata.
    pub struct MetadataInterest(u32) impl BitOr {
        /// Enables [`Metadata::file_type`] and related `is_*` methods.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        TYPE = libc::STATX_TYPE,
        /// Enables [`Metadata::len`].
        #[cfg(any(target_os = "android", target_os = "linux"))]
        SIZE = libc::STATX_SIZE,
        /// Enables [`Metadata::block_size`].
        #[cfg(any(target_os = "android", target_os = "linux"))]
        BLOCKS = libc::STATX_BLOCKS,
        /// Enables [`Metadata::permissions`].
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MODE = libc::STATX_MODE,
        /// Enables [`Metadata::modified`].
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MODIFIED_TIME = libc::STATX_MTIME,
        /// Enables [`Metadata::accessed`].
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ACCESSED_TIME = libc::STATX_ATIME,
        /// Enables [`Metadata::created`].
        #[cfg(any(target_os = "android", target_os = "linux"))]
        CREATED_TIME = libc::STATX_BTIME,
    }
);

#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd",
))]
new_flag!(
    /// Advise about data access.
    ///
    /// See [`AsyncFd::advise`].
    pub struct AdviseFlag(u32) {
        /// No advice.
        NORMAL = libc::POSIX_FADV_NORMAL,
        /// Data will be accessed sequentially.
        SEQUENTIAL = libc::POSIX_FADV_SEQUENTIAL,
        /// Data will be accessed randomly.
        RANDOM = libc::POSIX_FADV_RANDOM,
        /// Data will be accessed only once.
        NO_REUSE = libc::POSIX_FADV_NOREUSE,
        /// Data will be accessed in the near future.
        WILL_NEED = libc::POSIX_FADV_WILLNEED,
        /// Data will not be accessed in the near future.
        DONT_NEED = libc::POSIX_FADV_DONTNEED,
    }

    /// Mode for call to [`AsyncFd::allocate`].
    pub struct AllocateFlag(u32) impl BitOr {
        /// Keep the same file size.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        KEEP_SIZE = libc::FALLOC_FL_KEEP_SIZE,
        /// Guarantee that a subsequent write will not fail due to lack of
        /// space.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        UNSHARE_RANGE = libc::FALLOC_FL_UNSHARE_RANGE,
        /// Deallocate the space.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PUNCH_HOLE = libc::FALLOC_FL_PUNCH_HOLE,
        /// Remove the byte range from the file, without leaving a hole.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        COLLAPSE_RANGE = libc::FALLOC_FL_COLLAPSE_RANGE,
        /// Zero the byte range.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ZERO_RANGE = libc::FALLOC_FL_ZERO_RANGE,
        /// Inserta  hole in the file without overwriting any existing data.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        INSERT_RANGE = libc::FALLOC_FL_INSERT_RANGE,
    }
);

fd_operation!(
    /// [`Future`] behind [`AsyncFd::sync_all`] and [`AsyncFd::sync_data`].
    pub struct SyncData(sys::fs::SyncDataOp) -> io::Result<()>;

    /// [`Future`] behind [`AsyncFd::metadata`].
    pub struct Stat(sys::fs::StatOp) -> io::Result<Metadata>;

    /// [`Future`] behind [`AsyncFd::truncate`].
    pub struct Truncate(sys::fs::TruncateOp) -> io::Result<()>;
);

#[cfg(any(
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd",
))]
fd_operation!(
    /// [`Future`] behind [`AsyncFd::advise`].
    pub struct Advise(sys::fs::AdviseOp) -> io::Result<()>;

    /// [`Future`] behind [`AsyncFd::allocate`].
    pub struct Allocate(sys::fs::AllocateOp) -> io::Result<()>;
);

impl<'fd> Stat<'fd> {
    /// Set which field(s) of the metadata the kernel should fill.
    ///
    /// Defaults to filling some basic fields.
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub fn only(mut self, mask: MetadataInterest) -> Self {
        if let Some(args) = self.state.args_mut() {
            *args = mask;
        }
        self
    }
}

/// Metadata information about a file.
///
/// See [`AsyncFd::metadata`] and [`Stat`].
#[repr(transparent)]
pub struct Metadata(pub(crate) sys::fs::Stat);

impl Metadata {
    /// Which field(s) of the metadata are filled (based on the provided
    /// interest).
    #[cfg(any(target_os = "android", target_os = "linux"))]
    pub const fn filled(&self) -> MetadataInterest {
        sys::fs::filled(&self.0)
    }

    /// Returns the file type for this metadata.
    pub const fn file_type(&self) -> FileType {
        sys::fs::file_type(&self.0)
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
        sys::fs::len(&self.0)
    }

    /// The "preferred" block size for efficient filesystem I/O.
    pub const fn block_size(&self) -> u32 {
        sys::fs::block_size(&self.0)
    }

    /// Returns the permissions of the file this metadata is for.
    pub const fn permissions(&self) -> Permissions {
        sys::fs::permissions(&self.0)
    }

    /// Returns the time this file was last modified.
    pub fn modified(&self) -> SystemTime {
        sys::fs::modified(&self.0)
    }

    /// Returns the time this file was last accessed.
    ///
    /// # Notes
    ///
    /// It's possible to disable keeping track of this access time, which makes
    /// this function return an invalid value.
    pub fn accessed(&self) -> SystemTime {
        sys::fs::accessed(&self.0)
    }

    /// Returns the time this file was created.
    pub fn created(&self) -> SystemTime {
        sys::fs::created(&self.0)
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
pub struct FileType(pub(crate) u16);

#[allow(clippy::unnecessary_cast)] // Some OS define S_IFMT as u16, but not all.
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
pub struct Permissions(pub(crate) u16);

#[allow(clippy::unnecessary_cast)] // Some OS define S_IRUSR as u16, but not all.
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
        // SAFETY: we only set ASCII bytes, which makes the string valid UTF-8.
        let permissions = unsafe { str::from_utf8_unchecked(&buf) };
        f.debug_tuple("Permissions").field(&permissions).finish()
    }
}

fn path_to_cstring(path: PathBuf) -> CString {
    unsafe { CString::from_vec_unchecked(path.into_os_string().into_vec()) }
}

pub(crate) fn path_from_cstring(path: CString) -> PathBuf {
    OsString::from_vec(path.into_bytes()).into()
}
