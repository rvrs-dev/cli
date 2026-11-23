//! Which addresses a node sends packets to.
//!
//! A node probes public addresses. Private and shared address space is refused unless the node's
//! owner allowed a range of it, for diagnostics on their own network. Loopback, link-local,
//! multicast, broadcast and reserved addresses are refused whatever was allowed: a probe to
//! `127.0.0.1` asks about the node itself, and `169.254.169.254` is where a cloud keeps a
//! machine's credentials.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The kind of address space an address is in, as far as a node cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    /// Routed on the internet.
    Public,
    /// 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 and fc00::/7.
    Private,
    /// 100.64.0.0/10, which a provider uses between its network and its customers'.
    Shared,
    Loopback,
    LinkLocal,
    Multicast,
    Broadcast,
    /// Set aside and never a host to probe: documentation ranges, 0.0.0.0/8, 240.0.0.0/4,
    /// unallocated IPv6 and the like.
    Reserved,
}

impl Space {
    pub fn of(address: IpAddr) -> Space {
        match address {
            IpAddr::V4(v4) => of_v4(v4),
            IpAddr::V6(v6) => of_v6(v6),
        }
    }

    /// Whether an owner can open this space with an allowed range.
    pub fn can_be_allowed(self) -> bool {
        matches!(self, Space::Private | Space::Shared)
    }

    /// "a private address", for a sentence about one address.
    pub fn one(self) -> &'static str {
        match self {
            Space::Public => "a public address",
            Space::Private => "a private address",
            Space::Shared => "an address in shared address space",
            Space::Loopback => "a loopback address",
            Space::LinkLocal => "a link-local address",
            Space::Multicast => "a multicast address",
            Space::Broadcast => "the broadcast address",
            Space::Reserved => "a reserved address",
        }
    }

    /// "private addresses", for a line about what a node doesn't probe.
    pub fn many(self) -> &'static str {
        match self {
            Space::Public => "public addresses",
            Space::Private => "private addresses",
            Space::Shared => "shared address space",
            Space::Loopback => "loopback addresses",
            Space::LinkLocal => "link-local addresses",
            Space::Multicast => "multicast addresses",
            Space::Broadcast => "the broadcast address",
            Space::Reserved => "reserved addresses",
        }
    }
}

fn of_v4(address: Ipv4Addr) -> Space {
    let [first, second, third, fourth] = address.octets();
    match (first, second, third) {
        (0, ..) => Space::Reserved,
        (10, ..) => Space::Private,
        (100, 64..=127, _) => Space::Shared,
        (127, ..) => Space::Loopback,
        (169, 254, _) => Space::LinkLocal,
        (172, 16..=31, _) => Space::Private,
        // IETF protocol assignments.
        (192, 0, 0) => Space::Reserved,
        // TEST-NET-1, -2 and -3.
        (192, 0, 2) | (198, 51, 100) | (203, 0, 113) => Space::Reserved,
        // The retired 6to4 relay anycast.
        (192, 88, 99) => Space::Reserved,
        (192, 168, _) => Space::Private,
        // Benchmarking.
        (198, 18..=19, _) => Space::Reserved,
        (224..=239, ..) => Space::Multicast,
        (255, 255, 255) if fourth == 255 => Space::Broadcast,
        (240..=255, ..) => Space::Reserved,
        _ => Space::Public,
    }
}

fn of_v6(address: Ipv6Addr) -> Space {
    let segments = address.segments();
    let embedded = |high: u16, low: u16| {
        Ipv4Addr::new(
            (high >> 8) as u8,
            (high & 0xff) as u8,
            (low >> 8) as u8,
            (low & 0xff) as u8,
        )
    };
    if address.is_loopback() {
        return Space::Loopback;
    }
    if address.is_unspecified() {
        return Space::Reserved;
    }
    // An IPv4 address written as IPv6 is the IPv4 address. A public one in this form is no
    // destination of its own, so it is never probed as IPv6.
    if let Some(mapped) = address.to_ipv4_mapped() {
        return match of_v4(mapped) {
            Space::Public => Space::Reserved,
            space => space,
        };
    }
    match segments[0] {
        // NAT64: the IPv4 address in the last 32 bits is where the packet ends up. A resolver
        // on an IPv6-only network makes these for public names, so a public one is probed.
        0x0064 if segments[1] == 0xff9b && segments[2..6] == [0, 0, 0, 0] => {
            of_v4(embedded(segments[6], segments[7]))
        }
        // NAT64 for local use, and the discard prefix.
        0x0064 | 0x0100 => Space::Reserved,
        // Teredo.
        0x2001 if segments[1] == 0 => Space::Reserved,
        // Documentation.
        0x2001 if segments[1] == 0x0db8 => Space::Reserved,
        0x3fff if segments[1] & 0xf000 == 0 => Space::Reserved,
        // 6to4: the IPv4 address in bits 16 to 48 is the router the packet goes to.
        0x2002 => of_v4(embedded(segments[1], segments[2])),
        first if first & 0xfe00 == 0xfc00 => Space::Private,
        first if first & 0xffc0 == 0xfe80 => Space::LinkLocal,
        first if first & 0xff00 == 0xff00 => Space::Multicast,
        // Global unicast is 2000::/3. The rest is not allocated.
        first if first & 0xe000 == 0x2000 => Space::Public,
        _ => Space::Reserved,
    }
}

