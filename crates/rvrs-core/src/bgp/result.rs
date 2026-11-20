//! What a BGP lookup found: the route an address falls in, who originates it, whether RPKI
//! authorizes that, how collectors reach it, what changed, and the network behind it. For an AS
//! number only the network part is filled.
//!
//! Stored in history and saved runs: a field added later must carry `#[serde(default)]`.

use std::collections::BTreeMap;
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use super::glass::{GlassResult, GlassSessions};
use super::lookup::{BgpEvent, NetworkPart};
use crate::format;
use crate::model::Run;

/// "A", "A and B", "A, B, and C".
pub fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [first, second] => format!("{first} and {second}"),
        [head @ .., last] => format!("{}, and {last}", head.join(", ")),
    }
}

/// "1,204".
pub fn count(number: u64) -> String {
    let digits = number.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// "1 prefix", "1,204 prefixes".
pub fn counted(number: u64, one: &str, many: &str) -> String {
    format!("{} {}", count(number), if number == 1 { one } else { many })
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        one.to_string()
    } else {
        many.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Host,
    Address,
    Prefix,
    Asn,
}

/// What a BGP lookup was asked about: a hostname, an address, a prefix or a network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: TargetKind,
    /// The target as rvrs writes it: "example.com", "192.0.2.1", "192.0.2.0/24", "AS64500".
    pub canonical: String,
}

impl Target {
    pub fn asn(&self) -> Option<u32> {
        match self.kind {
            TargetKind::Asn => self.canonical[2..].parse().ok(),
            _ => None,
        }
    }

    /// Reads what was typed after `rvrs bgp`. A bare number is an AS number here, since the BGP
    /// tool was chosen. "AS64500" is one anywhere.
    pub fn parse(raw: &str) -> Option<Target> {
        let text = raw.trim();
        if text.is_empty() {
            return None;
        }
        if let Some(number) = as_number(text) {
            return Some(Target {
                kind: TargetKind::Asn,
                canonical: format!("AS{number}"),
            });
        }
        if !text.contains("://") {
            if let Some((base, length)) = text.rsplit_once('/') {
                if let (Ok(address), Ok(length)) = (base.parse::<IpAddr>(), length.parse::<u8>()) {
                    let longest = if address.is_ipv4() { 32 } else { 128 };
                    if length <= longest {
                        return Some(Target {
                            kind: TargetKind::Prefix,
                            canonical: format!("{address}/{length}"),
                        });
                    }
                }
            }
        }
        let host = host_of(text)?;
        if let Ok(address) = host.parse::<IpAddr>() {
            return Some(Target {
                kind: TargetKind::Address,
                canonical: address.to_string(),
            });
        }
        Some(Target {
            kind: TargetKind::Host,
            canonical: host,
        })
    }

    /// True for "AS64500" and "192.0.2.0/24": targets only the BGP tool can take.
    pub fn is_routing_only(text: &str) -> bool {
        match Target::parse(text) {
            Some(target) => {
                (target.kind == TargetKind::Asn && text.trim().to_lowercase().starts_with("as"))
                    || target.kind == TargetKind::Prefix
            }
            None => false,
        }
    }
}

/// "AS64500", "as64500" or "64500". `None` for anything else.
pub fn as_number(text: &str) -> Option<u32> {
    let text = text.trim();
    let digits = if text.len() >= 2 && text[..2].eq_ignore_ascii_case("as") {
        &text[2..]
    } else {
        text
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok().filter(|number| *number > 0)
}

/// The host in what was typed or pasted: a URL loses its scheme, path and port.
fn host_of(text: &str) -> Option<String> {
    let rest = text.split_once("://").map_or(text, |(_, rest)| rest);
    let rest = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    let host = if let Some(bracketed) = rest.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or(bracketed)
    } else if rest.matches(':').count() == 1 {
        rest.split(':').next().unwrap_or(rest)
    } else {
        rest
    };
    let host = host.trim_end_matches('.').to_lowercase();
    if host.is_empty() || host.chars().any(char::is_whitespace) {
        return None;
    }
    Some(host)
}

/// RPKI origin validation of one route: a prefix announced by one origin AS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpkiStatus {
    Valid,
    InvalidAsn,
    InvalidLength,
    NotFound,
    Unavailable,
}

impl RpkiStatus {
    /// RIPEstat's words: valid, invalid_asn, invalid_length, unknown.
    pub fn from_provider(word: &str) -> Self {
        match word.to_lowercase().as_str() {
            "valid" => RpkiStatus::Valid,
            "invalid_asn" => RpkiStatus::InvalidAsn,
            "invalid_length" => RpkiStatus::InvalidLength,
            "unknown" | "not_found" => RpkiStatus::NotFound,
            _ => RpkiStatus::Unavailable,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            RpkiStatus::Valid => "Origin authorized",
            RpkiStatus::InvalidAsn => "Wrong origin ASN",
            RpkiStatus::InvalidLength => "Prefix too specific",
            RpkiStatus::NotFound => "No ROA found",
            RpkiStatus::Unavailable => "Unable to check",
        }
    }

    /// The word on a badge.
    pub fn badge(self) -> &'static str {
        match self {
            RpkiStatus::Valid => "Valid",
            RpkiStatus::InvalidAsn | RpkiStatus::InvalidLength => "Invalid",
            RpkiStatus::NotFound => "Not Found",
            RpkiStatus::Unavailable => "Unknown",
        }
    }

    pub fn is_invalid(self) -> bool {
        matches!(self, RpkiStatus::InvalidAsn | RpkiStatus::InvalidLength)
    }

    pub fn grade(self) -> Grade {
        match self {
            RpkiStatus::Valid => Grade::Good,
            RpkiStatus::InvalidAsn | RpkiStatus::InvalidLength => Grade::Bad,
            RpkiStatus::NotFound | RpkiStatus::Unavailable => Grade::Neutral,
        }
    }
}

/// One validated ROA payload that covers a route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Roa {
    pub origin: u32,
    pub prefix: String,
    pub max_length: u32,
    /// The provider's verdict on this ROA against the route: valid, invalid_asn, invalid_length.
    pub validity: String,
}

/// One of the three questions origin validation asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub title: &'static str,
    pub detail: String,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Validation {
    pub prefix: String,
    pub origin: u32,
    pub status: RpkiStatus,
    #[serde(default)]
    pub roas: Vec<Roa>,
    #[serde(default)]
    pub checked_at: u64,
}

impl Validation {
    pub fn is(&self, prefix: &str, origin: u32) -> bool {
        self.prefix == prefix && self.origin == origin
    }

    /// One or two plain sentences: what was checked and what it means.
    pub fn explanation(&self) -> String {
        let (prefix, origin) = (&self.prefix, self.origin);
        match self.status {
            RpkiStatus::Valid => {
                format!("A ROA names AS{origin} for {prefix} and allows its length.")
            }
            RpkiStatus::InvalidAsn => {
                let mut owners: Vec<u32> = self.roas.iter().map(|roa| roa.origin).collect();
                owners.sort_unstable();
                owners.dedup();
                let names: Vec<String> = owners.iter().map(|asn| format!("AS{asn}")).collect();
                let named = if names.is_empty() {
                    "another AS".to_string()
                } else {
                    list(&names)
                };
                format!("A ROA covers {prefix} and names {named}. AS{origin} is not named, so networks that drop invalid routes reject this announcement.")
            }
            RpkiStatus::InvalidLength => {
                let limit = self
                    .roas
                    .iter()
                    .map(|roa| roa.max_length)
                    .max()
                    .map_or_else(
                        || "the allowed maximum".to_string(),
                        |max| format!("/{max}"),
                    );
                format!("A ROA names AS{origin} with a maximum length of {limit}. {prefix} is longer, so networks that drop invalid routes reject this announcement.")
            }
            RpkiStatus::NotFound => {
                format!("No ROA covers {prefix}, so RPKI says nothing about this origin AS.")
            }
            RpkiStatus::Unavailable => "The validator gave no answer for this route.".to_string(),
        }
    }

