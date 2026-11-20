//! Building and reading ICMP datagrams the way unprivileged ICMP sockets deliver them.
//! On macOS IPv4 data still carries the IPv4 header; IPv6 data starts at the ICMPv6 header and the
//! kernel fills in the ICMPv6 checksum. Pure functions, so every branch is testable.

use crate::Family;

pub const HEADER_LEN: usize = 8;
const IPV6_HEADER_LEN: usize = 40;
const PROTO_ICMPV6: u8 = 58;

mod v4 {
    pub const ECHO_REPLY: u8 = 0;
    pub const UNREACHABLE: u8 = 3;
    pub const ECHO_REQUEST: u8 = 8;
    pub const TIME_EXCEEDED: u8 = 11;
}

mod v6 {
    pub const UNREACHABLE: u8 = 1;
    pub const TIME_EXCEEDED: u8 = 3;
    pub const ECHO_REQUEST: u8 = 128;
    pub const ECHO_REPLY: u8 = 129;
}

/// What an incoming datagram turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parsed {
    EchoReply {
        identifier: u16,
        sequence: u16,
    },
    /// A router dropped the probe. The quoted echo header says which one.
    TimeExceeded {
        identifier: u16,
        sequence: u16,
    },
    Unreachable {
        code: u8,
        identifier: u16,
        sequence: u16,
    },
    Other {
        kind: u8,
    },
}

/// Builds an echo request. IPv4 carries its checksum; IPv6 leaves it to the kernel, which has
/// the pseudo-header the checksum covers.
pub fn echo_request(family: Family, identifier: u16, sequence: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(HEADER_LEN + payload.len());
    packet.push(match family {
        Family::V4 => v4::ECHO_REQUEST,
        Family::V6 => v6::ECHO_REQUEST,
    });
    packet.extend_from_slice(&[0, 0, 0]);
    packet.extend_from_slice(&identifier.to_be_bytes());
    packet.extend_from_slice(&sequence.to_be_bytes());
    packet.extend_from_slice(payload);
    if family == Family::V4 {
        let sum = checksum(&packet);
        packet[2..4].copy_from_slice(&sum.to_be_bytes());
    }
    packet
}

