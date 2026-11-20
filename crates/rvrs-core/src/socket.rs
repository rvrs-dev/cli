//! An unprivileged ICMP datagram socket for one address family, or a raw one where that is refused.
//!
//! macOS hands every ICMP message for our probes to the socket, errors included. Linux delivers
//! echo replies on the socket and the errors of other routers (time exceeded, unreachable) on
//! its error queue, which `IP_RECVERR` turns on; it also rewrites the echo identifier to the
//! socket's own, so replies are matched by sequence alone there.
//!
//! Where Linux refuses the datagram socket and the process may open a raw one (root, or a
//! unikernel such as Unikraft's, which has no unprivileged ICMP), the raw socket is used instead.
//! It receives every ICMP message with its IPv4 header in front, errors included, and keeps our
//! identifier, so the parser and the identifier check do the filtering.

use std::io;
use std::net::{IpAddr, SocketAddr};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::packet::{self, Parsed};
use crate::Family;

pub struct Datagram {
    pub parsed: Parsed,
    pub source: IpAddr,
    pub received_at: Instant,
    /// The TTL (IPv4) or hop limit (IPv6) the message arrived with, when the system reports it.
    pub ttl: Option<u8>,
}

pub struct IcmpSocket {
    socket: Socket,
    family: Family,
    raw: bool,
}

impl IcmpSocket {
    pub fn open(family: Family) -> io::Result<Self> {
        let (domain, protocol) = match family {
            Family::V4 => (Domain::IPV4, Protocol::ICMPV4),
            Family::V6 => (Domain::IPV6, Protocol::ICMPV6),
        };
        let (socket, raw) = match Socket::new(domain, Type::DGRAM, Some(protocol)) {
            Ok(socket) => (socket, false),
            // The datagram socket's error is the one to report: it names the setting that
            // allows it, which is the fix for anyone who isn't root.
            Err(error) if cfg!(target_os = "linux") && is_refusal(&error) => {
                match Socket::new(domain, Type::from(libc::SOCK_RAW), Some(protocol)) {
                    Ok(socket) => (socket, true),
                    Err(_) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        socket.set_nonblocking(true)?;
        #[cfg(target_os = "linux")]
        if !raw {
            let (level, option) = match family {
                Family::V4 => (libc::SOL_IP, libc::IP_RECVERR),
                Family::V6 => (libc::SOL_IPV6, libc::IPV6_RECVERR),
            };
            set_int(&socket, level, option, 1)?;
        }
        // Ask for the TTL of what arrives. Ping shows it; a refusal only costs that column.
        let (level, option) = match family {
            Family::V4 => (libc::IPPROTO_IP, libc::IP_RECVTTL),
            Family::V6 => (libc::IPPROTO_IPV6, libc::IPV6_RECVHOPLIMIT),
        };
        let _ = set_int(&socket, level, option, 1);
        Ok(Self {
            socket,
            family,
            raw,
        })
    }

    /// True when the kernel replaces the echo identifier with its own: Linux's datagram socket.
    pub fn rewrites_identifier(&self) -> bool {
        cfg!(target_os = "linux") && !self.raw
    }

    /// Sends one packet with the given hop limit (IPv4 TTL or IPv6 hop limit).
    pub fn send(&self, packet: &[u8], destination: IpAddr, ttl: u8) -> io::Result<()> {
        let (level, option) = match self.family {
            Family::V4 => (libc::IPPROTO_IP, libc::IP_TTL),
            Family::V6 => (libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS),
        };
        set_int(&self.socket, level, option, i32::from(ttl))?;
        let address = SockAddr::from(SocketAddr::new(destination, 0));
        let sent = self.socket.send_to(packet, &address)?;
        if sent != packet.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "short send"));
        }
        Ok(())
    }

