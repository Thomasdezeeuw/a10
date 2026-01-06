//! Networking.
//!
//! To create a new socket ([`AsyncFd`]) use the [`socket`] function, which
//! issues a non-blocking `socket(2)` call.

use std::ffi::OsStr;
use std::future::Future;
use std::mem::{self, MaybeUninit};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
#[cfg(any(target_os = "android", target_os = "linux"))]
use std::os::linux::net::SocketAddrExt;
use std::os::unix;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::pin::Pin;
use std::task::{self, Poll};
use std::{fmt, io, ptr, slice};

use crate::op::{Operation, operation};
use crate::{AsyncFd, SubmissionQueue, fd, man_link, new_flag, sys};

/// Creates a new socket.
#[doc = man_link!(socket(2))]
pub fn socket(
    sq: SubmissionQueue,
    domain: Domain,
    r#type: Type,
    protocol: Option<Protocol>,
) -> Socket {
    let args = (domain, r#type, protocol.unwrap_or(Protocol(0)));
    Socket(Operation::new(sq, fd::Kind::File, args))
}

new_flag!(
    /// Specification of the communication domain for a socket.
    pub struct Domain(i32) {
        /// Domain for IPv4 communication.
        IPV4 = libc::AF_INET,
        /// Domain for IPv6 communication.
        IPV6 = libc::AF_INET6,
        /// Domain for Unix socket communication.
        UNIX = libc::AF_UNIX,
        /// Domain for low-level packet interface.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PACKET = libc::AF_PACKET,
        /// Domain for low-level VSOCK interface.
        VSOCK = libc::AF_VSOCK,
    }

    /// Specification of communication semantics on a socket.
    pub struct Type(u32) {
        /// Provides sequenced, reliable, two-way, connection-based byte
        /// streams.
        ///
        /// Used for protocols such as TCP.
        STREAM = libc::SOCK_STREAM,
        /// Supports datagrams (connectionless, unreliable messages of a fixed
        /// maximum length).
        ///
        /// Used for protocols such as UDP.
        DGRAM = libc::SOCK_DGRAM,
        /// Raw network protocol access.
        RAW = libc::SOCK_RAW,
        /// Provides a reliable datagram layer that does not guarantee ordering.
        RDM = libc::SOCK_RDM,
        /// Provides a sequenced, reliable, two-way connection-based data
        /// transmission path for datagrams of fixed maximum length.
        SEQPACKET = libc::SOCK_SEQPACKET,
        /// Datagram Congestion Control Protocol socket.
        ///
        /// Used for the DCCP protocol.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DCCP = libc::SOCK_DCCP,
    }

    /// Specification of communication protocol.
    pub struct Protocol(u32) {
        /// Internet Control Message Protocol IPv4.
        ICMPV4 = libc::IPPROTO_ICMP,
        /// Internet Control Message Protocol IPv6.
        ICMPV6 = libc::IPPROTO_ICMPV6,
        /// Transmission Control Protocol.
        TCP = libc::IPPROTO_TCP,
        /// User Datagram Protocol.
        UDP = libc::IPPROTO_UDP,
        /// Datagram Congestion Control Protocol.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DCCP = libc::IPPROTO_DCCP,
        /// Stream Control Transport Protocol.
        SCTP = libc::IPPROTO_SCTP,
        /// UDP-Lite.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        UDPLITE = libc::IPPROTO_UDPLITE,
        /// Raw IP packets.
        RAW = libc::IPPROTO_RAW,
        /// Multipath TCP connection.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MPTCP = libc::IPPROTO_MPTCP,
    }
);

impl Domain {
    /// Returns the correct domain for `address`.
    pub fn for_address<A: SocketAddress>(address: &A) -> Domain {
        address.domain()
    }
}

operation!(
    /// [`Future`] behind [`socket`].
    ///
    /// If you're looking for a socket type, there is none, see [`AsyncFd`].
    pub struct Socket(sys::net::SocketOp) -> io::Result<AsyncFd>;
);

#[derive(Copy, Clone, Debug)]
pub(crate) enum Name {
    Local,
    Peer,
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum SendCall {
    Normal,
    ZeroCopy,
}

new_flag!(
    /// Flags in calls to recv.
    ///
    /// See [`AsyncFd::recv`], [`AsyncFd::multishot_recv`],
    /// [`AsyncFd::recv_vectored`], [`AsyncFd::recv_from`] and
    /// [`AsyncFd::recv_from_vectored`].
    pub struct RecvFlag(u32) impl BitOr {
        /// Set the close-on-exec flag for the file descriptor received via a
        /// UNIX domain file descriptor using the `SCM_RIGHTS` operation.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        CMSG_CLOEXEC = libc::MSG_CMSG_CLOEXEC,
        /// This flag specifies that queued errors should be received from the
        /// socket error queue.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ERR_QUEUE = libc::MSG_ERRQUEUE,
        /// Requests receipt of out-of-band data that would not be received in
        /// the normal data stream.
        OOB = libc::MSG_OOB,
        /// Receive data without removing that data from the queue.
        PEEK = libc::MSG_PEEK,
        // NOTE: `MSG_TRUNC` breaks the logic used to initialise the buffers so
        // not including it here.
        /// This flag requests that the operation block until the full request
        /// is satisfied.
        WAIT_ALL = libc::MSG_WAITALL,
    }

    /// Flags in calls to send.
    ///
    /// See functions such as [`AsyncFd::send`], [`AsyncFd::send_vectored`] and
    /// [`AsyncFd::send_to`].
    pub struct SendFlag(u32) impl BitOr {
        /// Tell the link layer that forward progress happened: you got a
        /// successful reply from the other side.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        CONFIRM = libc::MSG_CONFIRM,
        /// Don't use a gateway to send out the packet, send to hosts only on
        /// directly connected networks.
        DONT_ROUTE = libc::MSG_DONTROUTE,
        /// Terminates a record.
        EOR = libc::MSG_EOR,
        /// The caller has more data to send.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MORE = libc::MSG_MORE,
        // NOTE: `MSG_NOSIGNAL` is always set.
        /// Sends out-of-band data.
        OOB = libc::MSG_OOB,
        /// Attempts TCP Fast Open.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        FAST_OPEN = libc::MSG_FASTOPEN,
    }

    /// Flags in calls to accept.
    ///
    /// See [`AsyncFd::accept4`] and [`AsyncFd::multishot_accept4`].
    pub struct AcceptFlag(u32) {
        // NOTE: we don't need SOCK_NONBLOCK and SOCK_CLOEXEC.
    }

    /// Socket level.
    ///
    /// See [`AsyncFd::socket_option`] and [`AsyncFd::set_socket_option`].
    pub struct Level(u32) {
        /// Options for all sockets.
        ///
        /// See [`SocketOpt`] for the options.
        ///
        /// This is also used for Unix options, see [`UnixOpt`].
        SOCKET = libc::SOL_SOCKET,
        /// IPv4.
        ///
        /// See [`IPv4Opt`] for the options.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        IPV4 = libc::SOL_IP,
        /// IPv6.
        ///
        /// See [`IPv6Opt`] for the options.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        IPV6 = libc::SOL_IPV6,
        /// Transmission Control Protocol.
        ///
        /// See [`TcpOpt`] for the options.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        TCP = libc::SOL_TCP,
        /// User Datagram Protocol.
        ///
        /// See [`UdpOpt`] for the options.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        UDP = libc::SOL_UDP,
    }

    /// Socket option.
    ///
    /// The actual options are defined on the types that can be converted into this
    /// type, e.g. [`SocketOpt`] or [`TcpOpt`].
    ///
    /// See [`AsyncFd::socket_option`] and [`AsyncFd::set_socket_option`].
    pub struct Opt(u32) {}

    /// Socket level option.
    ///
    /// See [`Opt`].
    #[doc = man_link!(socket(7))]
    pub struct SocketOpt(u32) {
        /// Returns a value indicating whether or not this socket has been
        /// marked to accept connections with `listen(2)`.
        ACCEPT_CONN = libc::SO_ACCEPTCONN,
        /// Attach a classic BPF program to the socket for use as a filter of
        /// incoming packets.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ATTACH_FILTER = libc::SO_ATTACH_FILTER,
        /// Attach an extended BPF program to the socket for use as a filter of
        /// incoming packets.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ATTACH_BPF = libc::SO_ATTACH_BPF,
        /// Set a classic BPF program which defines how packets are assigned to
        /// the sockets in the reuseport group.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ATTACH_REUSE_PORT_CBPF = libc::SO_ATTACH_REUSEPORT_CBPF,
        /// Set an extended BPF program which defines how packets are assigned
        /// to the sockets in the reuseport group.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ATTACH_REUSE_PORT_EBPF = libc::SO_ATTACH_REUSEPORT_EBPF,
        /// Bind to a particular device.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        BIND_TO_DEVICE = libc::SO_BINDTODEVICE,
        /// Broadcast flag.
        BROADCAST = libc::SO_BROADCAST,
        /// Enable socket debugging.
        DEBUG = libc::SO_DEBUG,
        /// Remove the classic extended BPF program.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DETACH_FILTER = libc::SO_DETACH_FILTER,
        /// Remove the classic or extended BPF program.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DETACH_BPF = libc::SO_DETACH_BPF,
        /// Domain.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DOMAIN = libc::SO_DOMAIN,
        /// Get and clear the pending socket error.
        ERROR = libc::SO_ERROR,
        /// Don't send via a gateway, send only to directly connected hosts.
        DONT_ROUTE = libc::SO_DONTROUTE,
        /// CPU affinity.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        INCOMING_CPU = libc::SO_INCOMING_CPU,
        /// Returns a system-level unique ID called NAPI ID that is associated
        /// with a RX queue on which the last packet associated with that socket
        /// is received.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        INCOMING_NAPI_ID = libc::SO_INCOMING_NAPI_ID,
        /// Enable sending of keep-alive messages on connection-oriented
        /// sockets.
        KEEP_ALIVE = libc::SO_KEEPALIVE,
        /// Linger option.
        LINGER = libc::SO_LINGER,
        /// When set, this option will prevent changing the filters associated
        /// with the socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        LOCK_FILTER = libc::SO_LOCK_FILTER,
        /// Set the mark for each packet sent through this socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MARK = libc::SO_MARK,
        /// Place out-of-band (OOB) data directly into the receive data stream.
        OOB_INLINE = libc::SO_OOBINLINE,
        /// Return the credentials of the peer process connected to this socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PEER_CRED = libc::SO_PEERCRED,
        /// Return the security context of the peer socket connected to this
        /// socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PEER_SEC = libc::SO_PEERSEC,
        /// Set the protocol-defined priority for all packets to be sent.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PRIORITY = libc::SO_PRIORITY,
        /// Retrieves the socket protocol.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PROTOCOL = libc::SO_PROTOCOL,
        /// Maximum receive buffer in bytes.
        RECV_BUF = libc::SO_RCVBUF,
        /// Same task as [`SocketOpt::RECV_BUF`], but the rmem_max limit can be
        /// overridden.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        RECV_BUF_FORCE = libc::SO_RCVBUFFORCE,
        /// Minimum number of bytes in the buffer until the socket layer will
        /// pass the data to the user on receiving.
        RECV_LOW_WATER = libc::SO_RCVLOWAT,
        /// Minimum number of bytes in the buffer until the socket layer will
        /// pass the data to the protocol.
        SEND_LOW_WATER = libc::SO_SNDLOWAT,
        /// Specify the receiving timeouts until reporting an error.
        RECV_TIMEOUT = libc::SO_RCVTIMEO,
        /// Specify the sending timeouts until reporting an error.
        SEND_TIMEOUT = libc::SO_SNDTIMEO,
        /// Allow reuse of local addresses.
        REUSE_ADDR = libc::SO_REUSEADDR,
        /// Allow multiple sockets to be bound to an identical socket address.
        REUSE_PORT = libc::SO_REUSEPORT,
        /// Maximum send buffer in bytes.
        SEND_BUF = libc::SO_SNDBUF,
        /// Same task as [`SocketOpt::SEND_BUF`], but the wmem_max limit can be
        /// overridden.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        SEND_BUF_FORCE = libc::SO_SNDBUFFORCE,
        /// Receiving of the `SO_TIMESTAMP` control message.
        TIMESTAMP = libc::SO_TIMESTAMP,
        /// Receiving of the `SO_TIMESTAMPNS` control message.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        TIMESTAMP_NS = libc::SO_TIMESTAMPNS,
        /// Type.
        TYPE = libc::SO_TYPE,
        /// Sets the approximate time in microseconds to busy poll on a blocking
        /// receive when there is no data.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        BUSY_POLL = libc::SO_BUSY_POLL,
    }

    /// IPv4 level option.
    ///
    /// See [`Opt`].
    #[doc = man_link!(ip(7))]
    pub struct IPv4Opt(u32) {
        /// Join a multicast group.
        ADD_MEMBERSHIP = libc::IP_ADD_MEMBERSHIP,
        /// Join a multicast group and allow receiving data only from a
        /// specified source.
        ADD_SOURCE_MEMBERSHIP = libc::IP_ADD_SOURCE_MEMBERSHIP,
        /// Do not reserve an ephemeral port when using `bind(2)` with a port
        /// number of 0.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        BIND_ADDRESS_NO_PORT = libc::IP_BIND_ADDRESS_NO_PORT,
        /// Stop receiving multicast data from a specific source in a given
        /// group.
        BLOCK_SOURCE = libc::IP_BLOCK_SOURCE,
        /// Leave a multicast group.
        DROP_MEMBERSHIP = libc::IP_DROP_MEMBERSHIP,
        /// Leave a source-specific group.
        DROP_SOURCE_MEMBERSHIP = libc::IP_DROP_SOURCE_MEMBERSHIP,
        /// Allow binding to an IP address that is nonlocal or does not (yet)
        /// exist.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        FREE_BIND = libc::IP_FREEBIND,
        /// If enabled, the user supplies an IP header in front of the user
        /// data.
        HDR_INCL = libc::IP_HDRINCL,
        /// Access to the advanced full-state filtering API.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MSFILTER = libc::IP_MSFILTER,
        /// MTU value.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MTU = libc::IP_MTU,
        /// Set or receive the Path MTU Discovery setting for a socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MTU_DISCOVER = libc::IP_MTU_DISCOVER,
        /// Delivery policy of multicast messages.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MULTICAST_ALL = libc::IP_MULTICAST_ALL,
        /// Set the local device for a multicast socket.
        MULTICAST_IF = libc::IP_MULTICAST_IF,
        /// Control whether the socket sees multicast packets that it has send
        /// itself.
        MULTICAST_LOOP = libc::IP_MULTICAST_LOOP,
        /// Set or read the time-to-live value of outgoing multicast packets for
        /// this socket.
        MULTICAST_TTL = libc::IP_MULTICAST_TTL,
        /// Control the reassembly of outgoing packets is disabled in the
        /// netfilter layer.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        NO_DEFRAG = libc::IP_NODEFRAG,
        /// IP options to be sent with every packet from this socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        OPTIONS = libc::IP_OPTIONS,
        /// Enables receiving of the SELinux security label of the peer socket
        /// in an ancillary message of type `SCM_SECURITY`.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PASS_SEC = libc::IP_PASSSEC,
        /// Collect information about this socket.
        PKT_INFO = libc::IP_PKTINFO,
        /// Enable extended reliable error message passing.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        RECV_ERR = libc::IP_RECVERR,
        /// Pass all incoming IP options to the user in a `IP_OPTIONS` control
        /// message.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        RECV_OPTS = libc::IP_RECVOPTS,
        /// Enables the `IP_ORIGDSTADDR` ancillary message in `recvmsg(2)`.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        RECV_ORIG_DST_ADDR = libc::IP_RECVORIGDSTADDR,
        /// Enable passing of `IP_TOS` in ancillary message with incoming
        /// packets.
        RECV_TOS = libc::IP_RECVTOS,
        /// Enable passing of `IP_TTL` in ancillary message with incoming
        /// packets.
        RECV_TTL = libc::IP_RECVTTL,
        /// Identical to [`IPv4Opt::RECV_OPTS`], but returns raw unprocessed
        /// options with timestamp and route record options not filled in for
        /// this hop.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        RET_OPTS = libc::IP_RETOPTS,
        /// Pass all to-be forwarded packets with the IP Router Alert option set
        /// to this socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ROUTER_ALERT = libc::IP_ROUTER_ALERT,
        /// Type-Of-Service (TOS) field that is sent with every IP packet
        /// originating from this socket.
        TOS = libc::IP_TOS,
        /// Enable transparent proxying on this socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        TRANSPARENT = libc::IP_TRANSPARENT,
        /// Current time-to-live field that is used in every packet sent from
        /// this socket.
        TTL = libc::IP_TTL,
        /// Unblock previously blocked multicast source.
        UNBLOCK_SOURCE = libc::IP_UNBLOCK_SOURCE,
    }

    /// IPv6 level option.
    ///
    /// See [`Opt`].
    #[doc = man_link!(ipv6(7))]
    pub struct IPv6Opt(u32) {
        /// Turn an IPv6 socket into a socket of a different address family.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ADDR_FORM = libc::IPV6_ADDRFORM,
        /// Control membership in multicast groups.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ADD_MEMBERSHIP = libc::IPV6_ADD_MEMBERSHIP,
        /// Control membership in multicast groups.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DROP_MEMBERSHIP = libc::IPV6_DROP_MEMBERSHIP,
        /// MTU value.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MTU = libc::IPV6_MTU,
        /// Control path-MTU discovery on the socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        MTU_DISCOVER = libc::IPV6_MTU_DISCOVER,
        /// Set the multicast hop limit for the socket.
        MULTICAST_HOPS = libc::IPV6_MULTICAST_HOPS,
        /// Set the device for outgoing multicast packets on the socket.
        MULTICAST_IF = libc::IPV6_MULTICAST_IF,
        /// Control whether the socket sees multicast packets that it has send
        /// itself.
        MULTICAST_LOOP = libc::IPV6_MULTICAST_LOOP,
        /// Set delivery of the `IPV6_PKTINFO` control message on incoming
        /// datagrams.
        RECV_PKT_INFO = libc::IPV6_RECVPKTINFO,
        /// Set routing header.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        RT_HDR = libc::IPV6_RTHDR,
        /// Set authentication header.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        AUTH_HDR = libc::IPV6_AUTHHDR,
        /// Set destination options.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DST_OPTS = libc::IPV6_DSTOPTS,
        /// Set hop options.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        HOP_OPTS = libc::IPV6_HOPOPTS,
        /// Set flow id.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        FLOW_INFO = libc::IPV6_FLOWINFO,
        /// Set hop limit.
        HOP_LIMIT = libc::IPV6_HOPLIMIT,
        /// Control receiving of asynchronous error options.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        RECV_ERR = libc::IPV6_RECVERR,
        /// Pass forwarded packets containing a router alert hop-by-hop option
        /// to this socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        ROUTER_ALERT = libc::IPV6_ROUTER_ALERT,
        /// Set the unicast hop limit for the socket.
        UNICAST_HOPS = libc::IPV6_UNICAST_HOPS,
        /// Restrict to sending and receiving IPv6 packets only.
        V6_ONLY = libc::IPV6_V6ONLY,
    }

    /// Transmission Control Protocol level option.
    ///
    /// See [`Opt`].
    #[doc = man_link!(tcp(7))]
    pub struct TcpOpt(u32) {
        /// Set the TCP congestion control algorithm to be used.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        CONGESTION = libc::TCP_CONGESTION,
        /// Don't send out partial frames.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        CORK = libc::TCP_CORK,
        /// Allow a listener to be awakened only when data arrives on the
        /// socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        DEFER_ACCEPT = libc::TCP_DEFER_ACCEPT,
        /// Collect information about this socket.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        INFO = libc::TCP_INFO,
        /// The maximum number of keepalive probes TCP should send before
        /// dropping the connection.
        KEEP_CNT = libc::TCP_KEEPCNT,
        /// The time (in seconds) the connection needs to remain idle before TCP
        /// starts sending keepalive probes.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        KEEP_IDLE = libc::TCP_KEEPIDLE,
        /// The time (in seconds) between individual keepalive probes.
        KEEP_INTVL = libc::TCP_KEEPINTVL,
        /// The lifetime of orphaned FIN_WAIT2 state sockets.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        LINGER2 = libc::TCP_LINGER2,
        /// The maximum segment size for outgoing TCP packets.
        MAX_SEG = libc::TCP_MAXSEG,
        /// Disable the Nagle algorithm.
        NO_DELAY = libc::TCP_NODELAY,
        /// Enable quickack mode.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        QUICK_ACK = libc::TCP_QUICKACK,
        /// Set the number of SYN retransmits that TCP should send before
        /// aborting the attempt to connect.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        SYN_CNT = libc::TCP_SYNCNT,
        /// Maximum amount of time in milliseconds that transmitted data may
        /// remain unacknowledged, or buffered data may remain untransmitted,
        /// before TCP will forcibly close the connection.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        USER_TIMEOUT = libc::TCP_USER_TIMEOUT,
        /// Bound the size of the advertised window.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        WINDOW_CLAMP = libc::TCP_WINDOW_CLAMP,
        /// This option enables Fast Open on the listener socket.
        FASTOPEN = libc::TCP_FASTOPEN,
        /// This option enables an alternative way to perform Fast Open on the
        /// client side.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        FASTOPEN_CONNECT = libc::TCP_FASTOPEN_CONNECT,
    }

    /// User Datagram Protocol level option.
    ///
    /// See [`Opt`].
    #[doc = man_link!(udp(7))]
    pub struct UdpOpt(u32) {
        /// If this option is enabled, then all data output on this socket is
        /// accumulated into a single datagram that is transmitted when the
        /// option is disabled.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        CORK = libc::UDP_CORK,
        /// Enables UDP segmentation offload.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        SEGMENT = libc::UDP_SEGMENT,
        /// Enables UDP receive offload.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        GRO = libc::UDP_GRO,
    }

    /// Unix level option.
    ///
    /// Notes this uses [`Level::SOCKET`].
    ///
    /// See [`Opt`].
    #[doc = man_link!(unix(7))]
    pub struct UnixOpt(u32) {
        /// Enables receiving of the credentials of the sending process in an
        /// `SCM_CREDENTIALS` ancillary message in each subsequently received
        /// message.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PASS_CRED = libc::SO_PASSCRED,
        /// Enables receiving of the SELinux security label of the peer socket
        /// in an ancillary message of type `SCM_SECURITY`.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PASS_SEC = libc::SO_PASSSEC,
        /// Set the value of the "peek offset" for the `recv(2)` system call when
        /// used with `MSG_PEEK` flag.
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PEEK_OFF = libc::SO_PEEK_OFF,
        /// Returns the credentials of the peer process connected to this
        /// socket.
        ///
        /// *Read-only*
        #[cfg(any(target_os = "android", target_os = "linux"))]
        PEER_CRED = libc::SO_PEERCRED,
    }
);

macro_rules! impl_from {
    (
        $into: ident <- $( $from: ty ),+ $(,)?
    ) => {
        $(
        impl $from {
            /// Constant version of the `From` implementation.
            #[allow(dead_code)]
            const fn into_opt(self) -> $into {
                $into(self.0)
            }
        }

        impl From<$from> for $into {
            fn from(option: $from) -> $into {
                $into(option.0)
            }
        }
        )+
    };
}

impl_from!(Opt <- SocketOpt, IPv4Opt, IPv6Opt, TcpOpt, UdpOpt, UnixOpt);

/// Trait that defines the behaviour of socket addresses.
///
/// Unix uses different address types for different sockets, to support
/// all of them A10 uses this trait.
pub trait SocketAddress: private::SocketAddress + Sized {}

mod private {
    use std::mem::MaybeUninit;

    use super::Domain;

    pub trait SocketAddress {
        type Storage: Sized;

        /// Returns itself as storage.
        fn into_storage(self) -> Self::Storage;

        /// Returns a raw pointer and length to the storage.
        ///
        /// # Safety
        ///
        /// The pointer must be valid to read up to length bytes from.
        ///
        /// The implementation must ensure that the pointer is valid, i.e. not null
        /// and pointing to memory owned by the address. Furthermore it must ensure
        /// that the returned length is, in combination with the pointer, valid. In
        /// other words the memory the pointer and length are pointing to must be a
        /// valid memory address and owned by the address.
        unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t);

        /// Returns a mutable raw pointer and length to `storage`.
        ///
        /// # Safety
        ///
        /// Only initialised bytes may be written to the pointer returned.
        unsafe fn as_mut_ptr(
            storage: &mut MaybeUninit<Self::Storage>,
        ) -> (*mut libc::sockaddr, libc::socklen_t);

        /// Initialise the address from `storage`, to which at least `length`
        /// bytes have been written (by the kernel).
        ///
        /// # Safety
        ///
        /// Caller must ensure that at least `length` bytes have been written to
        /// `address`.
        unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self;

        /// Return the correct domain for the address.
        fn domain(&self) -> Domain;
    }
}

impl SocketAddress for SocketAddr {}

impl private::SocketAddress for SocketAddr {
    type Storage = libc::sockaddr_in6; // Fits both v4 and v6.

    fn into_storage(self) -> Self::Storage {
        match self {
            SocketAddr::V4(addr) => {
                // SAFETY: all zeroes is valid for `sockaddr_in6`.
                let mut storage = unsafe { mem::zeroed::<libc::sockaddr_in6>() };
                // SAFETY: `sockaddr_in` fits in `sockaddr_in6`.
                unsafe {
                    ptr::from_mut(&mut storage)
                        .cast::<libc::sockaddr_in>()
                        .write(addr.into_storage());
                }
                storage
            }
            SocketAddr::V6(addr) => addr.into_storage(),
        }
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = ptr::from_ref(storage).cast();
        let size = if <libc::c_int>::from(storage.sin6_family) == libc::AF_INET {
            size_of::<libc::sockaddr_in>()
        } else {
            size_of::<libc::sockaddr_in6>()
        };
        (ptr, size as libc::socklen_t)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            storage.as_mut_ptr().cast(),
            size_of::<Self::Storage>() as libc::socklen_t,
        )
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length as usize >= size_of::<libc::sa_family_t>());
        let family = unsafe { storage.as_ptr().cast::<libc::sa_family_t>().read() };
        if family == libc::AF_INET as libc::sa_family_t {
            let storage = unsafe { storage.as_ptr().cast::<libc::sockaddr_in>().read() };
            unsafe { SocketAddrV4::init(MaybeUninit::new(storage), length).into() }
        } else {
            unsafe { SocketAddrV6::init(storage, length).into() }
        }
    }

    fn domain(&self) -> Domain {
        match self {
            SocketAddr::V4(_) => Domain::IPV4,
            SocketAddr::V6(_) => Domain::IPV6,
        }
    }
}

