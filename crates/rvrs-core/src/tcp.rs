//! TCP probing: a connect to a port, then a traceroute to it. What `traceroute -T -p` does,
//! without root.
//!
//! A probe is a TCP socket whose TTL is set before it connects, so its SYN dies at that hop.
//! The router's answer names the probe by the port it left from. macOS hands that ICMP message
//! to an ICMP datagram socket of ours. Linux puts it on the TCP socket's own error queue
//! (`IP_RECVERR`). The destination answers with a SYN-ACK, which is an open port, or with a
//! reset, which is a closed one. No answer from the destination is a filter on the way.
//!
//! The run starts with one connect at the system's own TTL, which says what the port does.
//! When it answers, the trace probes until it reaches the destination. When it doesn't, the
//! trace stops a few silent hops past the last router that answered, since that is where the
//! SYN stops.
//!
//! Probes go out a few at a time. Every SYN that reaches the destination is a connection it
//! has to set up, so the engine never has more than a handful on the wire and closes each at
//! once.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::engine::{Ending, Event, Options, ReplyKind};
use crate::Family;

/// What the destination did with a SYN to the port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortState {
    /// A SYN-ACK came back: something listens.
    Open,
    /// A reset came back: the host answers and nothing listens.
    Closed,
    /// Nothing came back from the destination: a filter drops the SYN, there or on the way.
    Filtered,
}

impl PortState {
    pub fn word(self) -> &'static str {
        match self {
            PortState::Open => "open",
            PortState::Closed => "closed",
            PortState::Filtered => "filtered",
        }
    }
}

/// The TCP side of a run, stored with it. A run without one probed with ICMP.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TcpProbe {
    pub port: u16,
    /// None while the first connect is on its way.
    #[serde(default)]
    pub state: Option<PortState>,
    /// How long the first connect took to be answered, when it was.
    #[serde(default)]
    pub connect_micros: Option<u64>,
    /// The maximum segment size the handshake settled on, for an open port. It is the path's
    /// MTU less the headers, as far as the two ends and the routers between them announced it.
    #[serde(default)]
    pub mss: Option<u16>,
}

impl TcpProbe {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            state: None,
            connect_micros: None,
            mss: None,
        }
    }
}

/// Probes on the wire at once. Each one that arrives costs the destination a connection.
const WINDOW: usize = 6;
/// Silent hops probed past the farthest router that answered, when the port is filtered.
const SILENT_HOPS: u8 = 5;
/// How long the router's ICMP message may trail the socket's own error on macOS.
const SOURCE_GRACE: Duration = Duration::from_millis(60);

/// Runs a TCP traceroute to `destination:port`. Blocks; run it on its own thread.
pub fn trace(
    destination: IpAddr,
    port: u16,
    options: Options,
    events: &Sender<Event>,
    stop: &Arc<AtomicBool>,
) {
    if let Err(message) = execute(destination, port, options.clamped(), events, stop) {
        let _ = events.send(Event::Failed(message));
    }
}

/// How a probe ended, as the socket or the router's message says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// The handshake completed.
    Connected,
    /// The destination reset it.
    Reset,
    /// A router dropped it: time exceeded.
    TimeExceeded,
    /// A router refused to carry it, with the ICMP code.
    Unreachable(u8),
}

/// How one probe ended: the answer, who gave it and when. None is no answer in time.
type Settled = Option<(Answer, IpAddr, Instant)>;

struct Probe {
    socket: Socket,
    ttl: u8,
    round: u8,
    sent_at: Instant,
    local_port: u16,
    /// The socket failed and the router's message hasn't been read yet (macOS).
    failed_at: Option<Instant>,
}

/// A router's ICMP message about one of our SYNs, read off the wire (macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Quoted {
    answer: Answer,
    source_port: u16,
    destination_port: u16,
}