/// RFC 1071 ones' complement checksum. A packet whose checksum field is filled in sums to zero.
pub fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for pair in &mut chunks {
        sum = sum.wrapping_add(u32::from(pair[0]) << 8 | u32::from(pair[1]));
    }
    if let [last] = chunks.remainder() {
        sum = sum.wrapping_add(u32::from(*last) << 8);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn parse(bytes: &[u8], family: Family) -> Option<Parsed> {
    match family {
        Family::V4 => parse_v4(bytes),
        Family::V6 => parse_v6(bytes),
    }
}

fn read16(bytes: &[u8], index: usize) -> u16 {
    u16::from(bytes[index]) << 8 | u16::from(bytes[index + 1])
}

/// Length of the IPv4 header starting at `offset`, or None when the bytes there are not one.
fn ipv4_header_len(bytes: &[u8], offset: usize) -> Option<usize> {
    let first = *bytes.get(offset)?;
    if first >> 4 != 4 {
        return None;
    }
    let len = usize::from(first & 0x0f) * 4;
    (len >= 20).then_some(len)
}

/// 40 when an IPv6 header carrying ICMPv6 starts at `offset`.
fn ipv6_header_len(bytes: &[u8], offset: usize) -> Option<usize> {
    (bytes.len() > offset + 6 && bytes[offset] >> 4 == 6 && bytes[offset + 6] == PROTO_ICMPV6)
        .then_some(IPV6_HEADER_LEN)
}

/// ICMPv4: usually behind the IPv4 header (macOS), tolerated without one (Linux).
fn parse_v4(bytes: &[u8]) -> Option<Parsed> {
    let start = ipv4_header_len(bytes, 0).unwrap_or(0);
    if bytes.len() < start + HEADER_LEN {
        return None;
    }
    let kind = bytes[start];
    match kind {
        v4::ECHO_REPLY => Some(Parsed::EchoReply {
            identifier: read16(bytes, start + 4),
            sequence: read16(bytes, start + 6),
        }),
        v4::TIME_EXCEEDED | v4::UNREACHABLE => {
            // Error messages quote the offending IPv4 header plus the first 8 bytes of its
            // payload, which for our probes is the echo header with identifier and sequence.
            let inner_ip = start + HEADER_LEN;
            let inner_icmp = inner_ip + ipv4_header_len(bytes, inner_ip)?;
            if bytes.len() < inner_icmp + HEADER_LEN || bytes[inner_icmp] != v4::ECHO_REQUEST {
                return None;
            }
            let identifier = read16(bytes, inner_icmp + 4);
            let sequence = read16(bytes, inner_icmp + 6);
            Some(if kind == v4::TIME_EXCEEDED {
                Parsed::TimeExceeded {
                    identifier,
                    sequence,
                }
            } else {
                Parsed::Unreachable {
                    code: bytes[start + 1],
                    identifier,
                    sequence,
                }
            })
        }
        other => Some(Parsed::Other { kind: other }),
    }
}

/// ICMPv6: no IPv6 header in front (one is tolerated). Error messages quote the invoking packet,
/// its 40-byte IPv6 header first and then our echo request.
fn parse_v6(bytes: &[u8]) -> Option<Parsed> {
    let start = ipv6_header_len(bytes, 0).unwrap_or(0);
    if bytes.len() < start + HEADER_LEN {
        return None;
    }
    let kind = bytes[start];
    match kind {
        v6::ECHO_REPLY => Some(Parsed::EchoReply {
            identifier: read16(bytes, start + 4),
            sequence: read16(bytes, start + 6),
        }),
        v6::TIME_EXCEEDED | v6::UNREACHABLE => {
            let inner_ip = start + HEADER_LEN;
            let inner_icmp = inner_ip + ipv6_header_len(bytes, inner_ip)?;
            if bytes.len() < inner_icmp + HEADER_LEN || bytes[inner_icmp] != v6::ECHO_REQUEST {
                return None;
            }
            let identifier = read16(bytes, inner_icmp + 4);
            let sequence = read16(bytes, inner_icmp + 6);
            Some(if kind == v6::TIME_EXCEEDED {
                Parsed::TimeExceeded {
                    identifier,
                    sequence,
                }
            } else {
                Parsed::Unreachable {
                    code: bytes[start + 1],
                    identifier,
                    sequence,
                }
            })
        }
        other => Some(Parsed::Other { kind: other }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv4_header(protocol: u8) -> Vec<u8> {
        let mut header = vec![0u8; 20];
        header[0] = 0x45;
        header[9] = protocol;
        header
    }

    #[test]
    fn echo_request_checksums_to_zero() {
        let packet = echo_request(Family::V4, 0xbeef, 7, &[0x72; 32]);
        assert_eq!(packet.len(), 40);
        assert_eq!(packet[0], 8);
        assert_eq!(checksum(&packet), 0);
    }

    #[test]
    fn ipv6_request_leaves_the_checksum_to_the_kernel() {
        let packet = echo_request(Family::V6, 1, 2, &[0; 4]);
        assert_eq!(packet[0], 128);
        assert_eq!(&packet[2..4], &[0, 0]);
    }

    #[test]
    fn checksum_handles_odd_lengths() {
        assert_eq!(checksum(&[0x01]), !0x0100);
    }

    #[test]
    fn reads_an_echo_reply_behind_its_ip_header() {
        let mut datagram = ipv4_header(1);
        datagram.extend_from_slice(&[0, 0, 0, 0, 0x12, 0x34, 0x00, 0x09]);
        assert_eq!(
            parse(&datagram, Family::V4),
            Some(Parsed::EchoReply {
                identifier: 0x1234,
                sequence: 9
            })
        );
    }

    #[test]
    fn reads_an_echo_reply_without_an_ip_header() {
        let datagram = [0, 0, 0, 0, 0x12, 0x34, 0x00, 0x09];
        assert_eq!(
            parse(&datagram, Family::V4),
            Some(Parsed::EchoReply {
                identifier: 0x1234,
                sequence: 9
            })
        );
    }

    #[test]
    fn time_exceeded_names_the_probe_it_quotes() {
        let mut datagram = ipv4_header(1);
        datagram.extend_from_slice(&[11, 0, 0, 0, 0, 0, 0, 0]);
        datagram.extend_from_slice(&ipv4_header(1));
        datagram.extend_from_slice(&[8, 0, 0, 0, 0xab, 0xcd, 0x01, 0x02]);
        assert_eq!(
            parse(&datagram, Family::V4),
            Some(Parsed::TimeExceeded {
                identifier: 0xabcd,
                sequence: 0x0102
            })
        );
    }

    #[test]
    fn unreachable_keeps_its_code() {
        let mut datagram = ipv4_header(1);
        datagram.extend_from_slice(&[3, 1, 0, 0, 0, 0, 0, 0]);
        datagram.extend_from_slice(&ipv4_header(1));
        datagram.extend_from_slice(&[8, 0, 0, 0, 0, 1, 0, 2]);
        assert_eq!(
            parse(&datagram, Family::V4),
            Some(Parsed::Unreachable {
                code: 1,
                identifier: 1,
                sequence: 2
            })
        );
    }

    #[test]
    fn an_error_about_someone_elses_packet_is_dropped() {
        let mut datagram = ipv4_header(1);
        datagram.extend_from_slice(&[11, 0, 0, 0, 0, 0, 0, 0]);
        datagram.extend_from_slice(&ipv4_header(17));
        datagram.extend_from_slice(&[0x82, 0x9a, 0x82, 0x9b, 0, 8, 0, 0]);
        assert_eq!(parse(&datagram, Family::V4), None);
    }

    #[test]
    fn icmpv6_time_exceeded_quotes_behind_the_ipv6_header() {
        let mut datagram = vec![3, 0, 0, 0, 0, 0, 0, 0];
        let mut inner = vec![0u8; 40];
        inner[0] = 0x60;
        inner[6] = 58;
        datagram.extend_from_slice(&inner);
        datagram.extend_from_slice(&[128, 0, 0, 0, 0, 5, 0, 6]);
        assert_eq!(
            parse(&datagram, Family::V6),
            Some(Parsed::TimeExceeded {
                identifier: 5,
                sequence: 6
            })
        );
    }

    #[test]
    fn short_datagrams_are_ignored() {
        assert_eq!(parse(&[0, 0, 0], Family::V4), None);
        assert_eq!(parse(&[], Family::V6), None);
    }
}