    /// The three questions origin validation asks, in order.
    pub fn checks(&self) -> Vec<Check> {
        if self.status == RpkiStatus::Unavailable {
            return Vec::new();
        }
        let origin = self.origin;
        let found = !self.roas.is_empty();
        let mut checks = vec![Check {
            title: "ROA found",
            detail: if found {
                "A ROA covers this prefix.".to_string()
            } else {
                "No ROA covers this prefix.".to_string()
            },
            passed: found,
        }];
        if !found {
            return checks;
        }
        let named: Vec<&Roa> = self
            .roas
            .iter()
            .filter(|roa| roa.origin == origin)
            .collect();
        let matches = !named.is_empty();
        checks.push(Check {
            title: "Origin AS matches",
            detail: if matches {
                format!("A ROA names AS{origin}.")
            } else {
                format!("No covering ROA names AS{origin}.")
            },
            passed: matches,
        });
        if matches {
            let length: u32 = self
                .prefix
                .rsplit('/')
                .next()
                .and_then(|length| length.parse().ok())
                .unwrap_or(0);
            let limit = named.iter().map(|roa| roa.max_length).max().unwrap_or(0);
            let allowed = named.iter().any(|roa| length <= roa.max_length);
            checks.push(Check {
                title: "Prefix length allowed",
                detail: if allowed {
                    format!("/{length} is within the maximum of /{limit}.")
                } else {
                    format!("/{length} is longer than the maximum of /{limit}.")
                },
                passed: allowed,
            });
        }
        checks
    }
}

/// An AS seen originating a prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub asn: u32,
    #[serde(default)]
    pub holder: Option<String>,
    /// The routing registries that hold a route object for this prefix and origin, as "radb".
    #[serde(default)]
    pub route_objects: Vec<String>,
}

/// One address a hostname resolves to, with the route it falls in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub address: String,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub asn: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelatedPrefix {
    pub prefix: String,
    #[serde(default)]
    pub origin: Option<u32>,
    /// True for a more specific announcement inside the prefix, false for a covering one.
    pub is_more_specific: bool,
}

/// One distinct AS path toward the prefix, with how many collector peers reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Path {
    /// From the collector's peer to the origin AS, prepending collapsed.
    pub asns: Vec<u32>,
    pub peers: usize,
    /// The collectors whose peers reported it: "RRC01 · London, GB".
    #[serde(default)]
    pub collectors: Vec<String>,
    #[serde(default)]
    pub last_changed: Option<u64>,
    #[serde(default)]
    pub prepended: bool,
    #[serde(default)]
    pub communities: Vec<String>,
}

impl Path {
    /// The network the route enters the origin AS from. `None` when the peer is the origin
    /// AS's neighbor itself.
    pub fn upstream(&self) -> Option<u32> {
        (self.asns.len() >= 2).then(|| self.asns[self.asns.len() - 2])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub asn: u32,
    pub peers: usize,
}

/// A stretch of time in which one origin AS announced a prefix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub origin: u32,
    pub prefix: String,
    pub start: u64,
    pub end: u64,
    /// Full-table peers that saw it, averaged over the stretch.
    pub peers: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub start: u64,
    pub announcements: u64,
    pub withdrawals: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// Seen left of the AS in paths: toward the collectors, which suggests an upstream.
    Left,
    /// Seen right of it: toward the origins, which suggests a downstream.
    Right,
    Uncertain,
}

impl Side {
    pub const ALL: [Side; 3] = [Side::Left, Side::Right, Side::Uncertain];

    pub fn title(self) -> &'static str {
        match self {
            Side::Left => "Upstream",
            Side::Right => "Downstream",
            Side::Uncertain => "Unclear",
        }
    }

    pub fn from_provider(word: &str) -> Self {
        match word {
            "left" => Side::Left,
            "right" => Side::Right,
            _ => Side::Uncertain,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Neighbor {
    pub asn: u32,
    pub side: Side,
    /// RIPEstat's weight: how many paths the adjacency was seen in.
    pub power: u64,
}

/// What is known about one AS: registration, what it announces, who it is seen next to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BgpNetwork {
    pub asn: u32,
    #[serde(default)]
    pub holder: Option<String>,
    #[serde(default)]
    pub registry: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub block: Option<String>,
    #[serde(default)]
    pub announced: Option<bool>,
    #[serde(default)]
    pub v4_count: Option<u64>,
    #[serde(default)]
    pub v6_count: Option<u64>,
    /// Originated prefixes, capped at [`BgpNetwork::PREFIX_LIMIT`].
    #[serde(default)]
    pub prefixes: Vec<String>,
    /// Origin validation for the first prefixes of the list.
    #[serde(default)]
    pub validations: Vec<Validation>,
    #[serde(default)]
    pub neighbors: Vec<Neighbor>,
    #[serde(default)]
    pub upstream_count: Option<u64>,
    #[serde(default)]
    pub downstream_count: Option<u64>,
    #[serde(default)]
    pub uncertain_count: Option<u64>,
}

impl BgpNetwork {
    pub const PREFIX_LIMIT: usize = 300;
    pub const VALIDATION_LIMIT: usize = 40;
    pub const NEIGHBOR_LIMIT: usize = 40;

    pub fn new(asn: u32, holder: Option<String>) -> Self {
        Self {
            asn,
            holder,
            registry: None,
            country: None,
            block: None,
            announced: None,
            v4_count: None,
            v6_count: None,
            prefixes: Vec::new(),
            validations: Vec::new(),
            neighbors: Vec::new(),
            upstream_count: None,
            downstream_count: None,
            uncertain_count: None,
        }
    }

    pub fn validation_for(&self, prefix: &str) -> Option<&Validation> {
        self.validations
            .iter()
            .find(|validation| validation.prefix == prefix)
    }

    /// (valid, checked): how many of the validated prefixes have an authorizing ROA.
    pub fn coverage(&self) -> Option<(usize, usize)> {
        let checked: Vec<&Validation> = self
            .validations
            .iter()
            .filter(|validation| validation.status != RpkiStatus::Unavailable)
            .collect();
        if checked.is_empty() {
            return None;
        }
        let valid = checked
            .iter()
            .filter(|validation| validation.status == RpkiStatus::Valid)
            .count();
        Some((valid, checked.len()))
    }

    pub fn invalid_count(&self) -> usize {
        self.validations
            .iter()
            .filter(|validation| validation.status.is_invalid())
            .count()
    }

    pub fn prefix_total(&self) -> Option<u64> {
        if self.v4_count.is_none() && self.v6_count.is_none() {
            return None;
        }
        Some(self.v4_count.unwrap_or(0) + self.v6_count.unwrap_or(0))
    }

    pub fn side_count(&self, side: Side) -> Option<u64> {
        match side {
            Side::Left => self.upstream_count,
            Side::Right => self.downstream_count,
            Side::Uncertain => self.uncertain_count,
        }
    }

    /// The neighbors of one side, the ones seen most often first.
    pub fn side(&self, side: Side) -> Vec<&Neighbor> {
        let mut shown: Vec<&Neighbor> = self
            .neighbors
            .iter()
            .filter(|neighbor| neighbor.side == side)
            .collect();
        shown.sort_by_key(|neighbor| std::cmp::Reverse(neighbor.power));
        shown
    }

    /// The sentence under a side's list: how many are shown and what the side suggests.
    pub fn side_note(&self, side: Side, shown: usize) -> String {
        let total = self.side_count(side).unwrap_or(shown as u64);
        let lead = if total > shown as u64 {
            format!("The {shown} seen most often, of {total}. ")
        } else {
            String::new()
        };
        let asn = self.asn;
        lead + &match side {
            Side::Left => {
                format!("Seen before AS{asn} in AS paths, which suggests an upstream.")
            }
            Side::Right => {
                format!("Seen after AS{asn} in AS paths, which suggests a downstream.")
            }
            Side::Uncertain => {
                format!("Seen on both sides of AS{asn}, or too rarely to tell.")
            }
        }
    }

    /// "Example Net · 12 IPv4 and 3 IPv6 prefixes · 14 of 15 checked have a ROA".
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(holder) = &self.holder {
            parts.push(holder.clone());
        }
        if let (Some(v4), Some(v6)) = (self.v4_count, self.v6_count) {
            parts.push(format!(
                "{} IPv4 and {} IPv6 prefixes",
                count(v4),
                count(v6)
            ));
        }
        if let Some((valid, checked)) = self.coverage() {
            parts.push(format!("{valid} of {checked} checked have a ROA"));
        }
        if parts.is_empty() {
            format!("AS{}", self.asn)
        } else {
            parts.join(" \u{00B7} ")
        }
    }
}

