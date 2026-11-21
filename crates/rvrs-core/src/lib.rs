//! The rvrs engines, the run model and the report, with no terminal code. The CLI, the Console
//! and later the node all drive a run through [`session::Session`].

pub mod asn;
pub mod bgp;
pub mod cases;
pub mod dns;
pub mod engine;
pub mod favorites;
pub mod format;
pub mod history;
pub mod model;
pub mod mtu;
pub mod packet;
pub mod paths;
pub mod report;
pub mod resolve;
pub mod session;
pub mod socket;
pub mod tcp;

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

/// An address family a socket is opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    pub fn of(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(_) => Family::V4,
            IpAddr::V6(_) => Family::V6,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Family::V4 => "IPv4",
            Family::V6 => "IPv6",
        }
    }
}

/// Which family a name should resolve to. A literal address decides for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FamilyChoice {
    #[default]
    Auto,
    V4,
    V6,
}

/// Private, loopback, link-local and carrier-grade NAT space. These addresses are never sent to
/// a lookup service and belong to no public network.
pub fn is_private(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let [first, second, ..] = v4.octets();
            match first {
                0 | 10 | 127 => true,
                100 => (64..=127).contains(&second),
                169 => second == 254,
                172 => (16..=31).contains(&second),
                192 => second == 168,
                _ => false,
            }
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_private(IpAddr::V4(mapped));
            }
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || first & 0xffc0 == 0xfe80
                || first & 0xfe00 == 0xfc00
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_space() {
        for text in [
            "10.0.0.1",
            "192.168.1.1",
            "172.16.0.1",
            "172.31.255.1",
            "100.64.0.1",
            "169.254.1.1",
            "127.0.0.1",
            "fe80::1",
            "fd00::1",
            "::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(is_private(text.parse().unwrap()), "{text}");
        }
        for text in [
            "1.1.1.1",
            "172.32.0.1",
            "100.128.0.1",
            "192.169.0.1",
            "2606:4700:4700::1111",
        ] {
            assert!(!is_private(text.parse().unwrap()), "{text}");
        }
    }
}