/// A block of addresses in CIDR form. An address alone is a block of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefix {
    network: IpAddr,
    length: u8,
}

impl Prefix {
    pub fn parse(text: &str) -> Result<Prefix, String> {
        let text = text.trim();
        let wrong = || format!("{text} is not a prefix such as 192.168.0.0/16 or fd00::/8.");
        let (address, length) = match text.split_once('/') {
            Some((address, length)) => (address, Some(length)),
            None => (text, None),
        };
        let address: IpAddr = address.parse().map_err(|_| wrong())?;
        let most = match address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let length = match length {
            Some(length) => length
                .parse::<u8>()
                .ok()
                .filter(|length| *length <= most)
                .ok_or_else(wrong)?,
            None => most,
        };
        Ok(Prefix {
            network: masked(address, length),
            length,
        })
    }

    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.network, address) {
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => {
                masked(address, self.length) == self.network
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Prefix {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.network, self.length)
    }
}

fn masked(address: IpAddr, length: u8) -> IpAddr {
    match address {
        IpAddr::V4(v4) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(length)).unwrap_or(0);
            IpAddr::V4(Ipv4Addr::from(u32::from(v4) & mask))
        }
        IpAddr::V6(v6) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(length)).unwrap_or(0);
            IpAddr::V6(Ipv6Addr::from(u128::from(v6) & mask))
        }
    }
}

/// The ranges a node's owner allowed, for diagnostics on their own network. They are written
/// where the node is set up and never come from the API: no request, claim or event carries one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Allowed(Vec<Prefix>);

impl Allowed {
    /// Nothing allowed, which is how every node starts.
    pub fn none() -> Allowed {
        Allowed(Vec::new())
    }

