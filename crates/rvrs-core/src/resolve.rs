//! Forward and reverse DNS through the system resolver.

use std::ffi::{CStr, CString};
use std::net::{IpAddr, SocketAddr, UdpSocket};

use socket2::SockAddr;

use crate::{Family, FamilyChoice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    NotFound(String),
    NoAddress(String, Family),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NotFound(host) => {
                write!(formatter, "Couldn't find \u{201C}{host}\u{201D}.")
            }
            ResolveError::NoAddress(host, family) => {
                write!(
                    formatter,
                    "\u{201C}{host}\u{201D} has no {} address.",
                    family.label()
                )
            }
        }
    }
}

/// Resolves a name or literal. A literal is taken as it is, whatever was asked for. A name is
/// looked up in the asked family, or with `Auto` in the order the system itself would connect,
/// which prefers IPv6 only where this device has it.
pub fn resolve(host: &str, choice: FamilyChoice) -> Result<IpAddr, ResolveError> {
    if let Ok(literal) = host.parse::<IpAddr>() {
        return Ok(literal);
    }
    let not_found = || ResolveError::NotFound(host.to_string());
    let name = CString::new(host).map_err(|_| not_found())?;
    // SAFETY: an all-zero addrinfo is a valid hints value.
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_socktype = libc::SOCK_DGRAM;
    match choice {
        FamilyChoice::Auto => {
            hints.ai_family = libc::AF_UNSPEC;
            hints.ai_flags = libc::AI_ADDRCONFIG;
        }
        FamilyChoice::V4 => hints.ai_family = libc::AF_INET,
        FamilyChoice::V6 => hints.ai_family = libc::AF_INET6,
    }
    let mut list: *mut libc::addrinfo = std::ptr::null_mut();
    // SAFETY: `name` and `hints` are valid for the call; `list` is freed below.
    let status = unsafe { libc::getaddrinfo(name.as_ptr(), std::ptr::null(), &hints, &mut list) };
    let mut found = None;
    if status == 0 {
        let mut cursor = list;
        while !cursor.is_null() && found.is_none() {
            // SAFETY: getaddrinfo returned a valid list; ai_addr points at ai_addrlen bytes.
            unsafe {
                let info = &*cursor;
                if !info.ai_addr.is_null() {
                    let (_, address) = SockAddr::try_init(|storage, length| {
                        std::ptr::copy_nonoverlapping(
                            info.ai_addr.cast::<u8>(),
                            storage.cast::<u8>(),
                            info.ai_addrlen as usize,
                        );
                        *length = info.ai_addrlen;
                        Ok(())
                    })
                    .map_err(|_| not_found())?;
                    // Asked for IPv6 on a device without it, some systems answer with an IPv4
                    // address mapped into IPv6. That is not an IPv6 address of the host.
                    found = address
                        .as_socket()
                        .map(|socket| socket.ip())
                        .filter(|ip| match ip {
                            IpAddr::V6(v6) => v6.to_ipv4_mapped().is_none(),
                            IpAddr::V4(_) => true,
                        });
                }
                cursor = info.ai_next;
            }
        }
        // SAFETY: `list` came from getaddrinfo and is freed once.
        unsafe { libc::freeaddrinfo(list) };
    }
    found.ok_or_else(|| match choice {
        FamilyChoice::V4 if status == 0 || has_other_family(host, choice) => {
            ResolveError::NoAddress(host.to_string(), Family::V4)
        }
        FamilyChoice::V6 if status == 0 || has_other_family(host, choice) => {
            ResolveError::NoAddress(host.to_string(), Family::V6)
        }
        _ => not_found(),
    })
}

/// True when the name exists but only in the family that wasn't asked for.
fn has_other_family(host: &str, asked: FamilyChoice) -> bool {
    let other = match asked {
        FamilyChoice::V4 => FamilyChoice::V6,
        FamilyChoice::V6 => FamilyChoice::V4,
        FamilyChoice::Auto => return false,
    };
    resolve_quiet(host, other).is_some()
}

fn resolve_quiet(host: &str, choice: FamilyChoice) -> Option<IpAddr> {
    use std::net::ToSocketAddrs;
    (host, 0)
        .to_socket_addrs()
        .ok()?
        .map(|socket| socket.ip())
        .find(|address| match choice {
            FamilyChoice::V4 => address.is_ipv4(),
            FamilyChoice::V6 => address.is_ipv6(),
            FamilyChoice::Auto => true,
        })
}

/// The PTR name of an address, or None. Blocks for as long as the system resolver takes.
pub fn reverse(address: IpAddr) -> Option<String> {
    let socket = SockAddr::from(SocketAddr::new(address, 0));
    let mut host = [0 as libc::c_char; 1025];
    // SAFETY: `socket` is a valid sockaddr of the given length and `host` is writable.
    let status = unsafe {
        libc::getnameinfo(
            socket.as_ptr().cast(),
            socket.len(),
            host.as_mut_ptr(),
            host.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        )
    };
    if status != 0 {
        return None;
    }
    // SAFETY: getnameinfo wrote a NUL-terminated string.
    let name = unsafe { CStr::from_ptr(host.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    (!name.is_empty()).then_some(name)
}

/// The address probes toward `destination` leave from. No packet is sent: a connected UDP
/// socket is enough for the system to pick the route.
pub fn source_toward(destination: IpAddr) -> Option<IpAddr> {
    let bind: SocketAddr = match destination {
        IpAddr::V4(_) => "0.0.0.0:0".parse().ok()?,
        IpAddr::V6(_) => "[::]:0".parse().ok()?,
    };
    let socket = UdpSocket::bind(bind).ok()?;
    socket.connect(SocketAddr::new(destination, 33434)).ok()?;
    socket.local_addr().ok().map(|local| local.ip())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_literal_wins_whatever_family_was_asked() {
        assert_eq!(
            resolve("1.1.1.1", FamilyChoice::V6),
            Ok("1.1.1.1".parse().unwrap())
        );
        assert_eq!(resolve("::1", FamilyChoice::V4), Ok("::1".parse().unwrap()));
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            ResolveError::NotFound("nope.invalid".into()).to_string(),
            "Couldn't find \u{201C}nope.invalid\u{201D}."
        );
        assert_eq!(
            ResolveError::NoAddress("example.com".into(), Family::V6).to_string(),
            "\u{201C}example.com\u{201D} has no IPv6 address."
        );
    }
}
