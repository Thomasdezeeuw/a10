impl AsyncFd<File> {
    /// Creates a new independently owned `AsyncFd` that shares the same
    /// underlying file descriptor as the existing `AsyncFd`.
    #[doc(alias = "dup")]
    #[doc(alias = "dup2")]
    #[doc(alias = "F_DUPFD")]
    #[doc(alias = "F_DUPFD_CLOEXEC")]
    pub fn try_clone(&self) -> io::Result<AsyncFd> {
        let fd = self.fd.try_clone()?;
        Ok(AsyncFd::new(fd, self.sq.clone()))
    }

    /// Convert a regular file descriptor into a direct descriptor.
    ///
    /// The file descriptor can continued to be used and the lifetimes of the
    /// file descriptor and the newly returned direct descriptor are not
    /// connected.
    ///
    /// # Notes
    ///
    /// The [`Ring`] must be configured [`with_direct_descriptors`] enabled,
    /// otherwise this will return `ENXIO`.
    ///
    /// [`Ring`]: crate::Ring
    /// [`with_direct_descriptors`]: crate::Config::with_direct_descriptors
    #[doc(alias = "IORING_OP_FILES_UPDATE")]
    #[doc(alias = "IORING_FILE_INDEX_ALLOC")]
    pub fn to_direct_descriptor<'fd>(&'fd self) -> ToDirect<'fd, File> {
        ToDirect::new(self, Box::new(self.fd()), ())
    }
}

/// Operations only available on direct descriptors.
impl AsyncFd<Direct> {
    /// Create a new `AsyncFd` from a direct descriptor.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `direct_fd` is valid and that it's no longer
    /// used by anything other than the returned `AsyncFd`. Furthermore the
    /// caller must ensure the direct descriptor is actually a direct
    /// descriptor.
    pub(crate) unsafe fn from_direct_fd(direct_fd: RawFd, sq: SubmissionQueue) -> AsyncFd<Direct> {
        AsyncFd::from_raw(direct_fd, sq)
    }

    /// Convert a direct descriptor into a regular file descriptor.
    ///
    /// The direct descriptor can continued to be used and the lifetimes of the
    /// direct descriptor and the newly returned file descriptor are not
    /// connected.
    ///
    /// # Notes
    ///
    /// Requires Linux 6.8.
    #[doc(alias = "IORING_OP_FIXED_FD_INSTALL")]
    pub const fn to_file_descriptor<'fd>(&'fd self) -> ToFd<'fd, Direct> {
        ToFd::new(self, ())
    }
}

// ToFd.
op_future! {
    fn AsyncFd::to_file_descriptor -> AsyncFd<File>,
    struct ToFd<'fd> {
        // No state needed.
    },
    setup_state: _unused: (),
    setup: |submission, fd, (), ()| unsafe {
        // NOTE: flags must currently be null.
        submission.create_file_descriptor(fd.fd(), 0);
    },
    map_result: |this, (), fd| {
        let sq = this.fd.sq.clone();
        // SAFETY: the fixed fd intall operation ensures that `fd` is valid.
        let fd = unsafe { AsyncFd::from_raw_fd(fd, sq) };
        Ok(fd)
    },
}

// ToDirect.
// NOTE: keep this in sync with the `process::ToSignalsDirect` implementation.
op_future! {
    fn AsyncFd::to_direct_descriptor -> AsyncFd<Direct>,
    struct ToDirect<'fd> {
        /// The file descriptor we're changing into a direct descriptor, needs
        /// to stay in memory so the kernel can access it safely.
        direct_fd: Box<RawFd>,
    },
    setup_state: _unused: (),
    setup: |submission, fd, (direct_fd,), ()| unsafe {
        submission.create_direct_descriptor(&mut **direct_fd, 1);
    },
    map_result: |this, (direct_fd,), res| {
        debug_assert!(res == 1);
        let sq = this.fd.sq.clone();
        // SAFETY: the files update operation ensures that `direct_fd` is valid.
        let fd = unsafe { AsyncFd::from_direct_fd(*direct_fd, sq) };
        Ok(fd)
    },
}
