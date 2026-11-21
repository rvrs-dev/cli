//! The path MTU: the largest packet that crosses to a destination unfragmented.
//!
//! rvrs sends echo requests of a chosen size with fragmenting forbidden. Three things can come
//! back. An echo reply: a packet of that size crosses. A router's "fragmentation needed" (IPv4)
//! or "packet too big" (IPv6) with the size that would fit: the path is narrower there, and the
//! router says by how much. Nothing: either the target ignores large pings, or a router drops
//! what doesn't fit and stays silent about it, which is how connections hang once they send
//! full packets. rvrs tells the two apart by finding the size from which packets stop coming
//! back: when small ones cross and large ones vanish, the path has such a hole.
//!
//! It tests up to this device's own MTU, so 1500 on Ethernet. It doesn't look for jumbo paths.

use std::io;
use std::mem::MaybeUninit;
use std::net::{IpAddr, SocketAddr};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::packet;
use crate::Family;

/// How the search for the path MTU ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MtuFinding {
    /// A packet as large as this device's own MTU crossed.
    Full,
    /// A router said packets must be smaller.
    Limited,
    /// Packets above the size found vanish and no router says why.
    Silent,
    /// The target didn't answer a ping of any size, so there is nothing to tell.
    Unknown,
}

/// The path MTU of a run, stored with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMtu {
    pub finding: MtuFinding,
    /// The largest packet seen to cross, in bytes with its IP header. None when unknown.
    #[serde(default)]
    pub mtu: Option<u16>,
    /// The router that said packets must be smaller.
    #[serde(default)]
    pub limited_by: Option<IpAddr>,
    /// The largest size tried, which is this device's own MTU toward the destination.
    #[serde(default)]
    pub tried: Option<u16>,
}

/// What one probe of one size brought back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    Crossed,
    /// A router wants packets no larger than this.
    TooBig(u16, IpAddr),
    /// The size doesn't leave this device: its own MTU is smaller.
    TooBigHere,
    Nothing,
}

/// The smallest packet every path carries, so the smallest worth probing.
fn floor(family: Family) -> u16 {
    match family {
        Family::V4 => 576,
        Family::V6 => 1280,
    }
}

/// Sizes a device's own MTU is usually one of, tried from the top when 1500 won't leave.
const LOCAL_SIZES: [u16; 10] = [1500, 1492, 1480, 1472, 1454, 1440, 1420, 1400, 1380, 1280];

/// Finds the path MTU toward `destination`. `wait` is how long one probe may take.
pub fn discover(
    destination: IpAddr,
    wait: Duration,
    stop: &Arc<AtomicBool>,
) -> Result<PathMtu, String> {
    let family = Family::of(destination);
    let prober = Prober::open(destination, wait)
        .map_err(|error| format!("Couldn't open an ICMP socket ({error})."))?;
    search(family, |size| {
        if stop.load(Ordering::Relaxed) {
            return Ok(Answer::Nothing);
        }
        prober.probe(size, stop)
    })
}

/// The search itself, over anything that can say what a probe of a size brought back.
fn search(
    family: Family,
    mut probe: impl FnMut(u16) -> Result<Answer, String>,
) -> Result<PathMtu, String> {
    let unknown = |tried| PathMtu {
        finding: MtuFinding::Unknown,
        mtu: None,
        limited_by: None,
        tried,
    };

    // The largest size that leaves this device at all.
    let mut top = None;
    for size in LOCAL_SIZES
        .into_iter()
        .filter(|size| *size >= floor(family))
    {
        match probe(size)? {
            Answer::TooBigHere => continue,
            answer => {
                top = Some((size, answer));
                break;
            }
        }
    }
    let Some((tried, mut answer)) = top else {
        return Ok(unknown(None));
    };

    let mut size = tried;
    let mut limited_by = None;
    // Routers on the way narrow it down, each naming the size that fits its link.
    for _ in 0..8 {
        match answer {
            Answer::Crossed => {
                return Ok(PathMtu {
                    finding: if limited_by.is_some() {
                        MtuFinding::Limited
                    } else {
                        MtuFinding::Full
                    },
                    mtu: Some(size),
                    limited_by,
                    tried: Some(tried),
                });
            }
            Answer::TooBig(fits, router) if fits >= floor(family) && fits < size => {
                limited_by = Some(router);
                size = fits;
            }
            // A router that names no usable size leaves only the search below.
            Answer::TooBig(_, router) => {
                limited_by = Some(router);
                break;
            }
            Answer::TooBigHere | Answer::Nothing => break,
        }
        answer = probe(size)?;
    }

    // Silence at `size`. Does anything cross? If the smallest doesn't, the target ignores pings.
    let mut low = floor(family);
    if probe(low)? != Answer::Crossed {
        return Ok(unknown(Some(tried)));
    }
    let mut high = size;
    // Down to one byte: a hole's edge is a link's MTU, and that is the number worth having.
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        match probe(middle)? {
            Answer::Crossed => low = middle,
            Answer::TooBig(fits, router) if fits >= low && fits < middle => {
                limited_by = Some(router);
                high = fits + 1;
                if probe(fits)? == Answer::Crossed {
                    low = fits;
                }
            }
            _ => high = middle,
        }
    }
    Ok(PathMtu {
        finding: if limited_by.is_some() {
            MtuFinding::Limited
        } else {
            MtuFinding::Silent
        },
        mtu: Some(low),
        limited_by,
        tried: Some(tried),
    })
}