    /// Prefixes separated by commas or spaces: `10.0.0.0/8, fd00::/8`.
    pub fn parse(text: &str) -> Result<Allowed, String> {
        text.split(|letter: char| letter == ',' || letter.is_whitespace())
            .filter(|part| !part.is_empty())
            .map(Prefix::parse)
            .collect::<Result<Vec<_>, _>>()
            .map(Allowed)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn prefixes(&self) -> &[Prefix] {
        &self.0
    }

    /// Whether a node may send packets to this address. The error is the space that rules it
    /// out. An allowed range opens private and shared address space and nothing else, so
    /// `0.0.0.0/0` still leaves loopback, link-local and multicast addresses refused.
    pub fn permit(&self, address: IpAddr) -> Result<(), Space> {
        match Space::of(address) {
            Space::Public => Ok(()),
            space if space.can_be_allowed() && self.0.iter().any(|p| p.contains(address)) => Ok(()),
            space => Err(space),
        }
    }
}

/// Names that belong to a local network by their last label: the special-use names of RFC 6761,
/// 6762, 8375 and 9476, the one ICANN set aside for private use, and the ones routers and
/// offices commonly hand out.
const LOCAL_SUFFIXES: [&str; 14] = [
    "localhost",
    "local",
    "localdomain",
    "lan",
    "home",
    "home.arpa",
    "internal",
    "intranet",
    "corp",
    "private",
    "test",
    "invalid",
    "onion",
    "alt",
];

/// Whether a name can only be one of the node's own network: a single label, which a resolver
/// completes with its search domain, or a name under a local suffix. A node with no allowed
/// range refuses such a name before it asks its resolver, so what the resolver knows about the
/// devices around it stays there.
pub fn is_local_name(name: &str) -> bool {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if !name.contains('.') {
        return true;
    }
    LOCAL_SUFFIXES
        .iter()
        .any(|suffix| name == *suffix || name.ends_with(&format!(".{suffix}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space(text: &str) -> Space {
        Space::of(text.parse().unwrap())
    }

    #[test]
    fn ipv4_space() {
        for (text, expected) in [
            ("1.1.1.1", Space::Public),
            ("8.8.8.8", Space::Public),
            ("100.63.255.255", Space::Public),
            ("100.128.0.1", Space::Public),
            ("172.32.0.1", Space::Public),
            ("192.169.0.1", Space::Public),
            ("198.20.0.1", Space::Public),
            ("223.255.255.255", Space::Public),
            ("10.0.0.1", Space::Private),
            ("172.16.0.1", Space::Private),
            ("172.31.255.254", Space::Private),
            ("192.168.178.1", Space::Private),
            ("100.64.0.1", Space::Shared),
            ("100.127.255.255", Space::Shared),
            ("127.0.0.1", Space::Loopback),
            ("127.255.255.255", Space::Loopback),
            ("169.254.169.254", Space::LinkLocal),
            ("224.0.0.1", Space::Multicast),
            ("239.255.255.250", Space::Multicast),
            ("255.255.255.255", Space::Broadcast),
            ("0.0.0.0", Space::Reserved),
            ("0.1.2.3", Space::Reserved),
            ("192.0.0.8", Space::Reserved),
            ("192.0.2.1", Space::Reserved),
            ("198.51.100.7", Space::Reserved),
            ("203.0.113.9", Space::Reserved),
            ("192.88.99.1", Space::Reserved),
            ("198.18.0.1", Space::Reserved),
            ("198.19.255.255", Space::Reserved),
            ("240.0.0.1", Space::Reserved),
            ("255.255.255.254", Space::Reserved),
        ] {
            assert_eq!(space(text), expected, "{text}");
        }
    }

    #[test]
    fn ipv6_space() {
        for (text, expected) in [
            ("2606:4700:4700::1111", Space::Public),
            ("2a00:1450:4001:81b::200e", Space::Public),
            ("fd00::1", Space::Private),
            ("fc00::1", Space::Private),
            ("::1", Space::Loopback),
            ("fe80::1", Space::LinkLocal),
            ("febf::1", Space::LinkLocal),
            ("ff02::1", Space::Multicast),
            ("::", Space::Reserved),
            ("2001:db8::1", Space::Reserved),
            ("3fff:0:1::1", Space::Reserved),
            ("2001::1", Space::Reserved),
            ("100::1", Space::Reserved),
            ("64:ff9b:1::1", Space::Reserved),
            ("fec0::1", Space::Reserved),
            ("4000::1", Space::Reserved),
        ] {
            assert_eq!(space(text), expected, "{text}");
        }
    }

    #[test]
    fn an_ipv4_address_inside_an_ipv6_one_is_judged_as_the_ipv4_address() {
        for (text, expected) in [
            // Mapped: a private one is private, and a public one is no destination of its own.
            ("::ffff:10.0.0.1", Space::Private),
            ("::ffff:127.0.0.1", Space::Loopback),
            ("::ffff:169.254.169.254", Space::LinkLocal),
            ("::ffff:1.1.1.1", Space::Reserved),
            // NAT64: where the packet ends up decides.
            ("64:ff9b::101:101", Space::Public),
            ("64:ff9b::a00:1", Space::Private),
            ("64:ff9b::7f00:1", Space::Loopback),
            // 6to4.
            ("2002:101:101::1", Space::Public),
            ("2002:c0a8:101::1", Space::Private),
            ("2002:7f00:1::1", Space::Loopback),
        ] {
            assert_eq!(space(text), expected, "{text}");
        }
    }

    #[test]
    fn a_prefix_holds_the_addresses_under_it() {
        let lan = Prefix::parse("192.168.1.0/24").unwrap();
        assert!(lan.contains("192.168.1.7".parse().unwrap()));
        assert!(!lan.contains("192.168.2.7".parse().unwrap()));
        assert!(!lan.contains("fd00::1".parse().unwrap()));
        // The host bits of what was written don't matter.
        assert_eq!(Prefix::parse("192.168.1.77/24").unwrap(), lan);
        assert_eq!(lan.to_string(), "192.168.1.0/24");

        let host = Prefix::parse("10.1.2.3").unwrap();
        assert!(host.contains("10.1.2.3".parse().unwrap()));
        assert!(!host.contains("10.1.2.4".parse().unwrap()));

        let ula = Prefix::parse("fd12:3456::/32").unwrap();
        assert!(ula.contains("fd12:3456:9::1".parse().unwrap()));
        assert!(!ula.contains("fd12:3457::1".parse().unwrap()));

        let all = Prefix::parse("0.0.0.0/0").unwrap();
        assert!(all.contains("203.0.113.1".parse().unwrap()));
        assert!(Prefix::parse("::/0")
            .unwrap()
            .contains("fd00::1".parse().unwrap()));
    }

    #[test]
    fn what_is_not_a_prefix_says_so() {
        for text in ["", "lan", "10.0.0.0/33", "fd00::/129", "10.0.0.0/-1", "/8"] {
            assert!(Prefix::parse(text).is_err(), "{text}");
        }
        assert_eq!(
            Prefix::parse("10.0.0.0/33").unwrap_err(),
            "10.0.0.0/33 is not a prefix such as 192.168.0.0/16 or fd00::/8."
        );
        assert!(Allowed::parse("10.0.0.0/8, nope").is_err());
    }

    #[test]
    fn nothing_is_allowed_until_the_owner_says_so() {
        let none = Allowed::none();
        assert_eq!(none.permit("1.1.1.1".parse().unwrap()), Ok(()));
        assert_eq!(
            none.permit("192.168.1.1".parse().unwrap()),
            Err(Space::Private)
        );
        assert_eq!(
            none.permit("100.64.0.1".parse().unwrap()),
            Err(Space::Shared)
        );
        assert_eq!(none.permit("fd00::1".parse().unwrap()), Err(Space::Private));
    }

    #[test]
    fn an_allowed_range_opens_private_space_and_nothing_else() {
        let allowed = Allowed::parse("192.168.0.0/16 100.64.0.0/10,fd00::/8").unwrap();
        assert_eq!(allowed.prefixes().len(), 3);
        assert_eq!(allowed.permit("192.168.1.1".parse().unwrap()), Ok(()));
        assert_eq!(allowed.permit("100.64.0.1".parse().unwrap()), Ok(()));
        assert_eq!(allowed.permit("fd00::1".parse().unwrap()), Ok(()));
        // Private, and outside what was allowed.
        assert_eq!(
            allowed.permit("10.0.0.1".parse().unwrap()),
            Err(Space::Private)
        );

        // Allowing everything still leaves these refused.
        let everything = Allowed::parse("0.0.0.0/0, ::/0").unwrap();
        assert_eq!(everything.permit("10.0.0.1".parse().unwrap()), Ok(()));
        for (text, space) in [
            ("127.0.0.1", Space::Loopback),
            ("::1", Space::Loopback),
            ("169.254.169.254", Space::LinkLocal),
            ("fe80::1", Space::LinkLocal),
            ("224.0.0.1", Space::Multicast),
            ("ff02::1", Space::Multicast),
            ("255.255.255.255", Space::Broadcast),
            ("0.0.0.0", Space::Reserved),
            ("::ffff:127.0.0.1", Space::Loopback),
            ("64:ff9b::7f00:1", Space::Loopback),
        ] {
            assert_eq!(
                everything.permit(text.parse().unwrap()),
                Err(space),
                "{text}"
            );
        }
        // A private address written as IPv6 is opened by the IPv6 range that holds it, never by
        // the IPv4 range of the address inside.
        let v4_only = Allowed::parse("10.0.0.0/8").unwrap();
        assert_eq!(
            v4_only.permit("::ffff:10.0.0.1".parse().unwrap()),
            Err(Space::Private)
        );
    }

    #[test]
    fn local_names() {
        for name in [
            "router",
            "nas",
            "localhost",
            "printer.local",
            "johns-iphone.lan",
            "nas.home.arpa",
            "db.internal",
            "wiki.corp",
            "LOCALHOST.",
            "a.b.localhost",
            "service.test",
        ] {
            assert!(is_local_name(name), "{name}");
        }
        for name in [
            "example.com",
            "example.com.",
            "localhost.example.com",
            "lan.example.net",
            "homelab.dev",
            "one.one.one.one",
        ] {
            assert!(!is_local_name(name), "{name}");
        }
    }
}
