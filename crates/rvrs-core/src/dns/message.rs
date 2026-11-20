//! Builds DNS queries and parses answers: RFC 1035 wire format with EDNS(0) and the DO bit.
//! Pure functions over bytes, so every record type is testable.
//!
//! A record's value has two presentations. The app's is what the rvrs screens show: names
//! without the trailing dot, keys and signatures summarized. dig's is the zone-file form dig
//! prints, with every field in full.

use serde::{Deserialize, Serialize};

use crate::format;

/// The record types rvrs asks for and can print. Anything else prints as hex.
pub mod types {
    pub const A: u16 = 1;
    pub const NS: u16 = 2;
    pub const CNAME: u16 = 5;
    pub const SOA: u16 = 6;
    pub const PTR: u16 = 12;
    pub const MX: u16 = 15;
    pub const TXT: u16 = 16;
    pub const AAAA: u16 = 28;
    pub const SRV: u16 = 33;
    pub const OPT: u16 = 41;
    pub const DS: u16 = 43;
    pub const RRSIG: u16 = 46;
    pub const DNSKEY: u16 = 48;
    pub const SVCB: u16 = 64;
    pub const HTTPS: u16 = 65;
    pub const ANY: u16 = 255;
    pub const CAA: u16 = 257;
}

const NAMES: [(u16, &str); 17] = [
    (types::A, "A"),
    (types::NS, "NS"),
    (types::CNAME, "CNAME"),
    (types::SOA, "SOA"),
    (types::PTR, "PTR"),
    (types::MX, "MX"),
    (types::TXT, "TXT"),
    (types::AAAA, "AAAA"),
    (types::SRV, "SRV"),
    (types::OPT, "OPT"),
    (types::DS, "DS"),
    (types::RRSIG, "RRSIG"),
    (types::DNSKEY, "DNSKEY"),
    (types::SVCB, "SVCB"),
    (types::HTTPS, "HTTPS"),
    (types::ANY, "ANY"),
    (types::CAA, "CAA"),
];

/// "A", "HTTPS", or "TYPE99" for a type rvrs has no name for.
pub fn type_name(code: u16) -> String {
    NAMES
        .iter()
        .find(|(known, _)| *known == code)
        .map_or_else(|| format!("TYPE{code}"), |(_, name)| (*name).to_string())
}

/// The code for a type as people type it: "mx", "AAAA", "TYPE99".
pub fn type_code(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    if let Some((code, _)) = NAMES.iter().find(|(_, known)| *known == upper) {
        return Some(*code);
    }
    upper.strip_prefix("TYPE")?.parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Section {
    Answer,
    Authority,
    Additional,
}

/// A parsed resource record. `value` is the presentation form, which is all a screen needs;
/// `section` says where the server put it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub type_code: u16,
    pub ttl: u32,
    pub value: String,
    pub section: Section,
}

impl Record {
    pub fn type_name(&self) -> String {
        type_name(self.type_code)
    }
}

/// The OPT record of an answer, which carries no data of the zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edns {
    pub udp_size: u16,
    pub version: u8,
    pub dnssec_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub id: u16,
    pub flags: u16,
    pub rcode: u16,
    /// QUERY, ANSWER, AUTHORITY, ADDITIONAL as the header counts them.
    pub counts: [u16; 4],
    pub question: Option<(String, u16)>,
    pub records: Vec<Record>,
    pub edns: Option<Edns>,
}

impl Response {
    pub fn is_authoritative(&self) -> bool {
        self.flags & 0x0400 != 0
    }

    pub fn is_truncated(&self) -> bool {
        self.flags & 0x0200 != 0
    }

    pub fn recursion_desired(&self) -> bool {
        self.flags & 0x0100 != 0
    }

    pub fn recursion_available(&self) -> bool {
        self.flags & 0x0080 != 0
    }

    pub fn authentic_data(&self) -> bool {
        self.flags & 0x0020 != 0
    }