/// Where one part of the result came from and when it was observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The RIPEstat data call, as "looking-glass". For a looking glass, its host.
    pub call: String,
    /// Who answered when it wasn't RIPEstat: "Alice-LG".
    #[serde(default)]
    pub provider: Option<String>,
    pub title: String,
    /// What it rests on: "RIPE RIS route collectors".
    pub basis: String,
    /// The time the data describes, when the provider says. Never the time it was fetched.
    #[serde(default)]
    pub observed_at: Option<u64>,
    #[serde(default)]
    pub retrieved_at: u64,
    #[serde(default)]
    pub failed: bool,
}

impl Source {
    /// "RIPEstat looking-glass · observed 2026-11-15 08:00 UTC · retrieved …".
    pub fn line(&self) -> String {
        let mut parts = vec![format!(
            "{} {}",
            self.provider.as_deref().unwrap_or("RIPEstat"),
            self.call
        )];
        if let Some(observed) = self.observed_at {
            parts.push(format!("observed {}", date_time(observed)));
        }
        parts.push(format!("retrieved {}", date_time(self.retrieved_at)));
        parts.join(" \u{00B7} ")
    }
}

/// "2026-11-15 08:00 UTC".
pub fn date_time(seconds: u64) -> String {
    let stamp = format::iso8601(seconds);
    format!("{} {} UTC", &stamp[..10], &stamp[11..16])
}

/// "2026-11-15".
pub fn date(seconds: u64) -> String {
    format::iso8601(seconds)[..10].to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    Good,
    Attention,
    Bad,
    Neutral,
}

/// The finding at the top of Overview, then the evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub grade: Grade,
    pub title: String,
    pub detail: String,
    /// The detail's first sentence, for a result in one line. Kept apart because a holder's
    /// name can hold a full stop of its own, as in "Example, Inc.".
    pub lead: String,
}

impl Verdict {
    fn new(grade: Grade, title: impl Into<String>, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let lead = match detail.split_once(". ") {
            Some((first, _)) => format!("{first}."),
            None => detail.clone(),
        };
        Self {
            grade,
            title: title.into(),
            detail,
            lead,
        }
    }