fn execute(
    destination: IpAddr,
    port: u16,
    options: Options,
    events: &Sender<Event>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    let family = Family::of(destination);
    let listener = IcmpListener::open(family);

    // The connect: what does the port do with a SYN that is allowed to arrive?
    let state = match connect_once(
        destination,
        port,
        options.wait.min(Duration::from_secs(3)),
        stop,
    )? {
        Some((Answer::Connected, rtt, mss)) => (PortState::Open, Some(rtt), mss),
        Some((Answer::Reset, rtt, _)) => (PortState::Closed, Some(rtt), None),
        _ => (PortState::Filtered, None, None),
    };
    let _ = events.send(Event::Port {
        state: state.0,
        connect: state.1,
        mss: state.2,
    });
    if stop.load(Ordering::Relaxed) {
        let _ = events.send(Event::Finished(Ending::Stopped));
        return Ok(());
    }
    let answers = state.0 != PortState::Filtered;

    let mut probes: Vec<Probe> = Vec::new();
    let mut destination_ttl: Option<u8> = None;
    let mut farthest: u8 = 0;
    let mut slowest = Duration::ZERO;
    let mut round: u8 = 0;
    let mut ttl = options.first_ttl;
    let mut sending = true;
    let mut next_send = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = events.send(Event::Finished(Ending::Stopped));
            return Ok(());
        }
        let now = Instant::now();

        // Where this round ends: at the destination once it is known, a few silent hops past
        // the last router when the port is filtered, at the highest TTL otherwise.
        let limit = match destination_ttl {
            Some(limit) => limit,
            None if !answers || round > 0 => farthest
                .saturating_add(SILENT_HOPS)
                .clamp(options.first_ttl, options.max_ttl),
            None => options.max_ttl,
        };
        if sending && ttl > limit {
            round += 1;
            ttl = options.first_ttl;
            if round >= options.queries {
                sending = false;
            }
            continue;
        }
        if sending && now >= next_send && probes.len() < WINDOW {
            let socket = open_probe(destination, port, Some(ttl))
                .map_err(|error| format!("Couldn't send a probe ({error})."))?;
            let local_port = socket
                .local_addr()
                .ok()
                .and_then(|address| address.as_socket())
                .map_or(0, |address| address.port());
            probes.push(Probe {
                socket,
                ttl,
                round,
                sent_at: Instant::now(),
                local_port,
                failed_at: None,
            });
            let _ = events.send(Event::Sent { ttl, round });
            next_send = Instant::now() + options.spacing;
            ttl += 1;
            continue;
        }

        let wait = if slowest.is_zero() {
            options.wait
        } else {
            options.wait.min((slowest * 10).max(Duration::from_secs(1)))
        };

        let mut settled: Vec<(usize, Settled)> = Vec::new();

        // Routers' messages, where the system hands them to a socket of ours.
        while let Some((quoted, source, received_at)) = listener.as_ref().and_then(|l| l.read()) {
            if quoted.destination_port != port {
                continue;
            }
            if let Some(index) = probes
                .iter()
                .position(|probe| probe.local_port == quoted.source_port)
            {
                if !settled.iter().any(|(seen, _)| *seen == index) {
                    settled.push((index, Some((quoted.answer, source, received_at))));
                }
            }
        }

        poll(&probes, listener.as_ref(), Duration::from_millis(15));
        let polled_at = Instant::now();
        for (index, probe) in probes.iter_mut().enumerate() {
            if settled.iter().any(|(seen, _)| *seen == index) {
                continue;
            }
            // Linux: the router's message is on this socket's error queue.
            if let Some((answer, source)) = read_error_queue(&probe.socket) {
                settled.push((index, Some((answer, source, polled_at))));
                continue;
            }
            match probe.socket.take_error() {
                Ok(Some(error)) if error.raw_os_error() == Some(libc::ECONNREFUSED) => {
                    settled.push((index, Some((Answer::Reset, destination, polled_at))));
                }
                Ok(Some(_)) => {
                    // The SYN died on the way. Who dropped it arrives on the ICMP socket.
                    probe.failed_at.get_or_insert(polled_at);
                }
                _ if probe.failed_at.is_none() && probe.socket.peer_addr().is_ok() => {
                    settled.push((index, Some((Answer::Connected, destination, polled_at))));
                }
                _ => {}
            }
            let overdue = match probe.failed_at {
                Some(failed_at) => polled_at >= failed_at + SOURCE_GRACE,
                None => polled_at >= probe.sent_at + wait,
            };
            if overdue && !settled.iter().any(|(seen, _)| *seen == index) {
                settled.push((index, None));
            }
        }

        // Highest index first, so removing one leaves the others where they are.
        settled.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
        for (index, outcome) in settled {
            let probe = probes.remove(index);
            let past = destination_ttl.is_some_and(|limit| probe.ttl > limit);
            match outcome {
                None => {
                    if !past {
                        let _ = events.send(Event::Lost {
                            ttl: probe.ttl,
                            round: probe.round,
                        });
                    }
                }
                Some((answer, source, at)) => {
                    let kind = match answer {
                        Answer::Connected | Answer::Reset => ReplyKind::Destination,
                        Answer::TimeExceeded => ReplyKind::Intermediate,
                        Answer::Unreachable(code) => ReplyKind::Unreachable(code),
                    };
                    if kind != ReplyKind::Intermediate {
                        destination_ttl =
                            Some(destination_ttl.map_or(probe.ttl, |limit| limit.min(probe.ttl)));
                    }
                    if destination_ttl.is_some_and(|limit| probe.ttl > limit) {
                        continue;
                    }
                    farthest = farthest.max(probe.ttl);
                    // A failed socket's time is when it failed, which is when the answer came.
                    let answered_at = probe.failed_at.unwrap_or(at);
                    let rtt = answered_at.saturating_duration_since(probe.sent_at);
                    slowest = slowest.max(rtt);
                    let _ = events.send(Event::Reply {
                        ttl: probe.ttl,
                        round: probe.round,
                        from: source,
                        rtt,
                        kind,
                    });
                }
            }
        }

        if !sending && probes.is_empty() {
            let ending = destination_ttl.map_or(Ending::Exhausted, Ending::Reached);
            let _ = events.send(Event::Finished(ending));
            return Ok(());
        }
    }
}