    pub fn checking_disabled(&self) -> bool {
        self.flags & 0x0010 != 0
    }

    pub fn in_section(&self, section: Section) -> impl Iterator<Item = &Record> {
        self.records
            .iter()
            .filter(move |record| record.section == section)
    }

    pub fn answers(&self) -> impl Iterator<Item = &Record> {
        self.in_section(Section::Answer)
    }

    pub fn authority(&self) -> impl Iterator<Item = &Record> {
        self.in_section(Section::Authority)
    }

    pub fn additional(&self) -> impl Iterator<Item = &Record> {
        self.in_section(Section::Additional)
    }
}

/// "NOERROR", "NXDOMAIN", "SERVFAIL": the exact status, as the industry spells it.
pub fn rcode_name(code: u16) -> String {
    match code {
        0 => "NOERROR".to_string(),
        1 => "FORMERR".to_string(),
        2 => "SERVFAIL".to_string(),
        3 => "NXDOMAIN".to_string(),
        4 => "NOTIMP".to_string(),
        5 => "REFUSED".to_string(),
        other => format!("RCODE{other}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageError {
    Truncated,
    BadName,
}

impl std::fmt::Display for MessageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            MessageError::Truncated => "The DNS answer was cut short.",
            MessageError::BadName => "The DNS answer had a malformed name.",
        })
    }
}

/// How a record's value is written out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// What the rvrs screens show.
    App,
    /// What dig prints.
    Dig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryFlags {
    /// RD: ask the server to resolve the name itself.
    pub recursive: bool,
    /// Send an OPT record that asks for 1232-byte answers.
    pub edns: bool,
    /// DO: ask for DNSSEC records, so signed zones show their signatures.
    pub dnssec_ok: bool,
    /// AD: ask the resolver to say whether it validated the answer, as dig does.
    pub authentic_data: bool,
}

/// A query for `name` and `type_code`.
pub fn query(id: u16, name: &str, type_code: u16, flags: QueryFlags) -> Vec<u8> {
    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&id.to_be_bytes());
    let header =
        if flags.recursive { 0x0100u16 } else { 0 } | if flags.authentic_data { 0x0020 } else { 0 };
    data.extend_from_slice(&header.to_be_bytes());
    data.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    data.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    data.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    data.extend_from_slice(&u16::from(flags.edns).to_be_bytes()); // ARCOUNT: the OPT record
    data.extend_from_slice(&encode_name(name));
    data.extend_from_slice(&type_code.to_be_bytes());
    data.extend_from_slice(&1u16.to_be_bytes()); // IN
    if flags.edns {
        // OPT: root name, type 41, UDP payload size, extended rcode, version, flags, no data.
        data.push(0);
        data.extend_from_slice(&types::OPT.to_be_bytes());
        data.extend_from_slice(&1232u16.to_be_bytes());
        data.extend_from_slice(&(if flags.dnssec_ok { 0x0000_8000u32 } else { 0 }).to_be_bytes());
        data.extend_from_slice(&0u16.to_be_bytes());
    }
    data
}

