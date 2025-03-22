use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::os::fd::RawFd;
use std::{io, ptr, slice};

use crate::fd::{AsyncFd, Descriptor};
use crate::io::{Buf, BufId, BufMut, BufMutSlice, BufSlice, Buffer, ReadBuf, ReadBufPool};
use crate::kqueue::{self, sync_op_impl};
use crate::net::{AddressStorage, NoAddress, SendCall, SocketAddress};
use crate::op::{FdIter, FdOpExtract, OpResult};
use crate::{syscall, SubmissionQueue};

pub(crate) use crate::unix::MsgHeader;

pub(crate) struct SocketOp<D>(PhantomData<*const D>);

impl<D: Descriptor> kqueue::Op for SocketOp<D> {
    type Output = AsyncFd<D>;
    type Resources = ();
    type Args = (libc::c_int, libc::c_int, libc::c_int, libc::c_int); // domain, type, protocol, flags

    sync_op_impl!();

    fn sync_operation(
        sq: &SubmissionQueue,
        (): Self::Resources,
        (domain, r#type, protocol, _): Self::Args,
    ) -> io::Result<Self::Output> {
        let fd = syscall!(socket(domain, r#type, protocol))?;
        // SAFETY: kernel ensures `fd` is valid.
        Ok(unsafe { AsyncFd::from_raw(fd, sq.clone()) })
    }
}