struct Prober {
    socket: Socket,
    destination: IpAddr,
    family: Family,
    wait: Duration,
    identifier: u16,
    sequence: std::cell::Cell<u16>,
}

impl Prober {
    fn open(destination: IpAddr, wait: Duration) -> io::Result<Self> {
        let family = Family::of(destination);
        let (domain, protocol) = match family {
            Family::V4 => (Domain::IPV4, Protocol::ICMPV4),
            Family::V6 => (Domain::IPV6, Protocol::ICMPV6),
        };
        let socket = Socket::new(domain, Type::DGRAM, Some(protocol))?;
        socket.set_nonblocking(true)?;
        forbid_fragments(&socket, family)?;
        Ok(Self {
            socket,
            destination,
            family,
            wait,
            identifier: (std::process::id() as u16).max(1),
            sequence: std::cell::Cell::new(1),
        })
    }

    /// Sends one echo request whose IP packet is `size` bytes and waits for what comes back.
    /// A size that gets no answer is tried a second time before it counts as nothing.
    fn probe(&self, size: u16, stop: &Arc<AtomicBool>) -> Result<Answer, String> {
        let mut answer = Answer::Nothing;
        for _ in 0..2 {
            answer = self.probe_once(size, stop)?;
            if answer != Answer::Nothing {
                break;
            }
        }
        Ok(answer)
    }

    fn probe_once(&self, size: u16, stop: &Arc<AtomicBool>) -> Result<Answer, String> {
        let headers: u16 = match self.family {
            Family::V4 => 28,
            Family::V6 => 48,
        };
        let sequence = self.sequence.get();
        self.sequence.set(sequence.wrapping_add(1).max(1));
        let payload = vec![0x72u8; usize::from(size.saturating_sub(headers))];
        let request = packet::echo_request(self.family, self.identifier, sequence, &payload);
        let address = SockAddr::from(SocketAddr::new(self.destination, 0));
        match self.socket.send_to(&request, &address) {
            Ok(_) => {}
            Err(error) if error.raw_os_error() == Some(libc::EMSGSIZE) => {
                // Linux says so once a router's answer is cached, and names the size.
                return Ok(match read_error_queue(&self.socket) {
                    Some((fits, router)) => Answer::TooBig(fits, router),
                    None => Answer::TooBigHere,
                });
            }
            Err(error) => return Err(format!("Couldn't send a probe ({error}).")),
        }
        let deadline = Instant::now() + self.wait;
        loop {
            if stop.load(Ordering::Relaxed) {
                return Ok(Answer::Nothing);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(Answer::Nothing);
            }
            let mut fd = libc::pollfd {
                fd: self.socket.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let left = (deadline - now).as_millis().min(50) as i32;
            // SAFETY: one live pollfd, and the count says so.
            unsafe { libc::poll(&mut fd, 1, left) };
            if let Some((fits, router)) = read_error_queue(&self.socket) {
                return Ok(Answer::TooBig(fits, router));
            }
            let mut buffer = [MaybeUninit::<u8>::uninit(); 2048];
            let Ok((count, from)) = self.socket.recv_from(&mut buffer) else {
                continue;
            };
            // SAFETY: recv_from initialized the first `count` bytes.
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), count) };
            let Some(source) = from.as_socket().map(|address| address.ip()) else {
                continue;
            };
            match read_message(bytes, self.family) {
                Some(Message::Reply(reply)) if reply == sequence => return Ok(Answer::Crossed),
                Some(Message::TooBig { fits, about }) if about == sequence => {
                    return Ok(Answer::TooBig(fits, source));
                }
                _ => {}
            }
        }
    }
}

/// An ICMP message as far as the search needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Message {
    /// An echo reply to this sequence number.
    Reply(u16),
    /// A router wants packets of at most `fits` bytes, about the probe with this sequence.
    TooBig { fits: u16, about: u16 },
}

fn read16(bytes: &[u8], index: usize) -> Option<u16> {
    Some(u16::from_be_bytes([
        *bytes.get(index)?,
        *bytes.get(index + 1)?,
    ]))
}