pub fn encode_name(name: &str) -> Vec<u8> {
    let mut data = Vec::new();
    for label in name.split('.').filter(|label| !label.is_empty()) {
        let bytes = &label.as_bytes()[..label.len().min(63)];
        data.push(bytes.len() as u8);
        data.extend_from_slice(bytes);
    }
    data.push(0);
    data
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Result<u8, MessageError> {
        let value = *self.bytes.get(self.cursor).ok_or(MessageError::Truncated)?;
        self.cursor += 1;
        Ok(value)
    }

    fn u16(&mut self) -> Result<u16, MessageError> {
        let slice = self
            .bytes
            .get(self.cursor..self.cursor + 2)
            .ok_or(MessageError::Truncated)?;
        self.cursor += 2;
        Ok(u16::from_be_bytes([slice[0], slice[1]]))
    }

    fn u32(&mut self) -> Result<u32, MessageError> {
        let slice = self
            .bytes
            .get(self.cursor..self.cursor + 4)
            .ok_or(MessageError::Truncated)?;
        self.cursor += 4;
        Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
    }

    /// A name with compression pointers followed. The cursor ends after the name in the stream.
    fn name(&mut self) -> Result<String, MessageError> {
        let mut labels: Vec<String> = Vec::new();
        let mut position = self.cursor;
        let mut jumped = false;
        let mut hops = 0;
        loop {
            let length = usize::from(*self.bytes.get(position).ok_or(MessageError::Truncated)?);
            if length == 0 {
                position += 1;
                if !jumped {
                    self.cursor = position;
                }
                break;
            }
            if length & 0xC0 == 0xC0 {
                let low = usize::from(
                    *self
                        .bytes
                        .get(position + 1)
                        .ok_or(MessageError::Truncated)?,
                );
                let pointer = (length & 0x3F) << 8 | low;
                if !jumped {
                    self.cursor = position + 2;
                }
                jumped = true;
                hops += 1;
                if hops >= 64 || pointer >= position {
                    return Err(MessageError::BadName);
                }
                position = pointer;
                continue;
            }
            position += 1;
            let label = self
                .bytes
                .get(position..position + length)
                .ok_or(MessageError::Truncated)?;
            labels.push(String::from_utf8_lossy(label).into_owned());
            position += length;
        }
        Ok(labels.join("."))
    }
}

pub fn parse(bytes: &[u8], presentation: Presentation) -> Result<Response, MessageError> {
    let mut reader = Reader { bytes, cursor: 0 };
    let id = reader.u16()?;
    let flags = reader.u16()?;
    let counts = [reader.u16()?, reader.u16()?, reader.u16()?, reader.u16()?];
    let mut response = Response {
        id,
        flags,
        rcode: flags & 0x000F,
        counts,
        question: None,
        records: Vec::new(),
        edns: None,
    };
    for _ in 0..counts[0] {
        let name = reader.name()?;
        let type_code = reader.u16()?;
        reader.u16()?;
        if response.question.is_none() {
            response.question = Some((name, type_code));
        }
    }
    let sections = [
        (counts[1], Section::Answer),
        (counts[2], Section::Authority),
        (counts[3], Section::Additional),
    ];
    for (count, section) in sections {
        for _ in 0..count {
            let name = reader.name()?;
            let type_code = reader.u16()?;
            let class = reader.u16()?; // the UDP size on OPT
            let ttl = reader.u32()?;
            let length = usize::from(reader.u16()?);
            let start = reader.cursor;
            if start + length > bytes.len() {
                return Err(MessageError::Truncated);
            }
            reader.cursor += length;
            if type_code == types::OPT {
                // The OPT TTL holds the extended rcode, the version and the flags.
                response.rcode |= ((ttl >> 24) as u16) << 4;
                response.edns = Some(Edns {
                    udp_size: class,
                    version: (ttl >> 16) as u8,
                    dnssec_ok: ttl & 0x8000 != 0,
                });
                continue;
            }
            let value = value_of(type_code, bytes, start, length, presentation);
            response.records.push(Record {
                name,
                type_code,
                ttl,
                value,
                section,
            });
        }
    }
    Ok(response)
}

fn dotted(name: &str, presentation: Presentation) -> String {
    match presentation {
        Presentation::App => name.to_string(),
        Presentation::Dig if name.is_empty() => ".".to_string(),
        Presentation::Dig => format!("{name}."),
    }
}

