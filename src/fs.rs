//! Asynchronous filesystem manipulation operations.

use std::ffi::CString;
use std::io;
use std::mem::{forget as leak, take};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::io::RawFd;
use std::path::PathBuf;
use std::task::Poll;

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

/// [`Future`] to open a [`File`].
#[derive(Debug)]
pub struct Open {
    /// Path used to open the file, need to stay in memory so the kernel can
    /// access it safely.
    path: CString,
    state: Option<SharedOperationState>,
}

impl Open {
    // TODO: replace with `Future` impl.
    pub fn check(&mut self) -> Poll<io::Result<File>> {
        let result = self.state.as_mut().unwrap().take_result();
        match result {
            None => Poll::Pending,
            Some(Ok(fd)) => Poll::Ready(Ok(File {
                fd,
                state: self.state.take().unwrap(),
            })),
            Some(Err(err)) => Poll::Ready(Err(err)),
        }
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

/// [`Future`] to read from a [`File`].
#[derive(Debug)]
pub struct Read<'f> {
    /// Buffer to write into, needs to stay in memory so the kernel can access
    /// it safely.
    buf: Option<Vec<u8>>,
    file: &'f File,
}

impl<'f> Read<'f> {
    // TODO: replace with `Future` impl.
    pub fn check(&mut self) -> Poll<io::Result<Vec<u8>>> {
        let result = self.file.state.take_result();
        match result {
            None => Poll::Pending,
            Some(Ok(n)) => Poll::Ready(Ok({
                let mut buf = self.buf.take().unwrap();
                unsafe { buf.set_len(buf.len() + n as usize) };
                buf
            })),
            Some(Err(err)) => Poll::Ready(Err(err)),
        }
    }
}

impl<'f> Drop for Read<'f> {
    fn drop(&mut self) {
        if let Some(buf) = take(&mut self.buf) {
            log::debug!("dropped `a10::fs::Read` before completion, leaking buffer");
            leak(buf);
        }
    }
}

/// [`Future`] to write to a [`File`].
#[derive(Debug)]
pub struct Write<'f> {
    /// Buffer to read from, needs to stay in memory so the kernel can access it
    /// safely.
    buf: Option<Vec<u8>>,
    file: &'f File,
}

impl<'f> Write<'f> {
    // TODO: replace with `Future` impl.
    pub fn check(&mut self) -> Poll<io::Result<(Vec<u8>, usize)>> {
        let result = self.file.state.take_result();
        match result {
            None => Poll::Pending,
            Some(Ok(n)) => Poll::Ready(Ok({
                let buf = self.buf.take().unwrap();
                (buf, n as usize)
            })),
            Some(Err(err)) => Poll::Ready(Err(err)),
        }
    }
}

impl<'f> Drop for Write<'f> {
    fn drop(&mut self) {
        if let Some(buf) = take(&mut self.buf) {
            log::debug!("dropped `a10::fs::Write` before completion, leaking buffer");
            leak(buf);
        }
    }
}