impl SocketAddress for SocketAddrV4 {}

impl private::SocketAddress for SocketAddrV4 {
    type Storage = libc::sockaddr_in;

    fn into_storage(self) -> Self::Storage {
        libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: self.port().to_be(),
            sin_addr: libc::in_addr {
                s_addr: u32::from_ne_bytes(self.ip().octets()),
            },
            sin_zero: [0; 8],
            // A number of OS have `sin_len`, but we don't use it.
            #[cfg(not(any(target_os = "android", target_os = "linux")))]
            sin_len: 0,
        }
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = ptr::from_ref(storage).cast();
        (ptr, size_of::<Self::Storage>() as libc::socklen_t)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            storage.as_mut_ptr().cast(),
            size_of::<Self::Storage>() as libc::socklen_t,
        )
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self::Storage>() as libc::socklen_t);
        // SAFETY: caller must initialise the address.
        let storage = unsafe { storage.assume_init() };
        debug_assert!(<libc::c_int>::from(storage.sin_family) == libc::AF_INET);
        let ip = Ipv4Addr::from(storage.sin_addr.s_addr.to_ne_bytes());
        let port = u16::from_be(storage.sin_port);
        SocketAddrV4::new(ip, port)
    }

    fn domain(&self) -> Domain {
        Domain::IPV4
    }
}