    /// A verdict whose first sentence is known, so it is never cut inside a name.
    fn led(mut self, lead: &str) -> Self {
        self.lead = lead.to_string();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BgpResult {
    pub kind: TargetKind,
    pub query: String,
    #[serde(default)]
    pub addresses: Vec<Address>,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub announced: Option<bool>,
    #[serde(default)]
    pub origins: Vec<Origin>,
    #[serde(default)]
    pub validations: Vec<Validation>,
    #[serde(default)]
    pub registry: Option<String>,
    #[serde(default)]
    pub seeing: Option<u64>,
    #[serde(default)]
    pub total_peers: Option<u64>,
    #[serde(default)]
    pub first_seen: Option<u64>,
    #[serde(default)]
    pub related: Vec<RelatedPrefix>,
    #[serde(default)]
    pub paths: Vec<Path>,
    #[serde(default)]
    pub path_count: usize,
    #[serde(default)]
    pub path_peers: usize,
    #[serde(default)]
    pub collector_count: usize,
    #[serde(default)]
    pub upstreams: Vec<Share>,
    /// Over every peer's path, not only the stored ones: the fewest networks, the median, and
    /// how many peers are next to the origin AS themselves.
    #[serde(default)]
    pub shortest_path: usize,
    #[serde(default)]
    pub typical_path: usize,
    #[serde(default)]
    pub direct_peers: usize,
    /// Holder names of the AS numbers that appear in paths and neighbor lists.
    #[serde(default)]
    pub names: BTreeMap<u32, String>,
    #[serde(default)]
    pub spans: Vec<Span>,
    #[serde(default)]
    pub activity: Vec<Activity>,
    #[serde(default)]
    pub network: Option<BgpNetwork>,
    /// What the user's looking glasses hold for the prefix.
    #[serde(default)]
    pub glasses: Vec<GlassResult>,
    #[serde(default)]
    pub sources: Vec<Source>,
    /// Why no public route is expected: a private or reserved address.
    #[serde(default)]
    pub unrouted_reason: Option<String>,
    /// Parts still loading. Zero once the lookup has settled.
    #[serde(default)]
    pub pending: i32,
    /// Sessions that arrived before their looking glass answered.
    #[serde(skip)]
    waiting_sessions: BTreeMap<String, GlassSessions>,
}

impl BgpResult {
    pub const PATH_LIMIT: usize = 80;

    pub fn new(kind: TargetKind, query: &str) -> Self {
        Self {
            kind,
            query: query.to_string(),
            addresses: Vec::new(),
            address: None,
            prefix: None,
            announced: None,
            origins: Vec::new(),
            validations: Vec::new(),
            registry: None,
            seeing: None,
            total_peers: None,
            first_seen: None,
            related: Vec::new(),
            paths: Vec::new(),
            path_count: 0,
            path_peers: 0,
            collector_count: 0,
            upstreams: Vec::new(),
            shortest_path: 0,
            typical_path: 0,
            direct_peers: 0,
            names: BTreeMap::new(),
            spans: Vec::new(),
            activity: Vec::new(),
            network: None,
            glasses: Vec::new(),
            sources: Vec::new(),
            unrouted_reason: None,
            pending: 0,
            waiting_sessions: BTreeMap::new(),
        }
    }

    /// Folds one part of a lookup in. The ending events are the run's to read.
    pub fn apply(&mut self, event: &BgpEvent) {
        match event {
            BgpEvent::Plan(parts) => self.pending += parts,
            BgpEvent::Addresses(addresses, selected) => {
                self.addresses = addresses.clone();
                self.address = selected.clone();
            }
            BgpEvent::Unrouted(reason) => self.unrouted_reason = Some(reason.clone()),
            BgpEvent::Route(prefix) => self.prefix = Some(prefix.clone()),
            BgpEvent::Overview {
                announced,
                origins,
                registry,
            } => {
                self.announced = *announced;
                // Route objects may have arrived first, with the routing status.
                let known = std::mem::take(&mut self.origins);
                self.origins = origins
                    .iter()
                    .map(|origin| {
                        let mut origin = origin.clone();
                        if let Some(earlier) = known.iter().find(|known| known.asn == origin.asn) {
                            origin.route_objects = earlier.route_objects.clone();
                        }
                        origin
                    })
                    .collect();
                self.registry = registry.clone();
            }
            BgpEvent::Status {
                seeing,
                total,
                first_seen,
                related,
                route_objects,
            } => {
                self.seeing = *seeing;
                self.total_peers = *total;
                self.first_seen = *first_seen;
                self.related = related.clone();
                for (asn, objects) in route_objects {
                    if let Some(origin) = self.origins.iter_mut().find(|origin| origin.asn == *asn)
                    {
                        origin.route_objects = objects.clone();
                    } else if self.origins.is_empty() || self.announced.is_none() {
                        self.origins.push(Origin {
                            asn: *asn,
                            holder: None,
                            route_objects: objects.clone(),
                        });
                    }
                }
            }
            BgpEvent::Validation(validation) => {
                self.validations
                    .retain(|known| !known.is(&validation.prefix, validation.origin));
                self.validations.push(validation.clone());
            }
            BgpEvent::Paths(paths, folded) => {
                self.paths = paths.clone();
                self.path_count = folded.paths;
                self.path_peers = folded.peers;
                self.collector_count = folded.collectors;
                self.upstreams = folded.upstreams.clone();
                self.shortest_path = folded.shortest;
                self.typical_path = folded.typical;
                self.direct_peers = folded.direct;
            }
            BgpEvent::Names(names) => {
                for (asn, name) in names {
                    self.names.insert(*asn, name.clone());
                }
            }
            BgpEvent::History(spans) => self.spans = spans.clone(),
            BgpEvent::Activity(activity) => self.activity = activity.clone(),
            BgpEvent::Network(asn) => {
                let holder = self
                    .origins
                    .iter()
                    .find(|origin| origin.asn == *asn)
                    .and_then(|origin| origin.holder.clone());
                self.network = Some(BgpNetwork::new(*asn, holder));
            }
            BgpEvent::NetworkPart(part) => {
                let Some(network) = self.network.as_mut() else {
                    return;
                };
                match part {
                    NetworkPart::Identity {
                        holder,
                        announced,
                        block,
                    } => {
                        if holder.is_some() {
                            network.holder = holder.clone();
                        }
                        network.announced = *announced;
                        network.block = block.clone();
                    }
                    NetworkPart::Registry(name) => network.registry = Some(name.clone()),
                    NetworkPart::Country(code) => network.country = Some(code.clone()),
                    NetworkPart::Neighbors {
                        list,
                        upstream,
                        downstream,
                        uncertain,
                    } => {
                        network.neighbors = list.clone();
                        network.upstream_count = *upstream;
                        network.downstream_count = *downstream;
                        network.uncertain_count = *uncertain;
                    }
                    NetworkPart::Prefixes { list, v4, v6 } => {
                        network.prefixes = list.clone();
                        network.v4_count = *v4;
                        network.v6_count = *v6;
                    }
                }
            }
            BgpEvent::PrefixValidation(validation) => {
                if let Some(network) = self.network.as_mut() {
                    network
                        .validations
                        .retain(|known| !known.is(&validation.prefix, validation.origin));
                    network.validations.push(validation.clone());
                }
            }
            BgpEvent::Glass(glass) => {
                let mut glass = glass.clone();
                if let Some(sessions) = self.waiting_sessions.get(&glass.host) {
                    glass.sessions = Some(sessions.clone());
                }
                self.glasses.retain(|known| known.host != glass.host);
                self.glasses.push(glass);
            }
            BgpEvent::GlassSessions(host, sessions) => {
                self.waiting_sessions.insert(host.clone(), sessions.clone());
                if let Some(glass) = self.glasses.iter_mut().find(|glass| &glass.host == host) {
                    glass.sessions = Some(sessions.clone());
                }
            }
            BgpEvent::Source(source) => {
                self.sources.retain(|known| known.call != source.call);
                self.sources.push(source.clone());
                self.pending -= 1;
            }
            BgpEvent::Finished | BgpEvent::Stopped | BgpEvent::Failed(_) => self.pending = 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.prefix.is_none()
            && self.network.is_none()
            && self.unrouted_reason.is_none()
            && self.addresses.is_empty()
    }

    pub fn is_network(&self) -> bool {
        self.kind == TargetKind::Asn
    }

    pub fn origin(&self) -> Option<&Origin> {
        self.origins.first()
    }

    pub fn name_of(&self, asn: u32) -> Option<&str> {
        if let Some(name) = self.names.get(&asn) {
            return Some(name);
        }
        if let Some(holder) = self
            .origins
            .iter()
            .find(|origin| origin.asn == asn)
            .and_then(|origin| origin.holder.as_deref())
        {
            return Some(holder);
        }
        self.network
            .as_ref()
            .filter(|network| network.asn == asn)
            .and_then(|network| network.holder.as_deref())
    }

    /// "AS64500 Example Net", or the number alone when the holder is not known.
    pub fn label(&self, asn: u32) -> String {
        match self.name_of(asn) {
            Some(name) => format!("AS{asn} {name}"),
            None => format!("AS{asn}"),
        }
    }

    pub fn validation_for(&self, origin: u32) -> Option<&Validation> {
        let prefix = self.prefix.as_deref()?;
        self.validations
            .iter()
            .find(|validation| validation.is(prefix, origin))
    }

    /// The validation of the route's first origin AS.
    pub fn validation(&self) -> Option<&Validation> {
        self.validation_for(self.origin()?.asn)
    }

    /// 0...1: the share of collector peers that see the prefix.
    pub fn visibility(&self) -> Option<f64> {
        match (self.seeing, self.total_peers) {
            (Some(seeing), Some(total)) if total > 0 => Some(seeing as f64 / total as f64),
            _ => None,
        }
    }

    /// The share of the paths' peers, 0...1.
    pub fn share(&self, peers: usize) -> f64 {
        if self.path_peers == 0 {
            0.0
        } else {
            peers as f64 / self.path_peers as f64
        }
    }

    /// "3 of 10 sources" while a lookup is live.
    pub fn progress(&self) -> String {
        let total = self.sources.len() as i32 + self.pending.max(0);
        if total <= 0 {
            return "Finding the route".to_string();
        }
        format!("{} of {total} sources", self.sources.len())
    }

    /// The finding, then the evidence. Observations only: an invalid route is named as
    /// invalid, never as a hijack.
    pub fn verdict(&self) -> Verdict {
        if let Some(reason) = &self.unrouted_reason {
            return Verdict::new(Grade::Neutral, "Not publicly routed", reason.clone());
        }
        if self.is_network() {
            return self.network_verdict();
        }
        let Some(prefix) = &self.prefix else {
            // An address no announced prefix covers has no route to name.
            if self.announced == Some(false) {
                return Verdict::new(Grade::Attention, "No route observed", format!("No collector reported a route that covers {}. Collectors see only what their peers send, so the address may still be reachable.", self.address.as_deref().unwrap_or(&self.query)));
            }
            return Verdict::new(
                Grade::Neutral,
                "Looking up",
                format!("Finding the route for {}.", self.query),
            );
        };
        let Some(origin) = self.origin() else {
            if self.announced == Some(false) {
                return Verdict::new(Grade::Attention, "No route observed", format!("No collector reported an announcement for {prefix}. Collectors see only what their peers send, so the address may still be reachable."));
            }
            return Verdict::new(
                Grade::Neutral,
                "Looking up",
                format!("Finding who announces {prefix}."),
            );
        };
        let who = match &origin.holder {
            Some(holder) => format!("AS{} {holder}", origin.asn),
            None => format!("AS{}", origin.asn),
        };
        let seen = match (self.seeing, self.total_peers) {
            (Some(seeing), Some(total)) => {
                format!(", seen by {seeing} of {total} collector peers")
            }
            _ => String::new(),
        };
        let lead = format!("{prefix} is announced by {who}{seen}.");
        if self.origins.len() > 1 {
            let others: Vec<String> = self.origins[1..]
                .iter()
                .map(|origin| format!("AS{}", origin.asn))
                .collect();
            let verb = if others.len() == 1 {
                "announces"
            } else {
                "announce"
            };
            return Verdict::new(
                Grade::Attention,
                format!("{} origin ASes observed", self.origins.len()),
                format!(
                    "{lead} {} {verb} it too. Anycast and migrations look like this.",
                    list(&others)
                ),
            )
            .led(&lead);
        }
        let Some(validation) = self.validation() else {
            return Verdict::new(Grade::Neutral, "Routed", format!("{lead} Checking RPKI."))
                .led(&lead);
        };
        let verdict = match validation.status {
            RpkiStatus::Valid => {
                let detail = format!("{lead} A ROA authorizes this origin AS.");
                if self.visibility().is_some_and(|visibility| visibility < 0.8) {
                    Verdict::new(Grade::Attention, "Authorized, partly visible", detail)
                } else {
                    Verdict::new(Grade::Good, "Routed and authorized", detail)
                }
            }
            RpkiStatus::InvalidAsn => Verdict::new(
                Grade::Bad,
                "Wrong origin ASN",
                format!("{lead} {}", validation.explanation()),
            ),
            RpkiStatus::InvalidLength => Verdict::new(
                Grade::Bad,
                "Prefix too specific",
                format!("{lead} {}", validation.explanation()),
            ),
            RpkiStatus::NotFound => Verdict::new(
                Grade::Neutral,
                "Routed, no ROA found",
                format!("{lead} No ROA covers it, so RPKI says nothing about this origin AS."),
            ),
            RpkiStatus::Unavailable => Verdict::new(
                Grade::Neutral,
                "Routed",
                format!("{lead} RPKI could not be checked."),
            ),
        };
        verdict.led(&lead)
    }

    fn network_verdict(&self) -> Verdict {
        let Some(network) = &self.network else {
            return Verdict::new(
                Grade::Neutral,
                "Looking up",
                format!("Finding {}.", self.query),
            );
        };
        let asn = network.asn;
        let who = network.holder.clone().unwrap_or_else(|| format!("AS{asn}"));
        let Some(total) = network.prefix_total() else {
            return Verdict::new(
                Grade::Neutral,
                who,
                format!("Counting what AS{asn} announces."),
            );
        };
        if total == 0 {
            return Verdict::new(Grade::Neutral, "Originates no prefixes", format!("Collectors see no prefix originated by AS{asn}. It may still carry other networks' routes."));
        }
        let counts = format!(
            "{} IPv4 and {} IPv6 prefixes",
            count(network.v4_count.unwrap_or(0)),
            count(network.v6_count.unwrap_or(0))
        );
        let title = format!("Originates {}", counted(total, "prefix", "prefixes"));
        let Some((valid, checked)) = network.coverage() else {
            return Verdict::new(Grade::Neutral, title, format!("{who} announces {counts}."))
                .led(&format!("{who} announces {counts}."));
        };
        let have = plural(valid, "has", "have");
        let checked_line = format!("Of {checked} checked, {valid} {have} an authorizing ROA");
        let invalid = network.invalid_count();
        if invalid > 0 {
            return Verdict::new(
                Grade::Bad,
                format!("{invalid} invalid {}", plural(invalid, "route", "routes")),
                format!(
                    "{who} announces {counts}. {checked_line} and {invalid} {} origin validation.",
                    plural(invalid, "fails", "fail")
                ),
            )
            .led(&format!("{who} announces {counts}."));
        }
        let grade = if valid == checked {
            Grade::Good
        } else {
            Grade::Neutral
        };
        Verdict::new(
            grade,
            title,
            format!("{who} announces {counts}. {checked_line}."),
        )
        .led(&format!("{who} announces {counts}."))
    }

    /// One line for lists: "AS64500 · 192.0.2.0/24 · Valid".
    pub fn summary(&self) -> String {
        if self.unrouted_reason.is_some() {
            return "Not publicly routed".to_string();
        }
        if self.is_network() {
            let mut parts = Vec::new();
            if let Some(network) = &self.network {
                if let Some(holder) = &network.holder {
                    parts.push(holder.clone());
                }
                if let Some(total) = network.prefix_total() {
                    parts.push(counted(total, "prefix", "prefixes"));
                }
            }
            return if parts.is_empty() {
                "No result".to_string()
            } else {
                parts.join(" \u{00B7} ")
            };
        }
        let mut parts = Vec::new();
        if let Some(origin) = self.origin() {
            parts.push(format!("AS{}", origin.asn));
        }
        if self.kind != TargetKind::Prefix {
            if let Some(prefix) = &self.prefix {
                parts.push(prefix.clone());
            }
        }
        if let Some(validation) = self.validation() {
            parts.push(validation.status.badge().to_string());
        }
        if parts.is_empty() {
            return if self.announced == Some(false) {
                "No route observed".to_string()
            } else {
                "No result".to_string()
            };
        }
        parts.join(" \u{00B7} ")
    }

    /// The status line of a finished lookup: "AS64500 · Valid · 320/320 peers".
    pub fn headline(&self) -> String {
        if self.is_network() || self.unrouted_reason.is_some() {
            return self.summary();
        }
        let mut parts = Vec::new();
        if let Some(origin) = self.origin() {
            parts.push(format!("AS{}", origin.asn));
        }
        if let Some(validation) = self.validation() {
            parts.push(validation.status.badge().to_string());
        }
        if let (Some(seeing), Some(total)) = (self.seeing, self.total_peers) {
            parts.push(format!("{seeing}/{total} peers"));
        }
        if parts.is_empty() {
            self.summary()
        } else {
            parts.join(" \u{00B7} ")
        }
    }

    /// The one fact the path figures don't carry: which network carries the route for the
    /// most collector peers.
    pub fn path_summary(&self) -> Option<String> {
        let top = self.upstreams.first()?;
        if self.path_peers == 0 {
            return None;
        }
        Some(format!(
            "{} is the last network before the origin AS for {} of collector peers, more than any other.",
            self.label(top.asn),
            format::percent(self.share(top.peers))
        ))
    }

    /// "from Example Net · 12 peers", under one AS path.
    pub fn path_line(&self, path: &Path) -> String {
        let mut parts = Vec::new();
        if path.asns.len() > 1 {
            if let Some(name) = path.asns.first().and_then(|first| self.name_of(*first)) {
                parts.push(format!("from {name}"));
            }
        }
        parts.push(format!(
            "{} {}",
            path.peers,
            plural(path.peers, "peer", "peers")
        ));
        parts.join(" \u{00B7} ")
    }

    /// The changes of the last `days` days, newest first.
    pub fn changes(&self, days: u64, now: u64) -> Vec<Change> {
        Change::derive(&self.spans, now.saturating_sub(days * 86_400), now)
    }

    /// One lane per origin AS for a timeline that starts at `since`: the stretches it announced
    /// the prefix, the origin AS announcing it now first. Stretches that touch are one, so a
    /// gap in a lane is a gap in the announcement.
    pub fn lanes(&self, since: u64) -> Vec<(u32, Vec<(u64, u64)>)> {
        let mut shown: Vec<&Span> = self.spans.iter().filter(|span| span.end > since).collect();
        shown.sort_by_key(|span| std::cmp::Reverse(span.end));
        let mut origins: Vec<u32> = Vec::new();
        for span in &shown {
            if !origins.contains(&span.origin) {
                origins.push(span.origin);
            }
        }
        shown.sort_by_key(|span| span.start);
        origins
            .into_iter()
            .map(|origin| {
                let mut stretches: Vec<(u64, u64)> = Vec::new();
                for span in shown.iter().filter(|span| span.origin == origin) {
                    let start = span.start.max(since);
                    match stretches.last_mut() {
                        Some(last) if start.saturating_sub(last.1) <= Change::GAP => {
                            last.1 = last.1.max(span.end);
                        }
                        _ => stretches.push((start, span.end)),
                    }
                }
                (origin, stretches)
            })
            .collect()
    }

    /// Full-table peers seeing the prefix from `since` to `now`, in `cells` equal steps. Every
    /// origin AS's peers count together.
    pub fn peers_over(&self, since: u64, now: u64, cells: usize) -> Vec<f64> {
        let step = now.saturating_sub(since) as f64 / cells.max(1) as f64;
        (0..cells)
            .map(|cell| {
                let moment = since as f64 + (cell as f64 + 0.5) * step;
                self.spans
                    .iter()
                    .filter(|span| (span.start as f64) <= moment && moment < span.end as f64)
                    .map(|span| span.peers)
                    .sum()
            })
            .collect()
    }

    /// A change's detail with the holder of its origin AS, when that is known.
    pub fn change_line(&self, change: &Change) -> String {
        match self.name_of(change.origin) {
            Some(name) if name.ends_with('.') => format!("{} {name}", change.detail),
            Some(name) => format!("{} {name}.", change.detail),
            None => change.detail.clone(),
        }
    }

    /// Announcements and withdrawals collectors saw over the activity window.
    pub fn updates(&self) -> u64 {
        self.activity
            .iter()
            .map(|bucket| bucket.announcements + bucket.withdrawals)
            .sum()
    }

    /// The text snapshot Copy Results and a pipe get, with the source of every part.
    pub fn export(&self, target: &str, started_at: u64) -> String {
        let mut lines = vec![
            format!("bgp {target}, started {}", date_time(started_at)),
            self.headline(),
            String::new(),
        ];
        let verdict = self.verdict();
        lines.push(format!("{}. {}", verdict.title, verdict.detail));
        if let Some(prefix) = &self.prefix {
            lines.push(String::new());
            lines.push(format!("prefix    {prefix}"));
            for origin in &self.origins {
                let rpki = self
                    .validation_for(origin.asn)
                    .map_or("not checked", |validation| validation.status.title());
                lines.push(format!(
                    "origin    AS{} {}  rpki: {rpki}",
                    origin.asn,
                    origin.holder.as_deref().unwrap_or("")
                ));
            }
            if let (Some(seeing), Some(total)) = (self.seeing, self.total_peers) {
                lines.push(format!("seen by   {seeing} of {total} collector peers"));
            }
        }
        if !self.paths.is_empty() {
            lines.push(String::new());
            lines.push(format!(
                "as paths ({} distinct, {} peers)",
                self.path_count, self.path_peers
            ));
            for path in self.paths.iter().take(10) {
                let asns: Vec<String> = path.asns.iter().map(u32::to_string).collect();
                lines.push(format!("{:>4}  {}", path.peers, asns.join(" ")));
            }
        }
        for glass in &self.glasses {
            lines.push(String::new());
            lines.push(format!("{} ({})", glass.name, glass.summary()));
            for path in glass.paths().iter().take(10) {
                let asns: Vec<String> = path.asns.iter().map(u32::to_string).collect();
                let state = if path.is_filtered {
                    "  filtered"
                } else if path.is_best {
                    "  best"
                } else {
                    ""
                };
                lines.push(format!(
                    "{:>4}  {}{state}",
                    path.route_servers.len(),
                    asns.join(" ")
                ));
            }
        }
        if let Some(network) = &self.network {
            lines.push(String::new());
            lines.push(
                format!(
                    "AS{} {}",
                    network.asn,
                    network.holder.as_deref().unwrap_or("")
                )
                .trim_end()
                .to_string(),
            );
            if let (Some(v4), Some(v6)) = (network.v4_count, network.v6_count) {
                lines.push(format!("originates {v4} IPv4 and {v6} IPv6 prefixes"));
            }
            if let Some((valid, checked)) = network.coverage() {
                lines.push(format!(
                    "rpki: {valid} of {checked} checked prefixes authorized"
                ));
            }
        }
        lines.push(String::new());
        for source in self.sources.iter().filter(|source| !source.failed) {
            let observed = source
                .observed_at
                .map(|observed| format!(", observed {}", date_time(observed)))
                .unwrap_or_default();
            lines.push(format!(
                ";; {}: {} {}{observed}",
                source.title,
                source.provider.as_deref().unwrap_or("RIPEstat"),
                source.call
            ));
        }
        lines.join("\n")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    OriginAppeared,
    OriginGone,
    VisibilityDropped,
    VisibilityRecovered,
}

/// What changed in who announces a prefix, read off the origin timelines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub date: u64,
    pub origin: u32,
    pub prefix: String,
    pub detail: String,
}

impl Change {
    /// A gap longer than this between two stretches is a withdrawal and a return.
    pub const GAP: u64 = 12 * 3600;
    /// How far back the origin timelines are asked for.
    pub const WINDOW: u64 = 90 * 86_400;

    pub fn title(&self) -> &'static str {
        match self.kind {
            ChangeKind::OriginAppeared => "New origin AS observed",
            ChangeKind::OriginGone => "Origin AS no longer seen",
            ChangeKind::VisibilityDropped => "Visibility dropped",
            ChangeKind::VisibilityRecovered => "Visibility recovered",
        }
    }

    pub fn is_good(&self) -> bool {
        matches!(
            self.kind,
            ChangeKind::OriginAppeared | ChangeKind::VisibilityRecovered
        )
    }

    /// Changes after `since`, newest first. Spans that touch (one ends where the next begins)
    /// are one announcement. A gap longer than [`Change::GAP`] is a withdrawal and a return. A
    /// drop of more than a fifth of the peers between touching spans is a visibility change.
    pub fn derive(spans: &[Span], since: u64, now: u64) -> Vec<Change> {
        let mut lanes: BTreeMap<(u32, &str), Vec<&Span>> = BTreeMap::new();
        for span in spans {
            lanes
                .entry((span.origin, span.prefix.as_str()))
                .or_default()
                .push(span);
        }
        let mut changes = Vec::new();
        for lane in lanes.values_mut() {
            lane.sort_by_key(|span| span.start);
            for (index, span) in lane.iter().enumerate() {
                let previous = index.checked_sub(1).map(|before| lane[before]);
                let next = lane.get(index + 1);
                let continues = previous
                    .is_some_and(|before| span.start.saturating_sub(before.end) <= Self::GAP);
                let change = |kind, date, detail| Change {
                    kind,
                    date,
                    origin: span.origin,
                    prefix: span.prefix.clone(),
                    detail,
                };
                // The timelines begin where the asked window begins. A stretch that starts
                // there was being announced before it, so it is no new origin AS.
                let carried_in = span.start <= now.saturating_sub(Self::WINDOW) + Self::GAP;
                if !continues {
                    if !carried_in {
                        changes.push(change(
                            ChangeKind::OriginAppeared,
                            span.start,
                            format!("{} announced by AS{}.", span.prefix, span.origin),
                        ));
                    }
                } else if let Some(before) = previous.filter(|before| before.peers > 0.0) {
                    let shift = (span.peers - before.peers) / before.peers;
                    if shift <= -0.2 {
                        changes.push(change(
                            ChangeKind::VisibilityDropped,
                            span.start,
                            format!(
                                "Seen by {} peers, down from {}.",
                                span.peers as u64, before.peers as u64
                            ),
                        ));
                    } else if shift >= 0.25 {
                        changes.push(change(
                            ChangeKind::VisibilityRecovered,
                            span.start,
                            format!(
                                "Seen by {} peers, up from {}.",
                                span.peers as u64, before.peers as u64
                            ),
                        ));
                    }
                }
                let resumes =
                    next.is_some_and(|after| after.start.saturating_sub(span.end) <= Self::GAP);
                if !resumes && now.saturating_sub(span.end) > Self::GAP {
                    changes.push(change(
                        ChangeKind::OriginGone,
                        span.end,
                        format!("AS{} stopped announcing {}.", span.origin, span.prefix),
                    ));
                }
            }
        }
        changes.retain(|change| change.date > since);
        changes.sort_by_key(|change| std::cmp::Reverse(change.date));
        changes
    }
}

/// A traceroute's networks laid against the routes collectors see. Collectors sit in other
/// networks than this device, so a difference is normal. The sentence says only what both show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceComparison {
    /// The networks the traceroute crossed, in order, repeats collapsed.
    pub trace_asns: Vec<u32>,
    pub sentence: String,
    pub matches: bool,
}

impl TraceComparison {
    pub fn new(trace: &Run, result: &BgpResult) -> Option<Self> {
        let origin = result.origin()?.asn;
        let mut sequence: Vec<u32> = Vec::new();
        let mut trace_names: BTreeMap<u32, String> = BTreeMap::new();
        for hop in trace.path() {
            let Some(network) = hop.address().and_then(|address| trace.network(address)) else {
                continue;
            };
            trace_names
                .entry(network.number)
                .or_insert_with(|| network.name.clone());
            if sequence.last() != Some(&network.number) {
                sequence.push(network.number);
            }
        }
        let last = *sequence.last()?;
        let label = |asn: u32| match result
            .name_of(asn)
            .or(trace_names.get(&asn).map(String::as_str))
        {
            Some(name) => format!("AS{asn} {name}"),
            None => format!("AS{asn}"),
        };
        if last != origin {
            return Some(Self {
                sentence: format!(
                    "Your traceroute ended in {}. The origin AS is {}. The last hops may not have answered.",
                    label(last),
                    label(origin)
                ),
                trace_asns: sequence,
                matches: false,
            });
        }
        if sequence.len() < 2 {
            return Some(Self {
                sentence: format!(
                    "Your traceroute reached {}. No other network answered on the way.",
                    label(origin)
                ),
                trace_asns: sequence,
                matches: true,
            });
        }
        let entry = sequence[sequence.len() - 2];
        let peers = result
            .upstreams
            .iter()
            .find(|share| share.asn == entry)
            .map_or(0, |share| share.peers);
        let (matches, sentence) = if peers > 0 && result.path_peers > 0 {
            (
                true,
                format!(
                    "Your traceroute entered {} from {}. {} of collector peers see the route through the same network.",
                    label(origin),
                    label(entry),
                    format::percent(result.share(peers))
                ),
            )
        } else {
            (
                false,
                format!(
                    "Your traceroute entered {} from {}. No collector peer reports that adjacency. Collectors sit in other networks, so their paths can differ from yours.",
                    label(origin),
                    label(entry)
                ),
            )
        };
        Some(Self {
            trace_asns: sequence,
            sentence,
            matches,
        })
    }
}

/// Why a looking glass has no route, with the evidence it gives: the origin AS's sessions.
pub fn no_route_line(glass: &GlassResult, prefix: &str) -> String {
    if glass.kind.has_routers() {
        return format!("Its routers hold no BGP route for {prefix}.");
    }
    let Some(sessions) = &glass.sessions else {
        return format!("Its route servers hold no route for {prefix}.");
    };
    let network = format!("AS{}", sessions.asn);
    let sessions_count = format!(
        "{} {}",
        sessions.total,
        plural(sessions.total, "session", "sessions")
    );
    if sessions.total == 0 {
        return format!(
            "{network} has no session with its route servers, so they hold no route for {prefix}."
        );
    }
    if sessions.received == 0 {
        return format!(
            "{network} has {sessions_count} with its route servers and sends them no routes."
        );
    }
    format!("{network} has {sessions_count} with its route servers. {prefix} is not among the routes it sends them.")
}

/// A result to test rendering against, here and in the crates that draw it.
#[doc(hidden)]
pub mod fixtures {
    use super::*;
    use crate::bgp::lookup::Folded;