/// One connect with the system's own TTL: the answer and how long it took, or None when
/// nothing came back in time.
fn connect_once(
    destination: IpAddr,
    port: u16,
    wait: Duration,
    stop: &Arc<AtomicBool>,
) -> Result<Option<(Answer, Duration, Option<u16>)>, String> {
    let socket = open_probe(destination, port, None)
        .map_err(|error| format!("Couldn't open a TCP socket ({error})."))?;
    let sent_at = Instant::now();
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let mut fd = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        // SAFETY: one live pollfd, and the count says so.
        unsafe { libc::poll(&mut fd, 1, 20) };
        let now = Instant::now();
        let _ = read_error_queue(&socket);
        match socket.take_error() {
            Ok(Some(error)) if error.raw_os_error() == Some(libc::ECONNREFUSED) => {
                return Ok(Some((Answer::Reset, now - sent_at, None)));
            }
            // Unreachable from here: no route, or a router said so. The trace shows where.
            Ok(Some(_)) => return Ok(None),
            _ if socket.peer_addr().is_ok() => {
                return Ok(Some((
                    Answer::Connected,
                    now - sent_at,
                    max_segment(&socket),
                )));
            }
            _ => {}
        }
        if now >= sent_at + wait {
            return Ok(None);
        }
    }
}

/// The maximum segment size of a connected socket.
fn max_segment(socket: &Socket) -> Option<u16> {
    let mut value: libc::c_int = 0;
    let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `value` and `length` are live and describe each other.
    let status = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::IPPROTO_TCP,
            libc::TCP_MAXSEG,
            (&mut value as *mut libc::c_int).cast(),
            &mut length,
        )
    };
    (status == 0).then(|| u16::try_from(value).ok()).flatten()
}