impl SocketAddress for SocketAddrV6 {}

impl private::SocketAddress for SocketAddrV6 {
    type Storage = libc::sockaddr_in6;

    fn into_storage(self) -> Self::Storage {
        libc::sockaddr_in6 {
            sin6_family: libc::AF_INET6 as libc::sa_family_t,
            sin6_port: self.port().to_be(),
            sin6_flowinfo: self.flowinfo(),
            sin6_addr: libc::in6_addr {
                s6_addr: self.ip().octets(),
            },
            sin6_scope_id: self.scope_id(),
            // A number of OS have `sin6_len`, but we don't use it.
            #[cfg(not(any(target_os = "android", target_os = "linux")))]
            sin6_len: 0,
        }
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = ptr::from_ref(storage).cast();
        (ptr, size_of::<Self::Storage>() as libc::socklen_t)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            storage.as_mut_ptr().cast(),
            size_of::<Self::Storage>() as libc::socklen_t,
        )
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length == size_of::<Self::Storage>() as libc::socklen_t);
        // SAFETY: caller must initialise the address.
        let storage = unsafe { storage.assume_init() };
        debug_assert!(<libc::c_int>::from(storage.sin6_family) == libc::AF_INET6);
        let ip = Ipv6Addr::from(storage.sin6_addr.s6_addr);
        let port = u16::from_be(storage.sin6_port);
        SocketAddrV6::new(ip, port, storage.sin6_flowinfo, storage.sin6_scope_id)
    }

    fn domain(&self) -> Domain {
        Domain::IPV6
    }
}