    pub fn valid(prefix: &str, origin: u32) -> Validation {
        Validation {
            prefix: prefix.to_string(),
            origin,
            status: RpkiStatus::Valid,
            roas: vec![Roa {
                origin,
                prefix: prefix.to_string(),
                max_length: 24,
                validity: "valid".to_string(),
            }],
            checked_at: 1_800_000_000,
        }
    }

    /// A finished lookup of a hostname: one origin AS, a valid ROA, three paths.
    pub fn routed() -> BgpResult {
        let mut result = BgpResult::new(TargetKind::Host, "example.com");
        let events = [
            BgpEvent::Addresses(
                vec![Address {
                    address: "192.0.2.10".to_string(),
                    prefix: Some("192.0.2.0/24".to_string()),
                    asn: Some(64500),
                }],
                Some("192.0.2.10".to_string()),
            ),
            BgpEvent::Route("192.0.2.0/24".to_string()),
            BgpEvent::Plan(10),
            BgpEvent::Status {
                seeing: Some(318),
                total: Some(320),
                first_seen: Some(1_500_000_000),
                related: vec![RelatedPrefix {
                    prefix: "192.0.2.0/23".to_string(),
                    origin: Some(64500),
                    is_more_specific: false,
                }],
                route_objects: BTreeMap::from([(64500, vec!["radb".to_string()])]),
            },
            BgpEvent::Overview {
                announced: Some(true),
                origins: vec![Origin {
                    asn: 64500,
                    holder: Some("Example Net".to_string()),
                    route_objects: Vec::new(),
                }],
                registry: Some("RIPE NCC".to_string()),
            },
            BgpEvent::Validation(valid("192.0.2.0/24", 64500)),
            BgpEvent::Paths(
                vec![
                    Path {
                        asns: vec![64510, 64501, 64500],
                        peers: 200,
                        collectors: vec!["RRC01 \u{00B7} London, GB".to_string()],
                        last_changed: Some(1_799_000_000),
                        prepended: false,
                        communities: vec!["64501:100".to_string()],
                    },
                    Path {
                        asns: vec![64511, 64502, 64500],
                        peers: 100,
                        collectors: vec!["RRC03 \u{00B7} Amsterdam, NL".to_string()],
                        last_changed: None,
                        prepended: true,
                        communities: Vec::new(),
                    },
                    Path {
                        asns: vec![64501, 64500],
                        peers: 18,
                        collectors: vec!["RRC01 \u{00B7} London, GB".to_string()],
                        last_changed: None,
                        prepended: false,
                        communities: Vec::new(),
                    },
                ],
                Folded {
                    paths: 3,
                    peers: 318,
                    collectors: 2,
                    upstreams: vec![
                        Share {
                            asn: 64501,
                            peers: 218,
                        },
                        Share {
                            asn: 64502,
                            peers: 100,
                        },
                    ],
                    shortest: 2,
                    typical: 3,
                    direct: 18,
                },
            ),
            BgpEvent::Names(BTreeMap::from([
                (64501, "Transit One".to_string()),
                (64502, "Transit Two".to_string()),
            ])),
            BgpEvent::Network(64500),
            BgpEvent::NetworkPart(NetworkPart::Prefixes {
                list: vec!["192.0.2.0/24".to_string(), "2001:db8::/32".to_string()],
                v4: Some(1),
                v6: Some(1),
            }),
            BgpEvent::PrefixValidation(valid("192.0.2.0/24", 64500)),
            BgpEvent::Source(Source {
                call: "prefix-overview".to_string(),
                provider: None,
                title: "Origin AS and holder".to_string(),
                basis: "RIPE RIS route collectors".to_string(),
                observed_at: Some(1_799_990_000),
                retrieved_at: 1_800_000_000,
                failed: false,
            }),
            BgpEvent::Finished,
        ];
        for event in &events {
            result.apply(event);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{routed, valid};
    use super::*;

    #[test]
    fn a_target_is_a_host_an_address_a_prefix_or_a_network() {
        let kinds = [
            ("example.com", TargetKind::Host, "example.com"),
            ("https://Example.com/path", TargetKind::Host, "example.com"),
            ("192.0.2.1", TargetKind::Address, "192.0.2.1"),
            ("2001:db8::1", TargetKind::Address, "2001:db8::1"),
            ("192.0.2.0/24", TargetKind::Prefix, "192.0.2.0/24"),
            ("2001:db8::/32", TargetKind::Prefix, "2001:db8::/32"),
            ("AS64500", TargetKind::Asn, "AS64500"),
            ("as64500", TargetKind::Asn, "AS64500"),
            ("64500", TargetKind::Asn, "AS64500"),
        ];
        for (typed, kind, canonical) in kinds {
            let target = Target::parse(typed).unwrap();
            assert_eq!((target.kind, target.canonical.as_str()), (kind, canonical));
        }
        assert_eq!(Target::parse("AS64500").unwrap().asn(), Some(64500));
        assert_eq!(Target::parse("  "), None);
        // A length no address has is read as a path after an address.
        assert_eq!(
            Target::parse("192.0.2.0/33").unwrap().kind,
            TargetKind::Address
        );
        assert!(Target::is_routing_only("AS64500"));
        assert!(Target::is_routing_only("192.0.2.0/24"));
        assert!(!Target::is_routing_only("64500"));
        assert!(!Target::is_routing_only("example.com"));
    }

    #[test]
    fn a_routed_and_authorized_prefix_says_so() {
        let result = routed();
        let verdict = result.verdict();
        assert_eq!(verdict.grade, Grade::Good);
        assert_eq!(verdict.title, "Routed and authorized");
        assert_eq!(
            verdict.detail,
            "192.0.2.0/24 is announced by AS64500 Example Net, seen by 318 of 320 collector peers. A ROA authorizes this origin AS."
        );
        assert_eq!(
            result.summary(),
            "AS64500 \u{00B7} 192.0.2.0/24 \u{00B7} Valid"
        );
        assert_eq!(
            result.headline(),
            "AS64500 \u{00B7} Valid \u{00B7} 318/320 peers"
        );
        assert_eq!(result.origins[0].route_objects, ["radb"]);
        assert_eq!(result.pending, 0);
        assert_eq!(
            result.path_summary().unwrap(),
            "AS64501 Transit One is the last network before the origin AS for 68.6% of collector peers, more than any other."
        );
        assert_eq!(result.path_line(&result.paths[0]), "200 peers");
        assert_eq!(
            result.path_line(&result.paths[2]),
            "from Transit One \u{00B7} 18 peers"
        );
    }

    #[test]
    fn an_invalid_route_is_named_invalid_and_explained() {
        let mut result = routed();
        result.validations[0].status = RpkiStatus::InvalidAsn;
        result.validations[0].roas[0].origin = 64496;
        let verdict = result.verdict();
        assert_eq!(
            (verdict.grade, verdict.title.as_str()),
            (Grade::Bad, "Wrong origin ASN")
        );
        assert!(verdict.detail.ends_with("A ROA covers 192.0.2.0/24 and names AS64496. AS64500 is not named, so networks that drop invalid routes reject this announcement."));
        let checks = result.validations[0].checks();
        assert_eq!(checks.len(), 2);
        assert!(checks[0].passed && !checks[1].passed);

        let mut long = valid("192.0.2.0/25", 64500);
        long.status = RpkiStatus::InvalidLength;
        let checks = long.checks();
        assert_eq!(checks[2].detail, "/25 is longer than the maximum of /24.");
        assert_eq!(
            long.explanation(),
            "A ROA names AS64500 with a maximum length of /24. 192.0.2.0/25 is longer, so networks that drop invalid routes reject this announcement."
        );
    }

    #[test]
    fn two_origins_and_low_visibility_ask_for_attention() {
        let mut result = routed();
        result.origins.push(Origin {
            asn: 64496,
            holder: None,
            route_objects: Vec::new(),
        });
        let verdict = result.verdict();
        assert_eq!(verdict.title, "2 origin ASes observed");
        assert!(verdict
            .detail
            .ends_with("AS64496 announces it too. Anycast and migrations look like this."));

        let mut partly = routed();
        partly.seeing = Some(100);
        assert_eq!(partly.verdict().title, "Authorized, partly visible");

        let mut none = BgpResult::new(TargetKind::Prefix, "192.0.2.0/24");
        none.prefix = Some("192.0.2.0/24".to_string());
        none.announced = Some(false);
        assert_eq!(none.verdict().title, "No route observed");
        assert_eq!(none.summary(), "No route observed");

        let mut uncovered = BgpResult::new(TargetKind::Address, "192.0.2.1");
        uncovered.address = Some("192.0.2.1".to_string());
        uncovered.announced = Some(false);
        let verdict = uncovered.verdict();
        assert_eq!(verdict.title, "No route observed");
        assert!(verdict
            .detail
            .starts_with("No collector reported a route that covers 192.0.2.1."));
        assert_eq!(uncovered.summary(), "No route observed");
    }

    #[test]
    fn a_network_counts_its_prefixes_and_its_roas() {
        let mut result = BgpResult::new(TargetKind::Asn, "AS64500");
        result.apply(&BgpEvent::Network(64500));
        assert_eq!(result.verdict().detail, "Counting what AS64500 announces.");
        result.apply(&BgpEvent::NetworkPart(NetworkPart::Identity {
            holder: Some("Example Net".to_string()),
            announced: Some(true),
            block: None,
        }));
        result.apply(&BgpEvent::NetworkPart(NetworkPart::Prefixes {
            list: vec!["192.0.2.0/24".to_string(), "198.51.100.0/24".to_string()],
            v4: Some(1200),
            v6: Some(4),
        }));
        assert_eq!(result.verdict().title, "Originates 1,204 prefixes");
        result.apply(&BgpEvent::PrefixValidation(valid("192.0.2.0/24", 64500)));
        let mut invalid = valid("198.51.100.0/24", 64500);
        invalid.status = RpkiStatus::InvalidLength;
        result.apply(&BgpEvent::PrefixValidation(invalid));
        let verdict = result.verdict();
        assert_eq!(verdict.title, "1 invalid route");
        assert_eq!(
            verdict.detail,
            "Example Net announces 1,200 IPv4 and 4 IPv6 prefixes. Of 2 checked, 1 has an authorizing ROA and 1 fails origin validation."
        );
        assert_eq!(result.summary(), "Example Net \u{00B7} 1,204 prefixes");
    }

    #[test]
    fn changes_are_read_off_the_origin_timelines() {
        let day = 86_400;
        let now = 100 * day;
        let span = |origin, start: u64, end: u64, peers| Span {
            origin,
            prefix: "192.0.2.0/24".to_string(),
            start: start * day,
            end: end * day,
            peers,
        };
        // AS64500 announces throughout and loses a third of its peers on day 96. AS64496
        // shows up on day 97 and is gone by day 98.
        let spans = [
            span(64500, 10, 96, 300.0),
            span(64500, 96, 100, 200.0),
            span(64496, 97, 98, 40.0),
        ];
        let changes = Change::derive(&spans, 93 * day, now);
        let kinds: Vec<ChangeKind> = changes.iter().map(|change| change.kind).collect();
        assert_eq!(
            kinds,
            [
                ChangeKind::OriginGone,
                ChangeKind::OriginAppeared,
                ChangeKind::VisibilityDropped
            ]
        );
        assert_eq!(changes[2].detail, "Seen by 200 peers, down from 300.");
        assert_eq!(
            changes[0].detail,
            "AS64496 stopped announcing 192.0.2.0/24."
        );
        assert!(Change::derive(&spans[..1], 93 * day, 96 * day).is_empty());
        // A stretch that begins with the asked window was announced before it.
        let carried = [span(64500, 10, 100, 300.0)];
        assert!(Change::derive(&carried, 0, now).is_empty());
        let appeared = [span(64500, 40, 100, 300.0)];
        assert_eq!(Change::derive(&appeared, 0, now).len(), 1);

        let mut named = super::fixtures::routed();
        named.origins[0].holder = Some("Example Net, Inc.".to_string());
        let change = Change::derive(&appeared, 0, now).remove(0);
        assert_eq!(
            named.change_line(&change),
            "192.0.2.0/24 announced by AS64500. Example Net, Inc."
        );
    }

    #[test]
    fn a_timeline_has_a_lane_per_origin_and_gaps_where_nothing_was_announced() {
        let day = 86_400;
        let mut result = routed();
        let span = |origin, start: u64, end: u64, peers| Span {
            origin,
            prefix: "192.0.2.0/24".to_string(),
            start: start * day,
            end: end * day,
            peers,
        };
        result.spans = vec![
            span(64496, 91, 92, 40.0),
            span(64500, 80, 94, 300.0),
            span(64500, 94, 96, 310.0),
            span(64500, 98, 100, 320.0),
        ];
        let lanes = result.lanes(90 * day);
        assert_eq!(
            lanes[0],
            (64500, vec![(90 * day, 96 * day), (98 * day, 100 * day)])
        );
        assert_eq!(lanes[1], (64496, vec![(91 * day, 92 * day)]));
        let peers = result.peers_over(90 * day, 100 * day, 10);
        assert_eq!(peers[0], 300.0);
        assert_eq!(peers[1], 340.0);
        assert_eq!(peers[5], 310.0);
        assert_eq!(peers[7], 0.0);
        assert_eq!(peers[9], 320.0);
    }

    #[test]
    fn a_stored_result_loads_again() {
        let result = routed();
        let json = serde_json::to_string(&result).unwrap();
        let back: BgpResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, result);
        let minimal: BgpResult =
            serde_json::from_str(r#"{"kind":"asn","query":"AS64500"}"#).unwrap();
        assert!(minimal.is_empty());
    }

    #[test]
    fn numbers_and_lists_read_as_words() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1204), "1,204");
        assert_eq!(count(1_234_567), "1,234,567");
        let names = |items: &[&str]| {
            list(
                &items
                    .iter()
                    .map(|item| item.to_string())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(names(&["A"]), "A");
        assert_eq!(names(&["A", "B"]), "A and B");
        assert_eq!(names(&["A", "B", "C"]), "A, B, and C");
        assert_eq!(date_time(1_800_000_000), "2027-01-15 08:00 UTC");
    }
}