/// Reads an echo reply or a "too big" off the wire. IPv4 messages come behind their IP header
/// on macOS and without one on Linux. ICMPv6 never has one.
fn read_message(bytes: &[u8], family: Family) -> Option<Message> {
    match family {
        Family::V4 => {
            let start = match bytes.first()? >> 4 {
                4 => usize::from(bytes[0] & 0x0f) * 4,
                _ => 0,
            };
            match (*bytes.get(start)?, *bytes.get(start + 1)?) {
                (0, _) => Some(Message::Reply(read16(bytes, start + 6)?)),
                // Destination unreachable, fragmentation needed: the size is in the header.
                (3, 4) => {
                    let inner = start + 8;
                    let echo = inner + usize::from(bytes.get(inner)? & 0x0f) * 4;
                    Some(Message::TooBig {
                        fits: read16(bytes, start + 6)?,
                        about: read16(bytes, echo + 6)?,
                    })
                }
                _ => None,
            }
        }
        Family::V6 => match *bytes.first()? {
            129 => Some(Message::Reply(read16(bytes, 6)?)),
            // Packet too big: a 32-bit size, then the packet it is about.
            2 => {
                let fits = u32::from_be_bytes([
                    *bytes.get(4)?,
                    *bytes.get(5)?,
                    *bytes.get(6)?,
                    *bytes.get(7)?,
                ]);
                Some(Message::TooBig {
                    fits: u16::try_from(fits).unwrap_or(u16::MAX),
                    about: read16(bytes, 8 + 40 + 6)?,
                })
            }
            _ => None,
        },
    }
}

/// Sets don't-fragment on every packet the socket sends.
fn forbid_fragments(socket: &Socket, family: Family) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    let (level, option, value) = match family {
        // Probe: set DF and ignore what the system remembers about the path.
        Family::V4 => (
            libc::IPPROTO_IP,
            libc::IP_MTU_DISCOVER,
            libc::IP_PMTUDISC_PROBE,
        ),
        Family::V6 => (
            libc::IPPROTO_IPV6,
            libc::IPV6_MTU_DISCOVER,
            libc::IPV6_PMTUDISC_PROBE,
        ),
    };
    #[cfg(not(target_os = "linux"))]
    let (level, option, value) = match family {
        Family::V4 => (libc::IPPROTO_IP, libc::IP_DONTFRAG, 1),
        Family::V6 => (libc::IPPROTO_IPV6, libc::IPV6_DONTFRAG, 1),
    };
    set_int(socket, level, option, value)?;
    #[cfg(target_os = "linux")]
    {
        let (level, option) = match family {
            Family::V4 => (libc::SOL_IP, libc::IP_RECVERR),
            Family::V6 => (libc::SOL_IPV6, libc::IPV6_RECVERR),
        };
        let _ = set_int(socket, level, option, 1);
    }
    Ok(())
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