impl SocketAddress for unix::net::SocketAddr {}

impl private::SocketAddress for unix::net::SocketAddr {
    type Storage = libc::sockaddr_un;

    fn into_storage(self) -> Self::Storage {
        let mut storage = libc::sockaddr_un {
            sun_family: libc::AF_UNIX as libc::sa_family_t,
            // SAFETY: all zero is valid for `sockaddr_un`.
            ..unsafe { mem::zeroed() }
        };
        // SAFETY: casting `[i8]` to `[u8]` is safe.
        let path = unsafe {
            slice::from_raw_parts_mut::<u8>(
                storage.sun_path.as_mut_ptr().cast(),
                storage.sun_path.len(),
            )
        };
        if let Some(pathname) = self.as_pathname() {
            let bytes = pathname.as_os_str().as_bytes();
            path[..bytes.len()].copy_from_slice(bytes);
        } else {
            #[cfg(any(target_os = "android", target_os = "linux"))]
            if let Some(bytes) = self.as_abstract_name() {
                path[1..][..bytes.len()].copy_from_slice(bytes);
            }

            // Unnamed address, we'll leave it all zero.
        }
        storage
    }

    unsafe fn as_ptr(storage: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        let ptr = ptr::from_ref(storage).cast();
        (ptr, size_of::<Self::Storage>() as libc::socklen_t)
    }