/// A TCP socket that has begun to connect, with its TTL set first when one is given. Closing
/// it resets the connection, so the destination holds nothing for us.
fn open_probe(destination: IpAddr, port: u16, ttl: Option<u8>) -> io::Result<Socket> {
    let family = Family::of(destination);
    let domain = match family {
        Family::V4 => Domain::IPV4,
        Family::V6 => Domain::IPV6,
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_nonblocking(true)?;
    let _ = socket.set_linger(Some(Duration::ZERO));
    if let Some(ttl) = ttl {
        let (level, option) = match family {
            Family::V4 => (libc::IPPROTO_IP, libc::IP_TTL),
            Family::V6 => (libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS),
        };
        set_int(&socket, level, option, i32::from(ttl))?;
    }
    #[cfg(target_os = "linux")]
    {
        let (level, option) = match family {
            Family::V4 => (libc::SOL_IP, libc::IP_RECVERR),
            Family::V6 => (libc::SOL_IPV6, libc::IPV6_RECVERR),
        };
        let _ = set_int(&socket, level, option, 1);
    }
    match socket.connect(&SockAddr::from(SocketAddr::new(destination, port))) {
        Ok(()) => Ok(socket),
        Err(error)
            if error.raw_os_error() == Some(libc::EINPROGRESS)
                || error.kind() == io::ErrorKind::WouldBlock =>
        {
            Ok(socket)
        }
        Err(error) => Err(error),
    }
}

/// Waits until a probe's socket or the ICMP socket has something, or the timeout passes.
fn poll(probes: &[Probe], listener: Option<&IcmpListener>, timeout: Duration) {
    let mut fds: Vec<libc::pollfd> = probes
        .iter()
        .filter(|probe| probe.failed_at.is_none())
        .map(|probe| libc::pollfd {
            fd: probe.socket.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        })
        .collect();
    if let Some(listener) = listener {
        fds.push(libc::pollfd {
            fd: listener.socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
    }
    if fds.is_empty() {
        std::thread::sleep(timeout.min(Duration::from_millis(5)));
        return;
    }
    // SAFETY: `fds` is a live slice of pollfd and the count is its length.
    unsafe {
        libc::poll(
            fds.as_mut_ptr(),
            fds.len() as libc::nfds_t,
            timeout.as_millis() as i32,
        )
    };
}

fn set_int(socket: &Socket, level: libc::c_int, option: libc::c_int, value: i32) -> io::Result<()> {
    // SAFETY: `value` is a live c_int and the length passed matches it.
    let status = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            level,
            option,
            (&value as *const i32).cast(),
            std::mem::size_of::<i32>() as libc::socklen_t,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Linux: the router's ICMP message about this socket's SYN, from its error queue.
#[cfg(target_os = "linux")]
fn read_error_queue(socket: &Socket) -> Option<(Answer, IpAddr)> {
    let mut payload = [0u8; 256];
    let mut control = [0u8; 512];
    let mut iov = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: payload.len(),
    };
    // SAFETY: an all-zero msghdr is valid, and every pointer set below outlives the call.
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = control.len() as _;
    let count = unsafe {
        libc::recvmsg(
            socket.as_raw_fd(),
            &mut message,
            libc::MSG_ERRQUEUE | libc::MSG_DONTWAIT,
        )
    };
    if count < 0 {
        return None;
    }
    // SAFETY: the kernel filled `control` up to msg_controllen; the CMSG macros walk it.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            let level = (*header).cmsg_level;
            let kind = (*header).cmsg_type;
            let is_v4 = level == libc::SOL_IP && kind == libc::IP_RECVERR;
            let is_v6 = level == libc::SOL_IPV6 && kind == libc::IPV6_RECVERR;
            if is_v4 || is_v6 {
                let data = libc::CMSG_DATA(header);
                let error = std::ptr::read_unaligned(data.cast::<libc::sock_extended_err>());
                let offender = data.add(std::mem::size_of::<libc::sock_extended_err>());
                let from_icmp = error.ee_origin == libc::SO_EE_ORIGIN_ICMP
                    || error.ee_origin == libc::SO_EE_ORIGIN_ICMP6;
                if from_icmp {
                    let (source, time_exceeded, unreachable) = if is_v4 {
                        let address =
                            std::ptr::read_unaligned(offender.cast::<libc::sockaddr_in>());
                        (
                            IpAddr::from(u32::from_be(address.sin_addr.s_addr).to_be_bytes()),
                            error.ee_type == 11,
                            error.ee_type == 3,
                        )
                    } else {
                        let address =
                            std::ptr::read_unaligned(offender.cast::<libc::sockaddr_in6>());
                        (
                            IpAddr::from(address.sin6_addr.s6_addr),
                            error.ee_type == 3,
                            error.ee_type == 1,
                        )
                    };
                    if time_exceeded {
                        return Some((Answer::TimeExceeded, source));
                    }
                    if unreachable {
                        return Some((Answer::Unreachable(error.ee_code), source));
                    }
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_error_queue(_socket: &Socket) -> Option<(Answer, IpAddr)> {
    None
}

/// An ICMP datagram socket that hears what routers say about our SYNs. macOS delivers those
/// messages to it. Linux doesn't, and has the error queue, so there it is never opened.
struct IcmpListener {
    socket: Socket,
    family: Family,
}

impl IcmpListener {
    fn open(family: Family) -> Option<Self> {
        if cfg!(target_os = "linux") {
            return None;
        }
        let (domain, protocol) = match family {
            Family::V4 => (Domain::IPV4, Protocol::ICMPV4),
            Family::V6 => (Domain::IPV6, Protocol::ICMPV6),
        };
        let socket = Socket::new(domain, Type::DGRAM, Some(protocol)).ok()?;
        socket.set_nonblocking(true).ok()?;
        Some(Self { socket, family })
    }

    fn read(&self) -> Option<(Quoted, IpAddr, Instant)> {
        loop {
            let mut buffer = [std::mem::MaybeUninit::<u8>::uninit(); 1500];
            let (count, address) = self.socket.recv_from(&mut buffer).ok()?;
            let received_at = Instant::now();
            // SAFETY: recv_from initialized the first `count` bytes.
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), count) };
            let Some(source) = address.as_socket().map(|address| address.ip()) else {
                continue;
            };
            if let Some(quoted) = quoted_tcp(bytes, self.family) {
                return Some((quoted, source, received_at));
            }
        }
    }
}

const PROTO_TCP: u8 = 6;

/// Reads an ICMP error that quotes a TCP segment: which answer it is and the ports of the
/// segment it is about. IPv4 messages come behind their IP header on macOS and without one
/// elsewhere; ICMPv6 never has one.
fn quoted_tcp(bytes: &[u8], family: Family) -> Option<Quoted> {
    match family {
        Family::V4 => {
            let start = match bytes.first()? >> 4 {
                4 => usize::from(bytes[0] & 0x0f) * 4,
                _ => 0,
            };
            let answer = match (*bytes.get(start)?, *bytes.get(start + 1)?) {
                (11, _) => Answer::TimeExceeded,
                (3, code) => Answer::Unreachable(code),
                _ => return None,
            };
            let inner = start + 8;
            if bytes.get(inner)? >> 4 != 4 || *bytes.get(inner + 9)? != PROTO_TCP {
                return None;
            }
            let tcp = inner + usize::from(bytes[inner] & 0x0f) * 4;
            ports(bytes, tcp, answer)
        }
        Family::V6 => {
            let answer = match (*bytes.first()?, *bytes.get(1)?) {
                (3, _) => Answer::TimeExceeded,
                (1, code) => Answer::Unreachable(code),
                _ => return None,
            };
            let inner = 8;
            if bytes.get(inner)? >> 4 != 6 || *bytes.get(inner + 6)? != PROTO_TCP {
                return None;
            }
            ports(bytes, inner + 40, answer)
        }
    }
}

fn ports(bytes: &[u8], tcp: usize, answer: Answer) -> Option<Quoted> {
    let header = bytes.get(tcp..tcp + 4)?;
    Some(Quoted {
        answer,
        source_port: u16::from_be_bytes([header[0], header[1]]),
        destination_port: u16::from_be_bytes([header[2], header[3]]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An IPv4 header of 20 bytes carrying `protocol`.
    fn ipv4(protocol: u8) -> Vec<u8> {
        let mut header = vec![0u8; 20];
        header[0] = 0x45;
        header[9] = protocol;
        header
    }

    fn tcp_ports(source: u16, destination: u16) -> Vec<u8> {
        let mut segment = Vec::new();
        segment.extend_from_slice(&source.to_be_bytes());
        segment.extend_from_slice(&destination.to_be_bytes());
        segment.extend_from_slice(&[0, 0, 0, 1]);
        segment
    }

    #[test]
    fn a_time_exceeded_names_the_syn_by_its_ports() {
        // As macOS delivers it: outer IP header, ICMP header, the quoted IP header and segment.
        let mut message = ipv4(1);
        message.extend_from_slice(&[11, 0, 0, 0, 0, 0, 0, 0]);
        message.extend_from_slice(&ipv4(PROTO_TCP));
        message.extend_from_slice(&tcp_ports(51000, 443));
        assert_eq!(
            quoted_tcp(&message, Family::V4),
            Some(Quoted {
                answer: Answer::TimeExceeded,
                source_port: 51000,
                destination_port: 443,
            })
        );
        // Without the outer header, as a raw socket's payload would arrive.
        assert_eq!(
            quoted_tcp(&message[20..], Family::V4).map(|quoted| quoted.source_port),
            Some(51000)
        );
    }

    #[test]
    fn an_unreachable_keeps_its_code() {
        let mut message = ipv4(1);
        message.extend_from_slice(&[3, 13, 0, 0, 0, 0, 0, 0]);
        message.extend_from_slice(&ipv4(PROTO_TCP));
        message.extend_from_slice(&tcp_ports(51000, 25));
        assert_eq!(
            quoted_tcp(&message, Family::V4).map(|quoted| quoted.answer),
            Some(Answer::Unreachable(13))
        );
    }

    #[test]
    fn a_message_about_a_ping_is_not_ours() {
        let mut message = ipv4(1);
        message.extend_from_slice(&[11, 0, 0, 0, 0, 0, 0, 0]);
        message.extend_from_slice(&ipv4(1));
        message.extend_from_slice(&[8, 0, 0, 0, 0, 1, 0, 1]);
        assert_eq!(quoted_tcp(&message, Family::V4), None);
        // An echo reply quotes nothing.
        let mut reply = ipv4(1);
        reply.extend_from_slice(&[0, 0, 0, 0, 0, 1, 0, 1]);
        assert_eq!(quoted_tcp(&reply, Family::V4), None);
        assert_eq!(quoted_tcp(&[], Family::V4), None);
    }

    #[test]
    fn icmpv6_quotes_behind_the_ipv6_header() {
        let mut message = vec![3, 0, 0, 0, 0, 0, 0, 0];
        let mut inner = vec![0u8; 40];
        inner[0] = 0x60;
        inner[6] = PROTO_TCP;
        message.extend_from_slice(&inner);
        message.extend_from_slice(&tcp_ports(40000, 443));
        assert_eq!(
            quoted_tcp(&message, Family::V6),
            Some(Quoted {
                answer: Answer::TimeExceeded,
                source_port: 40000,
                destination_port: 443,
            })
        );
    }

    #[test]
    fn the_stored_probe_reads_back_and_older_runs_have_none() {
        let probe = TcpProbe {
            port: 443,
            state: Some(PortState::Open),
            connect_micros: Some(11_300),
            mss: Some(1460),
        };
        let json = serde_json::to_string(&probe).unwrap();
        assert!(json.contains("\"state\":\"open\""));
        assert_eq!(serde_json::from_str::<TcpProbe>(&json).unwrap(), probe);
        // A probe stored before a field existed still loads.
        let old: TcpProbe = serde_json::from_str("{\"port\":22}").unwrap();
        assert_eq!(old, TcpProbe::new(22));
    }
}