/// Linux: a router's "too big" from the socket's error queue, with the size that fits.
#[cfg(target_os = "linux")]
fn read_error_queue(socket: &Socket) -> Option<(u16, IpAddr)> {
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
                if error.ee_errno == libc::EMSGSIZE as u32 {
                    let offender = data.add(std::mem::size_of::<libc::sock_extended_err>());
                    let source = if is_v4 {
                        let address =
                            std::ptr::read_unaligned(offender.cast::<libc::sockaddr_in>());
                        IpAddr::from(u32::from_be(address.sin_addr.s_addr).to_be_bytes())
                    } else {
                        let address =
                            std::ptr::read_unaligned(offender.cast::<libc::sockaddr_in6>());
                        IpAddr::from(address.sin6_addr.s6_addr)
                    };
                    return Some((u16::try_from(error.ee_info).unwrap_or(u16::MAX), source));
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_error_queue(_socket: &Socket) -> Option<(u16, IpAddr)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv4(protocol: u8) -> Vec<u8> {
        let mut header = vec![0u8; 20];
        header[0] = 0x45;
        header[9] = protocol;
        header
    }

    #[test]
    fn an_echo_reply_names_its_sequence_with_or_without_the_ip_header() {
        let mut message = ipv4(1);
        message.extend_from_slice(&[0, 0, 0, 0, 0, 9, 0, 7]);
        assert_eq!(read_message(&message, Family::V4), Some(Message::Reply(7)));
        assert_eq!(
            read_message(&message[20..], Family::V4),
            Some(Message::Reply(7))
        );
    }

    #[test]
    fn fragmentation_needed_gives_the_size_that_fits_and_the_probe_it_is_about() {
        let mut message = ipv4(1);
        // Type 3 code 4, checksum, unused, next-hop MTU 1492.
        message.extend_from_slice(&[3, 4, 0, 0, 0, 0, 0x05, 0xd4]);
        message.extend_from_slice(&ipv4(1));
        message.extend_from_slice(&[8, 0, 0, 0, 0, 9, 0, 3]);
        assert_eq!(
            read_message(&message, Family::V4),
            Some(Message::TooBig {
                fits: 1492,
                about: 3
            })
        );
        // Another unreachable code is not about size.
        message[21] = 1;
        assert_eq!(read_message(&message, Family::V4), None);
    }

    #[test]
    fn icmpv6_packet_too_big_carries_a_32_bit_size() {
        let mut message = vec![2, 0, 0, 0, 0, 0, 0x05, 0x00];
        let mut inner = vec![0u8; 40];
        inner[0] = 0x60;
        message.extend_from_slice(&inner);
        message.extend_from_slice(&[128, 0, 0, 0, 0, 9, 0, 5]);
        assert_eq!(
            read_message(&message, Family::V6),
            Some(Message::TooBig {
                fits: 1280,
                about: 5
            })
        );
        assert_eq!(
            read_message(&[129, 0, 0, 0, 0, 9, 0, 5], Family::V6),
            Some(Message::Reply(5))
        );
    }

    const ROUTER: &str = "198.51.100.1";

    /// A path as a rule: what a probe of a size brings back.
    fn found(family: Family, path: impl Fn(u16) -> Answer) -> PathMtu {
        search(family, |size| Ok(path(size))).unwrap()
    }

    #[test]
    fn a_clean_path_carries_the_devices_own_mtu() {
        let result = found(Family::V4, |_| Answer::Crossed);
        assert_eq!(result.finding, MtuFinding::Full);
        assert_eq!((result.mtu, result.tried), (Some(1500), Some(1500)));
        // A device whose own link is smaller starts from there.
        let result = found(Family::V4, |size| {
            if size > 1420 {
                Answer::TooBigHere
            } else {
                Answer::Crossed
            }
        });
        assert_eq!(result.finding, MtuFinding::Full);
        assert_eq!((result.mtu, result.tried), (Some(1420), Some(1420)));
    }

    #[test]
    fn a_router_that_says_so_names_the_size_and_itself() {
        let router: IpAddr = ROUTER.parse().unwrap();
        let result = found(Family::V4, |size| {
            if size > 1492 {
                Answer::TooBig(1492, router)
            } else {
                Answer::Crossed
            }
        });
        assert_eq!(result.finding, MtuFinding::Limited);
        assert_eq!(result.mtu, Some(1492));
        assert_eq!(result.limited_by, Some(router));
        // Two narrower links in a row are followed down to the smaller.
        let result = found(Family::V4, |size| match size {
            size if size > 1492 => Answer::TooBig(1492, router),
            size if size > 1400 => Answer::TooBig(1400, router),
            _ => Answer::Crossed,
        });
        assert_eq!(result.mtu, Some(1400));
    }

    #[test]
    fn packets_that_vanish_are_found_to_the_byte_and_called_silent() {
        let result = found(Family::V4, |size| {
            if size > 1420 {
                Answer::Nothing
            } else {
                Answer::Crossed
            }
        });
        assert_eq!(result.finding, MtuFinding::Silent);
        assert_eq!(result.mtu, Some(1420));
        assert_eq!(result.limited_by, None);
    }

    #[test]
    fn a_target_that_answers_no_ping_leaves_the_mtu_unknown() {
        let result = found(Family::V4, |_| Answer::Nothing);
        assert_eq!(result.finding, MtuFinding::Unknown);
        assert_eq!(result.mtu, None);
        assert_eq!(result.tried, Some(1500));
        // IPv6 never probes below 1280.
        let mut smallest = u16::MAX;
        let _ = search(Family::V6, |size| {
            smallest = smallest.min(size);
            Ok(Answer::Nothing)
        });
        assert_eq!(smallest, 1280);
    }

    #[test]
    fn a_stored_result_reads_back_and_short_messages_are_ignored() {
        let found = PathMtu {
            finding: MtuFinding::Limited,
            mtu: Some(1492),
            limited_by: Some("198.51.100.1".parse().unwrap()),
            tried: Some(1500),
        };
        let json = serde_json::to_string(&found).unwrap();
        assert!(json.contains("\"finding\":\"limited\""));
        assert_eq!(serde_json::from_str::<PathMtu>(&json).unwrap(), found);
        let old: PathMtu = serde_json::from_str("{\"finding\":\"unknown\"}").unwrap();
        assert_eq!(old.mtu, None);
        assert_eq!(read_message(&[], Family::V4), None);
        assert_eq!(read_message(&[3, 4], Family::V4), None);
    }
}