    unsafe fn as_mut_ptr(
        storage: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        (
            storage.as_mut_ptr().cast(),
            size_of::<Self::Storage>() as libc::socklen_t,
        )
    }

    unsafe fn init(storage: MaybeUninit<Self::Storage>, length: libc::socklen_t) -> Self {
        debug_assert!(length as usize >= size_of::<libc::sa_family_t>());
        let family = unsafe { ptr::addr_of!((*storage.as_ptr()).sun_family).read() };
        debug_assert!(family == libc::AF_UNIX as libc::sa_family_t);
        let path_ptr = unsafe { ptr::addr_of!((*storage.as_ptr()).sun_path) };
        let length = length as usize - (storage.as_ptr().addr() - path_ptr.addr());
        // SAFETY: the kernel ensures that at least `length` bytes are
        // initialised.
        let path = unsafe { slice::from_raw_parts::<u8>(path_ptr.cast(), length) };
        #[cfg(any(target_os = "android", target_os = "linux"))]
        if let Some(0) = path.first() {
            // NOTE: `from_abstract_name` adds a starting null byte.
            if let Ok(addr) = unix::net::SocketAddr::from_abstract_name(&path[1..]) {
                return addr;
            }
        }

        unix::net::SocketAddr::from_pathname(Path::new(OsStr::from_bytes(path)))
            // Fallback to an unnamed address.
            .unwrap_or_else(|_| unix::net::SocketAddr::from_pathname("").unwrap())
    }

