//! Asks the device's resolver, the public resolvers and, after walking the delegation from the
//! root, the zone's own server. Every source is a snapshot with its own timestamp. The walk is
//! the app's (`DNSLookup.swift`): referrals followed from the root, DS at each parent, DNSKEY
//! and RRSIG at the zone. The DNSSEC verdict says which records are present. Nothing is
//! validated.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

use super::client::{self, Question, Reply};
use super::message::{rcode_name, type_name, types};
use super::result::{DelegationStep, Dnssec, SourceKind, SourceResult};
use crate::{format, is_private};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Resolver {
    pub label: String,
    pub address: IpAddr,
}

impl Resolver {
    fn new(label: &str, address: [u8; 4]) -> Self {
        Self {
            label: label.to_string(),
            address: IpAddr::from(address),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub public_resolvers: Vec<Resolver>,
    /// Resolvers of the person's own, asked beside the public ones.
    pub own_resolvers: Vec<Resolver>,
    /// The device's resolvers. None reads them from `/etc/resolv.conf`.
    pub device_resolvers: Option<Vec<IpAddr>>,
    /// Where the walk starts. The root servers, unless a test stands in for them.
    pub roots: Vec<(String, IpAddr)>,
    pub timeout: Duration,
    /// The types every source is asked for. HTTPS carries the modern service binding.
    pub types: Vec<u16>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            public_resolvers: vec![
                Resolver::new("Cloudflare", [1, 1, 1, 1]),
                Resolver::new("Google", [8, 8, 8, 8]),
                Resolver::new("Quad9", [9, 9, 9, 9]),
            ],
            own_resolvers: Vec::new(),
            device_resolvers: None,
            // A few root servers. The walk starts at the first one that answers.
            roots: vec![
                (
                    "a.root-servers.net".to_string(),
                    IpAddr::from([198, 41, 0, 4]),
                ),
                (
                    "b.root-servers.net".to_string(),
                    IpAddr::from([170, 247, 170, 2]),
                ),
                (
                    "c.root-servers.net".to_string(),
                    IpAddr::from([192, 33, 4, 12]),
                ),
                (
                    "d.root-servers.net".to_string(),
                    IpAddr::from([199, 7, 91, 13]),
                ),
            ],
            timeout: Duration::from_secs(3),
            types: vec![
                types::A,
                types::AAAA,
                types::CNAME,
                types::MX,
                types::TXT,
                types::NS,
                types::SOA,
                types::HTTPS,
                types::CAA,
            ],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DnsEvent {
    /// How many sources will answer.
    Plan(usize),
    Source(SourceResult),
    Step(DelegationStep),
    Dnssec(Dnssec, String),
    Finished,
    /// The person stopped the lookup. What had answered by then stands.
    Stopped,
    Failed(String),
}

/// The name as every server is asked for it: lower case, no dots at the ends.
pub fn normalize(domain: &str) -> String {
    domain.trim().trim_matches('.').to_lowercase()
}

/// The resolvers this device is using right now, read from `/etc/resolv.conf`.
pub fn device_resolvers() -> Vec<IpAddr> {
    std::fs::read_to_string("/etc/resolv.conf")
        .map(|text| parse_resolv_conf(&text))
        .unwrap_or_default()
}

pub fn parse_resolv_conf(text: &str) -> Vec<IpAddr> {
    let mut found: Vec<IpAddr> = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        if parts.next() != Some("nameserver") {
            continue;
        }
        // A link-local IPv6 resolver carries its interface after a percent sign.
        let Some(address) = parts
            .next()
            .and_then(|text| text.split('%').next())
            .and_then(|text| text.parse().ok())
        else {
            continue;
        };
        if !found.contains(&address) {
            found.push(address);
        }
    }
    found
}

/// Runs the lookup and reports through `events`. Blocks until every source has answered or
/// given up; run it on its own thread. Setting `stop` ends the walk early and the run with `Stopped`.
pub fn run(domain: &str, options: &Options, events: &Sender<DnsEvent>, stop: &AtomicBool) {
    let name = normalize(domain);
    let device = options
        .device_resolvers
        .clone()
        .unwrap_or_else(device_resolvers);
    let resolvers: Vec<&Resolver> = options
        .public_resolvers
        .iter()
        .chain(&options.own_resolvers)
        .collect();
    let _ = events.send(DnsEvent::Plan(
        usize::from(!device.is_empty()) + resolvers.len() + 1,
    ));

    std::thread::scope(|scope| {
        if let Some(first) = device.first().copied() {
            let name = &name;
            scope.spawn(move || {
                // A resolver on a private address is the router, or something standing in for it.
                let label = if is_private(first) {
                    "Router".to_string()
                } else {
                    first.to_string()
                };
                let source = ask_source(SourceKind::Device, &label, first, name, true, options);
                let _ = events.send(DnsEvent::Source(source));
            });
        }
        for resolver in resolvers {
            let name = &name;
            scope.spawn(move || {
                let source = ask_source(
                    SourceKind::Public,
                    &resolver.label,
                    resolver.address,
                    name,
                    true,
                    options,
                );
                let _ = events.send(DnsEvent::Source(source));
            });
        }
        let name = &name;
        scope.spawn(move || walk_delegation(name, options, events, stop));
    });
    let _ = events.send(if stop.load(Ordering::Relaxed) {
        DnsEvent::Stopped
    } else {
        DnsEvent::Finished
    });
}

/// Every record type against one server, each question on its own thread. A question that
/// fails is recorded for its type and leaves the others standing.
pub fn ask_source(
    kind: SourceKind,
    label: &str,
    server: IpAddr,
    name: &str,
    recursive: bool,
    options: &Options,
) -> SourceResult {
    let mut source = SourceResult::new(kind, label, &server.to_string());
    let mut replies: Vec<(u16, Result<Reply, client::ClientError>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = options
            .types
            .iter()
            .map(|type_code| {
                let type_code = *type_code;
                scope.spawn(move || {
                    (
                        type_code,
                        client::query(&Question::new(
                            name,
                            type_code,
                            server,
                            recursive,
                            options.timeout,
                        )),
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .filter_map(|handle| handle.join().ok())
            .collect()
    });
    replies.sort_by_key(|(type_code, _)| *type_code);

    let mut fastest: Option<Duration> = None;
    let mut first_error: Option<String> = None;
    for (type_code, outcome) in replies {
        match outcome {
            Ok(reply) => {
                fastest = Some(fastest.map_or(reply.rtt, |best| best.min(reply.rtt)));
                let response = reply.response;
                if response.rcode != 0 {
                    source
                        .failures
                        .insert(type_name(type_code), rcode_name(response.rcode));
                }
                source.authentic_data |= response.authentic_data();
                source.is_authoritative |= response.is_authoritative();
                for record in response.records {
                    // A recursive answer's authority section repeats SOA and NS. Keep the answer
                    // section and the signatures.
                    let wanted = record.section == super::Section::Answer
                        || record.type_code == types::RRSIG
                        || kind == SourceKind::Authoritative;
                    if wanted && !source.records.contains(&record) {
                        source.records.push(record);
                    }
                }
            }
            Err(error) => {
                first_error.get_or_insert_with(|| error.to_string());
            }
        }
    }
    match fastest {
        Some(rtt) => source.rtt_ms = Some(format::ms_value(rtt)),
        None => source.error = first_error,
    }
    source
}

/// Root, TLD, zone: referrals followed down, then the zone's own answers. DS at each parent and
/// RRSIGs along the way give the DNSSEC verdict. A walk that can't finish says where it
/// stopped and leaves the other sources' answers standing.
fn walk_delegation(name: &str, options: &Options, events: &Sender<DnsEvent>, stop: &AtomicBool) {
    let ask = |question_name: &str, type_code: u16, server: IpAddr, recursive: bool| {
        client::query(&Question::new(
            question_name,
            type_code,
            server,
            recursive,
            options.timeout,
        ))
    };
    let unknown = || {
        let _ = events.send(DnsEvent::Dnssec(
            Dnssec::Unknown,
            "Couldn't check every step of the delegation.".to_string(),
        ));
    };

    let mut zone = ".".to_string();
    let mut candidates: Vec<(String, IpAddr)> = options.roots.clone();
    let mut steps: Vec<DelegationStep> = Vec::new();
    let mut final_zone = name.to_string();
    let mut authoritative: Option<(String, IpAddr)> = None;

    for _ in 0..8 {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let mut step: Option<DelegationStep> = None;
        let mut referral: Option<(String, Vec<(String, IpAddr)>)> = None;
        for (server_name, address) in candidates.iter().take(3) {
            let mut attempt = DelegationStep::new(&zone, server_name, &address.to_string());
            let reply = match ask(name, types::NS, *address, false) {
                Ok(reply) => reply,
                Err(error) => {
                    attempt.error = Some(error.to_string());
                    steps.push(attempt.clone());
                    let _ = events.send(DnsEvent::Step(attempt));
                    continue;
                }
            };
            let response = reply.response;
            attempt.rtt_ms = Some(format::ms_value(reply.rtt));
            attempt.signed = response
                .records
                .iter()
                .any(|record| record.type_code == types::RRSIG);
            let is_authoritative = response.is_authoritative();
            let nameservers: Vec<_> = if is_authoritative {
                response.answers().collect::<Vec<_>>()
            } else {
                response.authority().collect::<Vec<_>>()
            }
            .into_iter()
            .filter(|record| record.type_code == types::NS)
            .collect();
            let zone_name = if zone == "." { "" } else { zone.as_str() };
            if is_authoritative
                || nameservers.is_empty()
                || nameservers
                    .first()
                    .is_some_and(|record| record.name == zone_name)
            {
                // This server holds the name, or has nothing further to delegate: the walk ends here.
                attempt.nameservers = nameservers
                    .iter()
                    .map(|record| record.value.clone())
                    .collect();
                final_zone = if is_authoritative {
                    nameservers
                        .first()
                        .map_or_else(|| name.to_string(), |record| record.name.clone())
                } else {
                    zone.clone()
                };
                authoritative = Some((server_name.clone(), *address));
                step = Some(attempt);
                break;
            }
            let child = nameservers[0].name.clone();
            attempt.next_zone = Some(child.clone());
            attempt.nameservers = nameservers
                .iter()
                .map(|record| record.value.clone())
                .collect();
            attempt.ds_present = Some(
                response
                    .authority()
                    .any(|record| record.type_code == types::DS && record.name == child),
            );
            let mut glued: Vec<(String, IpAddr)> = Vec::new();
            for nameserver in &nameservers {
                let glue = response
                    .additional()
                    .find(|record| record.type_code == types::A && record.name == nameserver.value);
                if let Some(address) = glue.and_then(|record| record.value.parse().ok()) {
                    glued.push((nameserver.value.clone(), address));
                }
            }
            if glued.is_empty() {
                // No glue: the nameservers live outside the zone. Resolve a couple of them
                // through a public resolver.
                let helper = options
                    .public_resolvers
                    .first()
                    .map_or(IpAddr::from([1, 1, 1, 1]), |resolver| resolver.address);
                for nameserver in nameservers.iter().take(2) {
                    let address = ask(&nameserver.value, types::A, helper, true)
                        .ok()
                        .and_then(|reply| {
                            reply
                                .response
                                .answers()
                                .find(|record| record.type_code == types::A)
                                .and_then(|record| record.value.parse().ok())
                        });
                    if let Some(address) = address {
                        glued.push((nameserver.value.clone(), address));
                    }
                }
            }
            referral = Some((child, glued));
            step = Some(attempt);
            break;
        }
        let Some(step) = step else {
            // None of this zone's servers answered.
            return unknown();
        };
        steps.push(step.clone());
        let _ = events.send(DnsEvent::Step(step));
        let Some((child, servers)) = referral else {
            break;
        };
        if servers.is_empty() {
            let mut lost = DelegationStep::new(&child, "", "");
            lost.error = Some(format!("No nameserver answered for {child}."));
            let _ = events.send(DnsEvent::Step(lost));
            return unknown();
        }
        zone = child;
        candidates = servers;
    }

    let Some((server_name, address)) = authoritative else {
        return unknown();
    };
    let mut source = ask_source(
        SourceKind::Authoritative,
        &server_name,
        address,
        name,
        false,
        options,
    );
    source.is_authoritative = true;
    let answers_signed = source
        .records
        .iter()
        .any(|record| record.type_code == types::RRSIG);
    let _ = events.send(DnsEvent::Source(source));

    // DNSSEC: the last referral must carry a DS, the zone must publish DNSKEY and sign its answers.
    let last_referral = steps.iter().rev().find(|step| step.next_zone.is_some());
    let has_keys = ask(&final_zone, types::DNSKEY, address, false).is_ok_and(|reply| {
        reply
            .response
            .answers()
            .any(|record| record.type_code == types::DNSKEY)
    });
    let unsigned_parent = steps.iter().find(|step| step.ds_present == Some(false));
    let ds_at_parent = last_referral.is_some_and(|step| step.ds_present == Some(true));
    let (status, detail) = match (unsigned_parent, unsigned_parent.and_then(|step| step.next_zone.as_deref())) {
        (Some(parent), Some(child)) => {
            let parent = if parent.zone == "." { "The root".to_string() } else { parent.zone.clone() };
            (Dnssec::Unsigned, format!("{parent} publishes no DS for {child}, so the chain stops there."))
        }
        _ if ds_at_parent && has_keys && answers_signed => {
            let parent = last_referral.map_or("The parent", |step| step.zone.as_str());
            let parent = if parent == "." { "The root" } else { parent };
            (Dnssec::Signed, format!("{parent} publishes a DS for {final_zone}, the zone serves its DNSKEY records and its answers are signed. A validating resolver can check every step."))
        }
        _ if ds_at_parent && has_keys => (Dnssec::Broken, "The zone publishes keys but its answers carry no signatures.".to_string()),
        _ if ds_at_parent => (
            Dnssec::Broken,
            format!("The parent publishes a DS but {final_zone} serves no DNSKEY. Validating resolvers fail this name."),
        ),
        _ => (Dnssec::Unknown, "Couldn't check every step of the delegation.".to_string()),
    };
    let _ = events.send(DnsEvent::Dnssec(status, detail));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_asked_lower_case_without_dots_at_the_ends() {
        assert_eq!(normalize(" Example.COM. "), "example.com");
        assert_eq!(normalize("."), "");
    }

    #[test]
    fn resolv_conf_gives_each_resolver_once_without_its_interface() {
        let text = "# made by the system\nsearch lan\nnameserver 192.168.1.1\nnameserver fe80::1%en0\nnameserver 192.168.1.1\nnameserver nonsense\noptions ndots:1\n";
        let found = parse_resolv_conf(text);
        assert_eq!(
            found,
            vec![
                "192.168.1.1".parse::<IpAddr>().unwrap(),
                "fe80::1".parse().unwrap()
            ]
        );
        assert!(parse_resolv_conf("").is_empty());
    }

    #[test]
    fn the_defaults_are_the_apps() {
        let options = Options::default();
        let labels: Vec<&str> = options
            .public_resolvers
            .iter()
            .map(|resolver| resolver.label.as_str())
            .collect();
        assert_eq!(labels, ["Cloudflare", "Google", "Quad9"]);
        assert_eq!(options.types.len(), 9);
        assert_eq!(options.timeout, Duration::from_secs(3));
        assert_eq!(options.roots[0].0, "a.root-servers.net");
    }
}