fn value_of(
    type_code: u16,
    bytes: &[u8],
    start: usize,
    length: usize,
    presentation: Presentation,
) -> String {
    let end = start + length;
    let mut reader = Reader {
        bytes,
        cursor: start,
    };
    let dig = presentation == Presentation::Dig;
    let data = &bytes[start..end];
    // Every read below stays inside the message, and a short record falls back to hex.
    let value = (|| -> Result<String, MessageError> {
        Ok(match type_code {
            types::A if length == 4 => data.iter().map(u8::to_string).collect::<Vec<_>>().join("."),
            types::AAAA if length == 16 => ipv6(data),
            types::NS | types::CNAME | types::PTR => dotted(&reader.name()?, presentation),
            types::MX => {
                let preference = reader.u16()?;
                format!("{preference} {}", dotted(&reader.name()?, presentation))
            }
            types::TXT => {
                let mut strings: Vec<String> = Vec::new();
                while reader.cursor < end {
                    let count = usize::from(reader.u8()?);
                    let slice = &bytes[reader.cursor..(reader.cursor + count).min(end)];
                    let text = String::from_utf8_lossy(slice);
                    strings.push(if dig {
                        text.replace('\\', "\\\\").replace('"', "\\\"")
                    } else {
                        text.into_owned()
                    });
                    reader.cursor += count;
                }
                strings
                    .iter()
                    .map(|text| format!("\"{text}\""))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            types::SOA => {
                let mname = dotted(&reader.name()?, presentation);
                let rname = dotted(&reader.name()?, presentation);
                let numbers = [
                    reader.u32()?,
                    reader.u32()?,
                    reader.u32()?,
                    reader.u32()?,
                    reader.u32()?,
                ];
                format!(
                    "{mname} {rname} {} {} {} {} {}",
                    numbers[0], numbers[1], numbers[2], numbers[3], numbers[4]
                )
            }
            types::SRV => {
                let (priority, weight, port) = (reader.u16()?, reader.u16()?, reader.u16()?);
                format!(
                    "{priority} {weight} {port} {}",
                    dotted(&reader.name()?, presentation)
                )
            }
            types::DS => {
                let (key_tag, algorithm, digest_type) = (reader.u16()?, reader.u8()?, reader.u8()?);
                let digest = hex(&bytes[reader.cursor.min(end)..end]);
                format!(
                    "{key_tag} {algorithm} {digest_type} {}",
                    if dig { digest.to_uppercase() } else { digest }
                )
            }
            types::DNSKEY => {
                let (flags, protocol, algorithm) = (reader.u16()?, reader.u8()?, reader.u8()?);
                let key = &bytes[reader.cursor.min(end)..end];
                if dig {
                    format!("{flags} {protocol} {algorithm} {}", base64(key))
                } else {
                    let role = if flags & 0x0001 != 0 { "KSK" } else { "ZSK" };
                    format!("{flags} 3 {algorithm} ({role}, {} bytes)", key.len())
                }
            }
            types::RRSIG => {
                let (covered, algorithm, labels, original_ttl) =
                    (reader.u16()?, reader.u8()?, reader.u8()?, reader.u32()?);
                let (expiration, inception, key_tag) =
                    (reader.u32()?, reader.u32()?, reader.u16()?);
                let signer = reader.name()?;
                if dig {
                    let stamp = |seconds: u32| {
                        format::iso8601(u64::from(seconds)).replace(['-', ':', 'T', 'Z'], "")
                    };
                    format!(
                        "{} {algorithm} {labels} {original_ttl} {} {} {key_tag} {} {}",
                        type_name(covered),
                        stamp(expiration),
                        stamp(inception),
                        dotted(&signer, presentation),
                        base64(&bytes[reader.cursor.min(end)..end])
                    )
                } else {
                    let day = |seconds: u32| format::iso8601(u64::from(seconds))[..10].to_string();
                    format!(
                        "{} {algorithm} {signer} {}\u{2013}{}",
                        type_name(covered),
                        day(inception),
                        day(expiration)
                    )
                }
            }
            types::CAA => {
                let flags = reader.u8()?;
                let tag_length = usize::from(reader.u8()?);
                let tag_end = (reader.cursor + tag_length).min(end);
                let tag = String::from_utf8_lossy(&bytes[reader.cursor..tag_end]).into_owned();
                let value = String::from_utf8_lossy(&bytes[tag_end..end]).into_owned();
                format!("{flags} {tag} \"{value}\"")
            }
            types::SVCB | types::HTTPS => {
                let priority = reader.u16()?;
                let target = reader.name()?;
                let mut parts = vec![
                    priority.to_string(),
                    if target.is_empty() {
                        ".".to_string()
                    } else {
                        dotted(&target, presentation)
                    },
                ];
                while reader.cursor + 4 <= end {
                    let key = reader.u16()?;
                    let value_length = usize::from(reader.u16()?);
                    let value_end = (reader.cursor + value_length).min(end);
                    parts.push(svc_param(key, &bytes[reader.cursor..value_end]));
                    reader.cursor = value_end;
                }
                parts.join(" ")
            }
            _ if dig => format!("\\# {length} {}", hex(data)),
            _ => hex(data),
        })
    })();
    value.unwrap_or_else(|_| hex(data))
}

/// Service binding parameters in the RFC 9460 presentation: alpn="h2,h3" ipv4hint=... ech=...
fn svc_param(key: u16, value: &[u8]) -> String {
    match key {
        0 => {
            let keys: Vec<String> = value
                .chunks_exact(2)
                .map(|pair| svc_param_name(u16::from_be_bytes([pair[0], pair[1]])))
                .collect();
            format!("mandatory={}", keys.join(","))
        }
        1 => {
            let mut cursor = 0;
            let mut protocols: Vec<String> = Vec::new();
            while cursor < value.len() {
                let count = usize::from(value[cursor]);
                cursor += 1;
                protocols.push(
                    String::from_utf8_lossy(
                        &value[cursor.min(value.len())..(cursor + count).min(value.len())],
                    )
                    .into_owned(),
                );
                cursor += count;
            }
            format!("alpn=\"{}\"", protocols.join(","))
        }
        2 => "no-default-alpn".to_string(),
        3 if value.len() == 2 => format!("port={}", u16::from_be_bytes([value[0], value[1]])),
        4 => {
            let hints: Vec<String> = value
                .chunks_exact(4)
                .map(|address| {
                    address
                        .iter()
                        .map(u8::to_string)
                        .collect::<Vec<_>>()
                        .join(".")
                })
                .collect();
            format!("ipv4hint={}", hints.join(","))
        }
        5 => format!("ech={}", base64(value)),
        6 => {
            let hints: Vec<String> = value.chunks_exact(16).map(ipv6).collect();
            format!("ipv6hint={}", hints.join(","))
        }
        other => format!("{}={}", svc_param_name(other), hex(value)),
    }
}

fn svc_param_name(key: u16) -> String {
    match key {
        0 => "mandatory".to_string(),
        1 => "alpn".to_string(),
        2 => "no-default-alpn".to_string(),
        3 => "port".to_string(),
        4 => "ipv4hint".to_string(),
        5 => "ech".to_string(),
        6 => "ipv6hint".to_string(),
        7 => "dohpath".to_string(),
        other => format!("key{other}"),
    }
}

/// An IPv6 address the way inet_ntop prints it: the longest run of zero groups becomes "::".
pub fn ipv6(bytes: &[u8]) -> String {
    let Ok(octets) = <[u8; 16]>::try_from(bytes) else {
        return hex(bytes);
    };
    std::net::Ipv6Addr::from(octets).to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let block = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(block >> 18) as usize & 63] as char);
        out.push(ALPHABET[(block >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(block >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[block as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The name to ask a PTR for: 4.3.2.1.in-addr.arpa, or the nibbles of an IPv6 address under
/// ip6.arpa.
pub fn reverse_name(address: std::net::IpAddr) -> String {
    match address {
        std::net::IpAddr::V4(v4) => {
            let [a, b, c, d] = v4.octets();
            format!("{d}.{c}.{b}.{a}.in-addr.arpa")
        }
        std::net::IpAddr::V6(v6) => {
            let nibbles: Vec<String> = v6
                .octets()
                .iter()
                .rev()
                .flat_map(|byte| [format!("{:x}", byte & 0x0f), format!("{:x}", byte >> 4)])
                .collect();
            format!("{}.ip6.arpa", nibbles.join("."))
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A response builder for tests: a header, one question and records given as raw RDATA.
    pub struct Builder {
        bytes: Vec<u8>,
        counts: [u16; 4],
    }

    impl Builder {
        pub fn new(id: u16, flags: u16, question: &str, type_code: u16) -> Self {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&id.to_be_bytes());
            bytes.extend_from_slice(&flags.to_be_bytes());
            bytes.extend_from_slice(&[0; 8]);
            bytes.extend_from_slice(&encode_name(question));
            bytes.extend_from_slice(&type_code.to_be_bytes());
            bytes.extend_from_slice(&1u16.to_be_bytes());
            Self {
                bytes,
                counts: [1, 0, 0, 0],
            }
        }

        pub fn record(
            mut self,
            section: Section,
            name: &str,
            type_code: u16,
            ttl: u32,
            rdata: &[u8],
        ) -> Self {
            let index = match section {
                Section::Answer => 1,
                Section::Authority => 2,
                Section::Additional => 3,
            };
            self.counts[index] += 1;
            self.bytes.extend_from_slice(&encode_name(name));
            self.bytes.extend_from_slice(&type_code.to_be_bytes());
            self.bytes.extend_from_slice(&1u16.to_be_bytes());
            self.bytes.extend_from_slice(&ttl.to_be_bytes());
            self.bytes
                .extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            self.bytes.extend_from_slice(rdata);
            self
        }

        pub fn build(mut self) -> Vec<u8> {
            for (index, count) in self.counts.iter().enumerate() {
                self.bytes[4 + index * 2..6 + index * 2].copy_from_slice(&count.to_be_bytes());
            }
            self.bytes
        }
    }

    pub fn name_data(name: &str) -> Vec<u8> {
        encode_name(name)
    }

    fn one(type_code: u16, rdata: &[u8], presentation: Presentation) -> String {
        let bytes = Builder::new(7, 0x8180, "example.com", type_code)
            .record(Section::Answer, "example.com", type_code, 300, rdata)
            .build();
        parse(&bytes, presentation).unwrap().records[0]
            .value
            .clone()
    }

    #[test]
    fn a_query_carries_the_question_and_an_opt_record_with_the_do_bit() {
        let flags = QueryFlags {
            recursive: true,
            edns: true,
            dnssec_ok: true,
            authentic_data: false,
        };
        let bytes = query(0x1234, "example.com.", types::AAAA, flags);
        assert_eq!(
            &bytes[..12],
            &[0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 1]
        );
        assert_eq!(
            &bytes[12..25],
            &[7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0]
        );
        assert_eq!(&bytes[25..29], &[0, 28, 0, 1]);
        assert_eq!(&bytes[29..], &[0, 0, 41, 0x04, 0xD0, 0, 0, 0x80, 0, 0, 0]);
        let plain = query(
            1,
            "example.com",
            types::A,
            QueryFlags {
                recursive: false,
                edns: false,
                dnssec_ok: false,
                authentic_data: false,
            },
        );
        assert_eq!(plain.len(), 29);
        assert_eq!(&plain[2..4], &[0, 0]);
    }

    #[test]
    fn the_header_says_who_answered_and_how() {
        let bytes = Builder::new(9, 0x8583, "nope.example.com", types::A).build();
        let response = parse(&bytes, Presentation::App).unwrap();
        assert_eq!(response.id, 9);
        assert!(
            response.is_authoritative()
                && response.recursion_desired()
                && response.recursion_available()
        );
        assert!(!response.is_truncated() && !response.authentic_data());
        assert_eq!(rcode_name(response.rcode), "NXDOMAIN");
        assert_eq!(
            response.question,
            Some(("nope.example.com".to_string(), types::A))
        );
        assert_eq!(response.counts, [1, 0, 0, 0]);
    }

    #[test]
    fn addresses_and_names_in_both_presentations() {
        assert_eq!(
            one(types::A, &[93, 184, 216, 34], Presentation::App),
            "93.184.216.34"
        );
        let v6 = [
            0x26, 0x06, 0x28, 0x00, 0x02, 0x20, 0, 1, 0x02, 0x48, 0x18, 0x93, 0x25, 0xc8, 0x19,
            0x46,
        ];
        assert_eq!(
            one(types::AAAA, &v6, Presentation::Dig),
            "2606:2800:220:1:248:1893:25c8:1946"
        );
        assert_eq!(
            one(
                types::NS,
                &name_data("a.iana-servers.net"),
                Presentation::App
            ),
            "a.iana-servers.net"
        );
        assert_eq!(
            one(
                types::NS,
                &name_data("a.iana-servers.net"),
                Presentation::Dig
            ),
            "a.iana-servers.net."
        );
        let mut mx = vec![0, 10];
        mx.extend(name_data("mail.example.com"));
        assert_eq!(
            one(types::MX, &mx, Presentation::App),
            "10 mail.example.com"
        );
        assert_eq!(
            one(types::MX, &mx, Presentation::Dig),
            "10 mail.example.com."
        );
    }

    #[test]
    fn text_soa_srv_and_caa() {
        let txt = [&[11u8][..], b"v=spf1 -all", &[2], b"ok"].concat();
        assert_eq!(
            one(types::TXT, &txt, Presentation::App),
            "\"v=spf1 -all\" \"ok\""
        );
        let mut soa = name_data("ns.example.com");
        soa.extend(name_data("admin.example.com"));
        for number in [2026111501u32, 7200, 3600, 1209600, 300] {
            soa.extend(number.to_be_bytes());
        }
        assert_eq!(
            one(types::SOA, &soa, Presentation::App),
            "ns.example.com admin.example.com 2026111501 7200 3600 1209600 300"
        );
        assert_eq!(
            one(types::SOA, &soa, Presentation::Dig),
            "ns.example.com. admin.example.com. 2026111501 7200 3600 1209600 300"
        );
        let mut srv = vec![0, 1, 0, 5, 0x01, 0xBB];
        srv.extend(name_data("svc.example.com"));
        assert_eq!(
            one(types::SRV, &srv, Presentation::App),
            "1 5 443 svc.example.com"
        );
        let caa = [&[0u8, 5][..], b"issue", b"letsencrypt.org"].concat();
        assert_eq!(
            one(types::CAA, &caa, Presentation::App),
            "0 issue \"letsencrypt.org\""
        );
    }

    #[test]
    fn dnssec_records_are_summarized_for_the_app_and_whole_for_dig() {
        let ds = [0x4Eu8, 0x20, 13, 2, 0xAB, 0xCD];
        assert_eq!(one(types::DS, &ds, Presentation::App), "20000 13 2 abcd");
        assert_eq!(one(types::DS, &ds, Presentation::Dig), "20000 13 2 ABCD");
        let key = [0x01u8, 0x01, 3, 13, 1, 2, 3];
        assert_eq!(
            one(types::DNSKEY, &key, Presentation::App),
            "257 3 13 (KSK, 3 bytes)"
        );
        assert_eq!(one(types::DNSKEY, &key, Presentation::Dig), "257 3 13 AQID");
        let mut rrsig = vec![0, 1, 13, 2];
        rrsig.extend(300u32.to_be_bytes());
        rrsig.extend(1_794_735_000u32.to_be_bytes());
        rrsig.extend(1_793_000_000u32.to_be_bytes());
        rrsig.extend([0x30, 0x39]);
        rrsig.extend(name_data("example.com"));
        rrsig.extend([1, 2, 3]);
        assert_eq!(
            one(types::RRSIG, &rrsig, Presentation::App),
            "A 13 example.com 2026-10-26\u{2013}2026-11-15"
        );
        assert_eq!(
            one(types::RRSIG, &rrsig, Presentation::Dig),
            "A 13 2 300 20261115093000 20261026073320 12345 example.com. AQID"
        );
    }

    #[test]
    fn service_bindings_show_their_parameters() {
        let mut https = vec![0, 1, 0];
        https.extend([0, 1, 0, 6, 2, b'h', b'3', 2, b'h', b'2']);
        https.extend([0, 4, 0, 8, 104, 16, 132, 229, 104, 16, 133, 229]);
        https.extend([0, 3, 0, 2, 0x20, 0xFB]);
        assert_eq!(
            one(types::HTTPS, &https, Presentation::App),
            "1 . alpn=\"h3,h2\" ipv4hint=104.16.132.229,104.16.133.229 port=8443"
        );
    }

    #[test]
    fn compression_pointers_are_followed_and_loops_are_refused() {
        // The answer's name points back at the question's.
        let mut bytes = Builder::new(1, 0x8180, "example.com", types::A).build();
        bytes[6..8].copy_from_slice(&1u16.to_be_bytes());
        bytes.extend([0xC0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 1, 2, 3, 4]);
        let response = parse(&bytes, Presentation::App).unwrap();
        assert_eq!(response.records[0].name, "example.com");
        assert_eq!(response.records[0].value, "1.2.3.4");
        // A pointer that points at itself.
        let mut looping = Builder::new(1, 0x8180, "example.com", types::A).build();
        looping[6..8].copy_from_slice(&1u16.to_be_bytes());
        let at = looping.len() as u8;
        looping.extend([0xC0, at, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 1, 2, 3, 4]);
        assert_eq!(
            parse(&looping, Presentation::App),
            Err(MessageError::BadName)
        );
        assert_eq!(
            parse(&bytes[..20], Presentation::App),
            Err(MessageError::Truncated)
        );
    }

    #[test]
    fn the_opt_record_is_read_and_left_out_of_the_records() {
        let bytes = Builder::new(1, 0x81A0, "example.com", types::A)
            .record(Section::Answer, "example.com", types::A, 60, &[1, 2, 3, 4])
            .build();
        let mut bytes = bytes;
        bytes[10..12].copy_from_slice(&1u16.to_be_bytes());
        bytes.extend([0, 0, 41, 0x04, 0xD0, 0, 0, 0x80, 0, 0, 0]);
        let response = parse(&bytes, Presentation::App).unwrap();
        assert_eq!(response.records.len(), 1);
        assert_eq!(
            response.edns,
            Some(Edns {
                udp_size: 1232,
                version: 0,
                dnssec_ok: true
            })
        );
        assert!(response.authentic_data());
    }

    #[test]
    fn an_unknown_type_prints_as_hex() {
        assert_eq!(one(99, &[0xDE, 0xAD], Presentation::App), "dead");
        assert_eq!(one(99, &[0xDE, 0xAD], Presentation::Dig), "\\# 2 dead");
        assert_eq!(type_name(99), "TYPE99");
        assert_eq!(type_code("type99"), Some(99));
        assert_eq!(type_code("https"), Some(65));
        assert_eq!(type_code("nope"), None);
    }

    #[test]
    fn reverse_names() {
        assert_eq!(
            reverse_name("1.2.3.4".parse().unwrap()),
            "4.3.2.1.in-addr.arpa"
        );
        assert!(reverse_name("2001:db8::1".parse().unwrap()).ends_with(".8.b.d.0.1.0.0.2.ip6.arpa"));
        assert_eq!(base64(b"ab"), "YWI=");
    }
}