    fn domain(&self) -> Domain {
        Domain::UNIX
    }
}

/// When [`accept`]ing connections we're not interested in the address.
///
/// This is not acceptable in calls to [`connect`].
///
/// [`accept`]: AsyncFd::accept
/// [`connect`]: AsyncFd::connect
#[derive(Copy, Clone, Debug)]
pub struct NoAddress;

impl SocketAddress for NoAddress {}

impl private::SocketAddress for NoAddress {
    type Storage = Self;

    fn into_storage(self) -> Self::Storage {
        NoAddress
    }

    unsafe fn as_ptr(_: &Self::Storage) -> (*const libc::sockaddr, libc::socklen_t) {
        // NOTE: this goes against the requirements of `cast_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn as_mut_ptr(
        _: &mut MaybeUninit<Self::Storage>,
    ) -> (*mut libc::sockaddr, libc::socklen_t) {
        // NOTE: this goes against the requirements of `as_mut_ptr`.
        (ptr::null_mut(), 0)
    }

    unsafe fn init(_: MaybeUninit<Self>, length: libc::socklen_t) -> Self {
        debug_assert!(length == 0);
        NoAddress
    }

    fn domain(&self) -> Domain {
        Domain(libc::AF_UNSPEC)
    }
}

/// Implement [`fmt::Debug`] for [`SocketAddress::Storage`].
///
/// [`SocketAddress::Storage`]: private::SocketAddress::Storage
pub(crate) struct AddressStorage<A>(pub(crate) A);

impl<A> fmt::Debug for AddressStorage<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Address").finish()
    }
}

/// Implement [`fmt::Debug`] for [`SockOpt::Storage`].
///
/// [`SockOpt::Storage`]: private::SockOpt::Storage
pub(crate) struct OptionStorage<T>(pub(crate) T);

impl<T> fmt::Debug for OptionStorage<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Option").finish()
    }
}