    /// Waits up to `timeout` for the next ICMP message about one of our probes.
    pub fn receive(&self, timeout: Duration) -> io::Result<Option<Datagram>> {
        if let Some(datagram) = self.read_once()? {
            return Ok(Some(datagram));
        }
        let mut descriptor = libc::pollfd {
            fd: self.socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: `descriptor` is one valid pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut descriptor, 1, millis) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(None)
            } else {
                Err(error)
            };
        }
        if ready == 0 {
            return Ok(None);
        }
        self.read_once()
    }

    fn read_once(&self) -> io::Result<Option<Datagram>> {
        #[cfg(target_os = "linux")]
        if !self.raw {
            if let Some(datagram) = self.read_error_queue() {
                return Ok(Some(datagram));
            }
        }
        loop {
            let mut buffer = [0u8; 2048];
            let mut control = [0u8; 256];
            // SAFETY: all-zero values are valid for both, and the pointers set below outlive the call.
            let mut name: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
            let mut iov = libc::iovec {
                iov_base: buffer.as_mut_ptr().cast(),
                iov_len: buffer.len(),
            };
            message.msg_name = (&mut name as *mut libc::sockaddr_storage).cast();
            message.msg_namelen = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            message.msg_iov = &mut iov;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = control.len() as _;
            // SAFETY: `message` points at live buffers of the lengths given.
            let count =
                unsafe { libc::recvmsg(self.socket.as_raw_fd(), &mut message, libc::MSG_DONTWAIT) };
            if count < 0 {
                let error = io::Error::last_os_error();
                return match error.kind() {
                    io::ErrorKind::WouldBlock => Ok(None),
                    io::ErrorKind::Interrupted => continue,
                    // Linux reports a queued ICMP error as the socket's error first. The details
                    // are on the error queue, which the next call reads.
                    _ if cfg!(target_os = "linux") => Ok(None),
                    _ => Err(error),
                };
            }
            let received_at = Instant::now();
            let bytes = &buffer[..count as usize];
            let Some(source) = storage_address(&name) else {
                continue;
            };
            let Some(parsed) = packet::parse(bytes, self.family) else {
                continue;
            };
            // A raw socket also sees the echo requests other programs send.
            if matches!(parsed, Parsed::Other { .. }) && self.raw {
                continue;
            }
            // macOS and raw sockets hand over the IPv4 header, which carries the TTL. Everywhere
            // else it comes as a control message.
            let header_ttl = (self.family == Family::V4 && bytes.len() > 8 && bytes[0] >> 4 == 4)
                .then(|| bytes[8]);
            // SAFETY: the kernel filled `control` up to msg_controllen.
            let ttl = header_ttl.or_else(|| unsafe { control_ttl(&message) });
            return Ok(Some(Datagram {
                parsed,
                source: unmap(source),
                received_at,
                ttl,
            }));
        }
    }

    /// One message from the Linux error queue: the probe we sent comes back as the payload and
    /// the router that answered sits behind the extended error in the control message.
    #[cfg(target_os = "linux")]
    fn read_error_queue(&self) -> Option<Datagram> {
        let mut payload = [0u8; 512];
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
                self.socket.as_raw_fd(),
                &mut message,
                libc::MSG_ERRQUEUE | libc::MSG_DONTWAIT,
            )
        };
        if count < packet::HEADER_LEN as isize {
            return None;
        }
        let received_at = Instant::now();
        let identifier = u16::from_be_bytes([payload[4], payload[5]]);
        let sequence = u16::from_be_bytes([payload[6], payload[7]]);

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
                    let (source, time_exceeded, unreachable) = if is_v4 {
                        let address =
                            std::ptr::read_unaligned(offender.cast::<libc::sockaddr_in>());
                        let ip = IpAddr::from(u32::from_be(address.sin_addr.s_addr).to_be_bytes());
                        (ip, error.ee_type == 11, error.ee_type == 3)
                    } else {
                        let address =
                            std::ptr::read_unaligned(offender.cast::<libc::sockaddr_in6>());
                        (
                            unmap(IpAddr::from(address.sin6_addr.s6_addr)),
                            error.ee_type == 3,
                            error.ee_type == 1,
                        )
                    };
                    let from_icmp = error.ee_origin == libc::SO_EE_ORIGIN_ICMP
                        || error.ee_origin == libc::SO_EE_ORIGIN_ICMP6;
                    if from_icmp && time_exceeded {
                        return Some(Datagram {
                            parsed: Parsed::TimeExceeded {
                                identifier,
                                sequence,
                            },
                            source,
                            received_at,
                            ttl: None,
                        });
                    }
                    if from_icmp && unreachable {
                        return Some(Datagram {
                            parsed: Parsed::Unreachable {
                                code: error.ee_code,
                                identifier,
                                sequence,
                            },
                            source,
                            received_at,
                            ttl: None,
                        });
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
        None
    }
}

/// The errors Linux gives when unprivileged ICMP sockets aren't allowed or don't exist.
fn is_refusal(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EACCES)
            | Some(libc::EPERM)
            | Some(libc::EPROTONOSUPPORT)
            | Some(libc::ESOCKTNOSUPPORT)
    )
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

/// The TTL or hop limit among a message's control data.
///
/// # Safety
/// `message` must describe a control buffer the kernel has filled.
unsafe fn control_ttl(message: &libc::msghdr) -> Option<u8> {
    let mut header = libc::CMSG_FIRSTHDR(message);
    while !header.is_null() {
        let level = (*header).cmsg_level;
        let kind = (*header).cmsg_type;
        let v4 = level == libc::IPPROTO_IP && (kind == libc::IP_RECVTTL || kind == libc::IP_TTL);
        let v6 = level == libc::IPPROTO_IPV6 && kind == libc::IPV6_HOPLIMIT;
        if v4 || v6 {
            let data = libc::CMSG_DATA(header);
            let length = (*header).cmsg_len as usize - (data as usize - header as usize);
            // One byte on macOS for IPv4, an int everywhere else.
            let value = if length >= std::mem::size_of::<libc::c_int>() {
                std::ptr::read_unaligned(data.cast::<libc::c_int>())
            } else {
                libc::c_int::from(*data)
            };
            return u8::try_from(value).ok();
        }
        header = libc::CMSG_NXTHDR(message, header);
    }
    None
}

fn storage_address(storage: &libc::sockaddr_storage) -> Option<IpAddr> {
    // SAFETY: the family field says which sockaddr the storage holds.
    unsafe {
        match libc::c_int::from(storage.ss_family) {
            libc::AF_INET => {
                let address =
                    &*(storage as *const libc::sockaddr_storage).cast::<libc::sockaddr_in>();
                Some(IpAddr::from(
                    u32::from_be(address.sin_addr.s_addr).to_be_bytes(),
                ))
            }
            libc::AF_INET6 => {
                let address =
                    &*(storage as *const libc::sockaddr_storage).cast::<libc::sockaddr_in6>();
                Some(IpAddr::from(address.sin6_addr.s6_addr))
            }
            _ => None,
        }
    }
}

/// An IPv4 address mapped into IPv6 reads as the IPv4 address it is.
fn unmap(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}
