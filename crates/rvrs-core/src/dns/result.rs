//! What a DNS run found, as history stores it, and what follows from it: the reference answer,
//! the summary, the verdict, where the sources disagree and why that is usually fine. The rules
//! and the sentences are the app's (`DNSResult.swift`), ported one for one.
//!
//! Stored on disk across versions: a field added later must carry `#[serde(default)]`.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::message::{type_name, types, Record, Section};
use crate::format;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Device,
    Public,
    Authoritative,
}

impl SourceKind {
    pub fn title(self) -> &'static str {
        match self {
            SourceKind::Device => "Device",
            SourceKind::Public => "Public",
            SourceKind::Authoritative => "Authoritative",
        }
    }

    /// Device, then public resolvers, then the zone: a fixed order whatever answered first.
    pub fn rank(self) -> u8 {
        match self {
            SourceKind::Device => 0,
            SourceKind::Public => 1,
            SourceKind::Authoritative => 2,
        }
    }
}

/// What one source answered: the device's resolver, a public resolver, or the zone's own server
/// at the end of the delegation path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceResult {
    pub kind: SourceKind,
    /// "Cloudflare", "Router", "ns1.example.net".
    pub label: String,
    pub server: String,
    /// Seconds since 1970.
    pub queried_at: u64,
    #[serde(default)]
    pub rtt_ms: Option<f64>,
    #[serde(default)]
    pub records: Vec<Record>,
    #[serde(default)]
    pub authentic_data: bool,
    #[serde(default)]
    pub is_authoritative: bool,
    /// Rcode names for questions that came back with an error, by type ("A": "SERVFAIL").
    #[serde(default)]
    pub failures: BTreeMap<String, String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl SourceResult {
    pub fn new(kind: SourceKind, label: &str, server: &str) -> Self {
        Self {
            kind,
            label: label.to_string(),
            server: server.to_string(),
            queried_at: format::now(),
            rtt_ms: None,
            records: Vec::new(),
            authentic_data: false,
            is_authoritative: false,
            failures: BTreeMap::new(),
            error: None,
        }
    }

    pub fn id(&self) -> String {
        format!("{}-{}", self.kind.title().to_lowercase(), self.server)
    }

    pub fn answers(&self) -> impl Iterator<Item = &Record> {
        self.records
            .iter()
            .filter(|record| record.section == Section::Answer)
    }

    /// The values of the answer-section records of one type, sorted.
    pub fn values(&self, type_code: u16) -> Vec<String> {
        let mut values: Vec<String> = self
            .answers()
            .filter(|record| record.type_code == type_code)
            .map(|record| record.value.clone())
            .collect();
        values.sort();
        values
    }

    pub fn is_local(&self) -> bool {
        self.kind == SourceKind::Device
    }
}

/// One step of the delegation walk, from the root down to the zone that holds the name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelegationStep {
    /// The zone this server was asked about ("." for the root, "com", "example.com").
    pub zone: String,
    pub server: String,
    pub address: String,
    #[serde(default)]
    pub rtt_ms: Option<f64>,
    /// Nameservers the answer delegated the next zone to. Empty when it answered itself.
    #[serde(default)]
    pub nameservers: Vec<String>,
    #[serde(default)]
    pub next_zone: Option<String>,
    /// DS for the next zone present here: the parent vouches for the child's keys.
    #[serde(default)]
    pub ds_present: Option<bool>,
    /// RRSIGs in the answer: this zone signs what it serves.
    #[serde(default)]
    pub signed: bool,
    #[serde(default)]
    pub error: Option<String>,
}

impl DelegationStep {
    pub fn new(zone: &str, server: &str, address: &str) -> Self {
        Self {
            zone: zone.to_string(),
            server: server.to_string(),
            address: address.to_string(),
            rtt_ms: None,
            nameservers: Vec::new(),
            next_zone: None,
            ds_present: None,
            signed: false,
            error: None,
        }
    }

    /// "a.gtld-servers.net · 14 ms · delegates example.com to 2 servers"
    pub fn detail(&self) -> String {
        if let Some(error) = &self.error {
            return format!("{}: {error}", self.server);
        }
        let mut parts = vec![self.server.clone()];
        if let Some(rtt) = self.rtt_ms {
            parts.push(format!("{rtt:.0} ms"));
        }
        match &self.next_zone {
            Some(next) => {
                let count = self.nameservers.len();
                parts.push(format!(
                    "delegates {next} to {count} server{}",
                    if count == 1 { "" } else { "s" }
                ));
            }
            None => parts.push("answers for the name".to_string()),
        }
        parts.join(" \u{00B7} ")
    }

    /// "Root", "TLD", "Zone", and for the last step "Authoritative" or "Referral".
    pub fn role(&self, is_last: bool) -> &'static str {
        if self.zone == "." {
            "Root"
        } else if is_last {
            if self.next_zone.is_none() {
                "Authoritative"
            } else {
                "Referral"
            }
        } else if self.zone.contains('.') {
            "Zone"
        } else {
            "TLD"
        }
    }
}

/// Whether DNSSEC records are present along the delegation. rvrs checks that they are there. It
/// does not validate signatures, so the word is "signed", never "verified" or "secure".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dnssec {
    Signed,
    Unsigned,
    Broken,
    #[default]
    Unknown,
}

impl Dnssec {
    pub fn title(self) -> &'static str {
        match self {
            Dnssec::Signed => "DNSSEC signed",
            Dnssec::Unsigned => "No DNSSEC",
            Dnssec::Broken => "DNSSEC broken",
            Dnssec::Unknown => "DNSSEC unknown",
        }
    }

    /// The one word a tile shows.
    pub fn short(self) -> &'static str {
        match self {
            Dnssec::Signed => "Signed",
            Dnssec::Unsigned => "None",
            Dnssec::Broken => "Broken",
            Dnssec::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictGrade {
    Good,
    Attention,
    Problem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsVerdict {
    pub title: String,
    pub detail: String,
    pub grade: VerdictGrade,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DnsResult {
    pub domain: String,
    #[serde(default)]
    pub sources: Vec<SourceResult>,
    #[serde(default)]
    pub path: Vec<DelegationStep>,
    #[serde(default)]
    pub dnssec: Dnssec,
    #[serde(default)]
    pub dnssec_detail: String,
    /// Sources still being asked while the run is live.
    #[serde(default)]
    pub pending_sources: usize,
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

impl DnsResult {
    pub fn new(domain: &str) -> Self {
        Self {
            domain: domain.to_string(),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty() && self.path.is_empty()
    }

    pub fn device(&self) -> Option<&SourceResult> {
        self.sources
            .iter()
            .find(|source| source.kind == SourceKind::Device)
    }

    pub fn authoritative(&self) -> Option<&SourceResult> {
        self.sources
            .iter()
            .find(|source| source.kind == SourceKind::Authoritative)
    }

    pub fn public_resolvers(&self) -> impl Iterator<Item = &SourceResult> {
        self.sources
            .iter()
            .filter(|source| source.kind == SourceKind::Public)
    }

    /// The sources in the order the screens list them.
    pub fn ordered_sources(&self) -> Vec<&SourceResult> {
        let mut sources: Vec<&SourceResult> = self.sources.iter().collect();
        sources.sort_by_key(|source| source.kind.rank());
        sources
    }

    /// The answer the others are measured against: the zone itself, else the first public
    /// resolver that answered, else the device.
    pub fn reference(&self) -> Option<&SourceResult> {
        self.authoritative()
            .or_else(|| {
                self.public_resolvers()
                    .find(|source| source.error.is_none())
            })
            .or_else(|| self.device())
    }

    /// Addresses from the reference answer, A then AAAA as the server sent them.
    pub fn addresses(&self) -> Vec<&Record> {
        self.reference()
            .map(|reference| {
                reference
                    .answers()
                    .filter(|record| {
                        record.type_code == types::A || record.type_code == types::AAAA
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every distinct answer record across the sources.
    pub fn answer_count(&self) -> usize {
        let distinct: BTreeSet<String> = self
            .sources
            .iter()
            .flat_map(|source| source.answers())
            .map(|record| format!("{} {}", record.type_name(), record.value))
            .collect();
        distinct.len()
    }

    /// The quickest source to answer, in milliseconds.
    pub fn fastest_ms(&self) -> Option<f64> {
        self.sources
            .iter()
            .filter_map(|source| source.rtt_ms)
            .fold(None, |best, rtt| {
                Some(best.map_or(rtt, |best: f64| best.min(rtt)))
            })
    }

    /// The shortest TTL among the addresses: how long a cache may keep them.
    pub fn address_ttl(&self) -> Option<u32> {
        self.addresses().iter().map(|record| record.ttl).min()
    }

    /// "example-dns.com": the last two labels of the first nameserver, as a hint at who runs
    /// the zone.
    pub fn nameserver_provider(&self) -> Option<String> {
        let nameserver = self
            .reference()
            .and_then(|reference| reference.values(types::NS).into_iter().next())
            .or_else(|| {
                self.sources
                    .iter()
                    .find_map(|source| source.values(types::NS).into_iter().next())
            })?;
        let labels: Vec<&str> = nameserver
            .split('.')
            .filter(|label| !label.is_empty())
            .collect();
        Some(if labels.len() >= 2 {
            labels[labels.len() - 2..].join(".")
        } else {
            nameserver.clone()
        })
    }

    pub fn dnssec_word(&self) -> &'static str {
        match self.dnssec {
            Dnssec::Signed => "signed",
            Dnssec::Unsigned => "unsigned",
            Dnssec::Broken => "DNSSEC broken",
            Dnssec::Unknown => "DNSSEC unknown",
        }
    }

    /// "3 addresses · all agree · signed", for rows in a list.
    pub fn summary(&self) -> String {
        let mut parts = vec![plural(self.addresses().len(), "address", "addresses")];
        parts.push(
            if Diff::of(self).agree {
                "all agree"
            } else {
                "answers differ"
            }
            .to_string(),
        );
        if self.dnssec != Dnssec::Unknown {
            parts.push(self.dnssec_word().to_string());
        }
        parts.join(" \u{00B7} ")
    }

    /// "Resolved · 24 ms · all agree · signed", for the status line.
    pub fn headline(&self) -> String {
        let answered = self
            .sources
            .iter()
            .any(|source| source.error.is_none() && !source.records.is_empty());
        let mut parts = vec![if answered { "Resolved" } else { "No answer" }.to_string()];
        if let Some(fastest) = self.fastest_ms() {
            parts.push(format!("{fastest:.0} ms"));
        }
        parts.push(
            if Diff::of(self).agree {
                "all agree"
            } else {
                "answers differ"
            }
            .to_string(),
        );
        if self.dnssec != Dnssec::Unknown {
            parts.push(self.dnssec_word().to_string());
        }
        parts.join(" \u{00B7} ")
    }

    /// The verdict: a title that states the finding and the sentences behind it.
    pub fn verdict(&self) -> DnsVerdict {
        let diff = Diff::of(self);
        let count = self.addresses().len();
        let reference = self.reference();
        let mut sentences: Vec<String> = Vec::new();
        if count > 0 {
            let mut first = format!("Resolves to {}", plural(count, "address", "addresses"));
            if let Some(alias) =
                reference.and_then(|reference| reference.values(types::CNAME).into_iter().next())
            {
                first.push_str(&format!(" through {alias}"));
            }
            if let Some(provider) = self.nameserver_provider() {
                first.push_str(&format!(", zone served by {provider}"));
            }
            sentences.push(format!("{first}."));
        } else if let Some(failure) =
            reference.and_then(|reference| reference.failures.values().next())
        {
            sentences.push(format!("The zone answers {failure} for this name."));
        } else {
            sentences.push("No address came back from any source.".to_string());
        }
        match self.dnssec {
            Dnssec::Signed => {
                sentences.push("DNSSEC records are present from the root down.".to_string())
            }
            Dnssec::Unsigned => sentences.push("The zone isn't signed with DNSSEC.".to_string()),
            Dnssec::Broken => sentences.push(
                "The DNSSEC chain is broken. Validating resolvers return SERVFAIL for this name."
                    .to_string(),
            ),
            Dnssec::Unknown => {}
        }
        if !diff.agree {
            let differing = diff.notes.iter().filter(|note| note.is_difference).count();
            sentences.push(format!(
                "{} answer{} differently. Compare shows why.",
                plural(differing, "source", "sources"),
                if differing == 1 { "s" } else { "" }
            ));
        }
        let grade = if self.dnssec == Dnssec::Broken || count == 0 {
            VerdictGrade::Problem
        } else if diff.agree {
            VerdictGrade::Good
        } else {
            VerdictGrade::Attention
        };
        let title = match grade {
            VerdictGrade::Good => "All sources agree",
            VerdictGrade::Attention => "Answers differ",
            VerdictGrade::Problem if count == 0 => "No address",
            VerdictGrade::Problem => "DNSSEC broken",
        };
        DnsVerdict {
            title: title.to_string(),
            detail: sentences.join(" "),
            grade,
        }
    }

    /// A, AAAA, CNAME, HTTPS, the first MX and the first NS from the reference answer.
    pub fn key_records(&self) -> Vec<&Record> {
        let Some(reference) = self.reference() else {
            return Vec::new();
        };
        let answers: Vec<&Record> = reference.answers().collect();
        let all = |type_code: u16| {
            answers
                .iter()
                .copied()
                .filter(move |record| record.type_code == type_code)
        };
        let mut out: Vec<&Record> = all(types::A).chain(all(types::AAAA)).collect();
        for type_code in [types::CNAME, types::HTTPS, types::MX, types::NS] {
            out.extend(all(type_code).next());
        }
        out
    }
}

/// One thing the comparison found, with its usual reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub title: String,
    pub detail: String,
    pub is_difference: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStatus {
    Match,
    Differs,
    Error,
}

impl SourceStatus {
    pub fn title(self) -> &'static str {
        match self {
            SourceStatus::Match => "Match",
            SourceStatus::Differs => "Differs",
            SourceStatus::Error => "Error",
        }
    }
}

/// Where the sources disagree, and the usual reason. Computed, never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub agree: bool,
    pub notes: Vec<Note>,
    differing: BTreeSet<String>,
    failing: BTreeSet<String>,
}

/// The record types the comparison looks at, with the word a note uses for each.
const COMPARED: [(u16, &str); 6] = [
    (types::A, "IPv4"),
    (types::AAAA, "IPv6"),
    (types::CNAME, "CNAME"),
    (types::HTTPS, "HTTPS"),
    (types::MX, "mail"),
    (types::NS, "nameserver"),
];

impl Diff {
    pub fn of(result: &DnsResult) -> Self {
        let mut notes: Vec<Note> = Vec::new();
        let mut differing = BTreeSet::new();
        let mut failing = BTreeSet::new();
        let sources: Vec<&SourceResult> = result
            .sources
            .iter()
            .filter(|source| source.error.is_none())
            .collect();
        let reference = result
            .authoritative()
            .or_else(|| result.public_resolvers().next())
            .or_else(|| result.device());

        for source in &sources {
            let Some(rcode) = source
                .failures
                .get("A")
                .or_else(|| source.failures.values().next())
            else {
                continue;
            };
            if rcode == "SERVFAIL" {
                failing.insert(source.id());
                if source.kind == SourceKind::Public {
                    notes.push(Note {
                        title: format!("{} answers SERVFAIL", source.label),
                        detail: "Validating resolvers return SERVFAIL when the DNSSEC chain is broken or the zone's servers don't answer.".to_string(),
                        is_difference: true,
                    });
                }
            } else if rcode == "NXDOMAIN" {
                notes.push(Note {
                    title: format!("{} answers NXDOMAIN", source.label),
                    detail: "The zone has no such name.".to_string(),
                    is_difference: false,
                });
            }
        }

        let mut differs = false;
        if let Some(reference) = reference {
            for (type_code, what) in COMPARED {
                let expected = reference.values(type_code);
                for source in &sources {
                    if source.id() == reference.id()
                        || source.failures.contains_key(&type_name(type_code))
                    {
                        continue;
                    }
                    let got = source.values(type_code);
                    if got == expected || (got.is_empty() && expected.is_empty()) {
                        continue;
                    }
                    differs = true;
                    differing.insert(source.id());
                    notes.push(Note {
                        title: format!("{what} differs at {}", source.label),
                        detail: explain(type_code, source, reference, &got, &expected),
                        is_difference: true,
                    });
                }
            }
        }

        // A cached answer is not a difference, but it is worth a line.
        if let (Some(authoritative), Some(device)) = (result.authoritative(), result.device()) {
            let first_ttl = |source: &SourceResult| {
                source
                    .answers()
                    .find(|record| record.type_code == types::A)
                    .map(|record| record.ttl)
            };
            if let (Some(zone_ttl), Some(device_ttl)) =
                (first_ttl(authoritative), first_ttl(device))
            {
                if device_ttl < zone_ttl {
                    notes.push(Note {
                        title: "Device answer is cached".to_string(),
                        detail: format!(
                            "{} left of a {} TTL. It refreshes from the zone after that.",
                            format::ttl(device_ttl),
                            format::ttl(zone_ttl)
                        ),
                        is_difference: false,
                    });
                }
            }
        }

        Self {
            agree: !differs,
            notes,
            differing,
            failing,
        }
    }

    /// How one source compares with the reference answer.
    pub fn status(&self, source: &SourceResult) -> SourceStatus {
        if source.error.is_some() || (!source.failures.is_empty() && source.records.is_empty()) {
            return SourceStatus::Error;
        }
        if self.failing.contains(&source.id()) {
            return SourceStatus::Error;
        }
        if self.differing.contains(&source.id()) {
            SourceStatus::Differs
        } else {
            SourceStatus::Match
        }
    }

    pub fn difference_count(&self) -> usize {
        self.notes.iter().filter(|note| note.is_difference).count()
    }
}

fn explain(
    type_code: u16,
    source: &SourceResult,
    reference: &SourceResult,
    got: &[String],
    expected: &[String],
) -> String {
    let list = |values: &[String]| {
        if values.is_empty() {
            "nothing".to_string()
        } else {
            values.join(", ")
        }
    };
    let shown = format!(
        "{}: {} \u{00B7} {}: {}",
        source.label,
        list(got),
        reference.label,
        list(expected)
    );
    match type_code {
        types::A | types::AAAA if source.kind == SourceKind::Device => {
            format!("{shown}. Your resolver answers from its own view: a split-horizon zone, a filter, or an older cache.")
        }
        types::A | types::AAAA => format!("{shown}. A CDN or geo-aware DNS answers by where the resolver asks from, so both answers can be right."),
        types::CNAME | types::HTTPS => {
            format!("{shown}. The zone may have changed recently. Resolvers keep the old answer until its TTL runs out.")
        }
        types::MX => format!("{shown}. Mail routing differs between what the zone says and what this resolver has cached."),
        types::NS => format!(
            "{shown}. The NS set at the parent and the one in the zone should match. A mismatch usually means a nameserver change that isn't finished."
        ),
        _ => shown,
    }
}

/// What changed since the last saved lookup of the same name: the reference answers, type by type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub type_name: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

impl Change {
    /// "A: 192.0.2.1 → 192.0.2.7"
    pub fn line(&self) -> String {
        let list = |values: &[String]| {
            if values.is_empty() {
                "none".to_string()
            } else {
                values.join(", ")
            }
        };
        format!(
            "{}: {} \u{2192} {}",
            self.type_name,
            list(&self.before),
            list(&self.after)
        )
    }
}

pub fn changes(current: &DnsResult, previous: Option<&DnsResult>) -> Vec<Change> {
    let (Some(now), Some(then)) = (current.reference(), previous.and_then(DnsResult::reference))
    else {
        return Vec::new();
    };
    COMPARED
        .iter()
        .filter_map(|(type_code, _)| {
            let (before, after) = (then.values(*type_code), now.values(*type_code));
            (before != after && !(before.is_empty() && after.is_empty())).then(|| Change {
                type_name: type_name(*type_code),
                before,
                after,
            })
        })
        .collect()
}

/// The record groups the screens show, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordGroup {
    Addresses,
    Email,
    Infrastructure,
    Security,
}

impl RecordGroup {
    pub const ALL: [RecordGroup; 4] = [
        RecordGroup::Addresses,
        RecordGroup::Email,
        RecordGroup::Infrastructure,
        RecordGroup::Security,
    ];

    pub fn title(self) -> &'static str {
        match self {
            RecordGroup::Addresses => "Addresses",
            RecordGroup::Email => "Email",
            RecordGroup::Infrastructure => "Infrastructure",
            RecordGroup::Security => "Security",
        }
    }

    pub fn of(record: &Record) -> Self {
        let text = record.value.to_lowercase();
        match record.type_code {
            types::A
            | types::AAAA
            | types::CNAME
            | types::HTTPS
            | types::SVCB
            | types::PTR
            | types::SRV => RecordGroup::Addresses,
            types::MX => RecordGroup::Email,
            types::TXT if text.contains("v=spf1") || text.contains("v=dmarc1") => {
                RecordGroup::Email
            }
            types::CAA | types::DS | types::DNSKEY | types::RRSIG => RecordGroup::Security,
            _ => RecordGroup::Infrastructure,
        }
    }
}

/// One plain sentence per record type.
pub fn explain_record(record: &Record) -> &'static str {
    let text = record.value.to_lowercase();
    match record.type_code {
        types::A => "An IPv4 address for the name. Several A records mean the service spreads load or sits behind a CDN.",
        types::AAAA => "An IPv6 address for the name.",
        types::CNAME => "The name is an alias, and clients resolve its target instead. A CNAME can't sit at a zone apex.",
        types::NS => "A nameserver that holds the zone. The set at the parent and in the zone should match.",
        types::SOA => "Start of authority: the primary server, the admin mailbox, the serial and the timers secondaries follow.",
        types::MX => "Where mail for the domain is delivered, lowest preference first.",
        types::TXT if text.contains("v=spf1") => "SPF: which servers may send mail for the domain.",
        types::TXT if text.contains("v=dmarc1") => "DMARC: what receivers do with mail that fails SPF or DKIM.",
        types::TXT => "Free-form text, usually a domain-ownership verification for a service.",
        types::HTTPS => "Service binding for HTTPS: the protocols (alpn), ports and address hints a client may use before connecting, and ECH keys when present.",
        types::SVCB => "General service binding: how to reach a service, with parameters like alpn and address hints.",
        types::CAA => "Which certificate authorities may issue certificates for the name.",
        types::DS => "The parent's hash of the zone's signing key: the link in the DNSSEC chain.",
        types::DNSKEY => "A key the zone signs with. The parent's DS points at the key-signing key (KSK), and the zone-signing key (ZSK) signs the records.",
        types::RRSIG => "A signature over a record set, with the type covered, the signer and the validity window.",
        types::SRV => "Where a service lives: priority, weight, port and host.",
        types::PTR => "The name for an address, used for reverse lookups.",
        _ => "A record type rvrs shows as hex.",
    }
}

/// The part of a record's value the probing tools can run against: the address, or the target
/// name. None for records that name no host.
pub fn tool_host(record: &Record) -> Option<String> {
    let mut words = record.value.split(' ');
    let host = match record.type_code {
        types::A | types::AAAA | types::CNAME | types::NS | types::PTR => {
            Some(record.value.clone())
        }
        types::MX => words.next_back().map(str::to_string),
        types::HTTPS | types::SVCB => words.nth(1).map(|target| {
            if target == "." {
                record.name.clone()
            } else {
                target.to_string()
            }
        }),
        types::SOA => words.next().map(str::to_string),
        _ => None,
    };
    host.map(|host| host.trim_end_matches('.').to_string())
        .filter(|host| !host.is_empty())
}

/// "example.com. 300 IN A 192.0.2.1", the record as a line of a zone file.
pub fn zone_line(record: &Record) -> String {
    format!(
        "{}. {} IN {} {}",
        record.name,
        record.ttl,
        record.type_name(),
        record.value
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn record(type_code: u16, value: &str, ttl: u32) -> Record {
        Record {
            name: "example.com".into(),
            type_code,
            ttl,
            value: value.into(),
            section: Section::Answer,
        }
    }

    pub fn source(
        kind: SourceKind,
        label: &str,
        server: &str,
        records: Vec<Record>,
    ) -> SourceResult {
        let mut source = SourceResult::new(kind, label, server);
        source.queried_at = 1_794_735_000;
        source.rtt_ms = Some(12.0);
        source.records = records;
        source
    }

    fn zone_records() -> Vec<Record> {
        vec![
            record(types::A, "192.0.2.10", 300),
            record(types::A, "192.0.2.11", 300),
            record(types::NS, "ns1.example-dns.net", 86_400),
            record(types::MX, "10 mail.example.com", 3600),
        ]
    }

    /// A lookup where the router, one public resolver and the zone give the same answer.
    pub fn agreeing() -> DnsResult {
        let mut result = DnsResult::new("example.com");
        result.sources = vec![
            source(SourceKind::Public, "Cloudflare", "1.1.1.1", zone_records()),
            source(
                SourceKind::Authoritative,
                "ns1.example-dns.net",
                "198.51.100.53",
                zone_records(),
            ),
            source(SourceKind::Device, "Router", "192.168.1.1", zone_records()),
        ];
        result.sources[0].rtt_ms = Some(9.4);
        result.dnssec = Dnssec::Signed;
        result.dnssec_detail = "DS at com, DNSKEY in example.com, answers signed. Validating resolvers can check every step.".into();
        let mut root = DelegationStep::new(".", "a.root-servers.net", "198.41.0.4");
        root.next_zone = Some("com".into());
        root.nameservers = vec!["a.gtld-servers.net".into(), "b.gtld-servers.net".into()];
        root.ds_present = Some(true);
        root.rtt_ms = Some(14.2);
        let mut tld = DelegationStep::new("com", "a.gtld-servers.net", "192.5.6.30");
        tld.next_zone = Some("example.com".into());
        tld.nameservers = vec!["ns1.example-dns.net".into()];
        tld.ds_present = Some(true);
        let mut zone = DelegationStep::new("example.com", "ns1.example-dns.net", "198.51.100.53");
        zone.signed = true;
        result.path = vec![root, tld, zone];
        result
    }

    #[test]
    fn the_zone_is_the_reference_and_the_sources_list_in_a_fixed_order() {
        let result = agreeing();
        assert_eq!(result.reference().unwrap().kind, SourceKind::Authoritative);
        let order: Vec<&str> = result
            .ordered_sources()
            .iter()
            .map(|source| source.label.as_str())
            .collect();
        assert_eq!(order, ["Router", "Cloudflare", "ns1.example-dns.net"]);
        assert_eq!(result.addresses().len(), 2);
        assert_eq!(result.address_ttl(), Some(300));
        assert_eq!(result.fastest_ms(), Some(9.4));
        assert_eq!(result.answer_count(), 4);
        assert_eq!(
            result.nameserver_provider().as_deref(),
            Some("example-dns.net")
        );
    }

    #[test]
    fn agreement_reads_as_the_app_words_it() {
        let result = agreeing();
        assert_eq!(
            result.summary(),
            "2 addresses \u{00B7} all agree \u{00B7} signed"
        );
        assert_eq!(
            result.headline(),
            "Resolved \u{00B7} 9 ms \u{00B7} all agree \u{00B7} signed"
        );
        let verdict = result.verdict();
        assert_eq!(verdict.grade, VerdictGrade::Good);
        assert_eq!(verdict.title, "All sources agree");
        assert_eq!(
            verdict.detail,
            "Resolves to 2 addresses, zone served by example-dns.net. DNSSEC records are present from the root down."
        );
        let diff = Diff::of(&result);
        assert!(diff.agree && diff.notes.is_empty());
        assert!(result
            .sources
            .iter()
            .all(|source| diff.status(source) == SourceStatus::Match));
    }

    #[test]
    fn a_public_resolver_with_other_addresses_is_a_cdn_and_not_a_fault() {
        let mut result = agreeing();
        result.sources[0].records[0].value = "203.0.113.5".into();
        let diff = Diff::of(&result);
        assert!(!diff.agree);
        assert_eq!(diff.notes[0].title, "IPv4 differs at Cloudflare");
        assert!(diff.notes[0].detail.starts_with("Cloudflare: 192.0.2.11, 203.0.113.5 \u{00B7} ns1.example-dns.net: 192.0.2.10, 192.0.2.11."));
        assert!(diff.notes[0].detail.ends_with(
            "A CDN or geo-aware DNS answers by where the resolver asks from, so both answers can be right."
        ));
        assert_eq!(diff.status(&result.sources[0]), SourceStatus::Differs);
        assert_eq!(diff.status(&result.sources[2]), SourceStatus::Match);
        let verdict = result.verdict();
        assert_eq!(
            (verdict.title.as_str(), verdict.grade),
            ("Answers differ", VerdictGrade::Attention)
        );
        assert!(verdict
            .detail
            .ends_with("1 source answers differently. Compare shows why."));
        assert_eq!(
            result.summary(),
            "2 addresses \u{00B7} answers differ \u{00B7} signed"
        );
    }

    #[test]
    fn the_device_differing_is_its_own_view() {
        let mut result = agreeing();
        result.sources[2].records.truncate(1);
        let diff = Diff::of(&result);
        let note = diff
            .notes
            .iter()
            .find(|note| note.title == "IPv4 differs at Router")
            .unwrap();
        assert!(note.detail.ends_with("Your resolver answers from its own view: a split-horizon zone, a filter, or an older cache."));
        // The device has no MX or NS at all, which differs too.
        assert_eq!(diff.difference_count(), 3);
    }

    #[test]
    fn a_lower_ttl_on_the_device_is_a_cache_and_no_difference() {
        let mut result = agreeing();
        result.sources[2].records[0].ttl = 120;
        let diff = Diff::of(&result);
        assert!(diff.agree);
        assert_eq!(diff.notes.len(), 1);
        assert_eq!(diff.notes[0].title, "Device answer is cached");
        assert_eq!(
            diff.notes[0].detail,
            "2m left of a 5m TTL. It refreshes from the zone after that."
        );
        assert!(!diff.notes[0].is_difference);
    }

    #[test]
    fn servfail_at_a_validator_is_an_error_and_nxdomain_is_an_answer() {
        let mut result = agreeing();
        result.sources[0].records.clear();
        result.sources[0]
            .failures
            .insert("A".into(), "SERVFAIL".into());
        let diff = Diff::of(&result);
        assert_eq!(diff.notes[0].title, "Cloudflare answers SERVFAIL");
        assert_eq!(diff.status(&result.sources[0]), SourceStatus::Error);

        let mut missing = DnsResult::new("nope.example.com");
        let mut zone = source(
            SourceKind::Authoritative,
            "ns1.example-dns.net",
            "198.51.100.53",
            Vec::new(),
        );
        zone.failures.insert("A".into(), "NXDOMAIN".into());
        missing.sources = vec![zone];
        let verdict = missing.verdict();
        assert_eq!(
            (verdict.title.as_str(), verdict.grade),
            ("No address", VerdictGrade::Problem)
        );
        assert_eq!(verdict.detail, "The zone answers NXDOMAIN for this name.");
        assert_eq!(
            Diff::of(&missing).notes[0].detail,
            "The zone has no such name."
        );
        assert_eq!(
            missing.headline(),
            "No answer \u{00B7} 12 ms \u{00B7} all agree"
        );
    }

    #[test]
    fn a_broken_chain_is_the_problem_whatever_the_sources_say() {
        let mut result = agreeing();
        result.dnssec = Dnssec::Broken;
        let verdict = result.verdict();
        assert_eq!(
            (verdict.title.as_str(), verdict.grade),
            ("DNSSEC broken", VerdictGrade::Problem)
        );
        assert!(verdict
            .detail
            .contains("Validating resolvers return SERVFAIL for this name."));
        assert_eq!(result.dnssec_word(), "DNSSEC broken");
    }

    #[test]
    fn changes_compare_the_reference_answers() {
        let before = agreeing();
        let mut after = agreeing();
        after.sources[1].records[0].value = "192.0.2.99".into();
        after.sources[1]
            .records
            .retain(|record| record.type_code != types::MX);
        let found = changes(&after, Some(&before));
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0].line(),
            "A: 192.0.2.10, 192.0.2.11 \u{2192} 192.0.2.11, 192.0.2.99"
        );
        assert_eq!(found[1].line(), "MX: 10 mail.example.com \u{2192} none");
        assert!(changes(&after, None).is_empty());
        assert!(changes(&before, Some(&before)).is_empty());
    }

    #[test]
    fn key_records_groups_and_hosts() {
        let result = agreeing();
        let keys: Vec<String> = result
            .key_records()
            .iter()
            .map(|record| record.type_name())
            .collect();
        assert_eq!(keys, ["A", "A", "MX", "NS"]);
        assert_eq!(
            RecordGroup::of(&record(types::TXT, "\"v=spf1 -all\"", 60)),
            RecordGroup::Email
        );
        assert_eq!(
            RecordGroup::of(&record(types::TXT, "\"site-verification=abc\"", 60)),
            RecordGroup::Infrastructure
        );
        assert_eq!(
            RecordGroup::of(&record(types::CAA, "0 issue \"ca.example\"", 60)),
            RecordGroup::Security
        );
        assert_eq!(
            tool_host(&record(types::MX, "10 mail.example.com", 60)).as_deref(),
            Some("mail.example.com")
        );
        assert_eq!(
            tool_host(&record(types::HTTPS, "1 . alpn=\"h2\"", 60)).as_deref(),
            Some("example.com")
        );
        assert_eq!(
            tool_host(&record(
                types::SOA,
                "ns1.example.com admin.example.com 1 2 3 4 5",
                60
            ))
            .as_deref(),
            Some("ns1.example.com")
        );
        assert_eq!(tool_host(&record(types::TXT, "\"x\"", 60)), None);
        assert_eq!(
            zone_line(&record(types::A, "192.0.2.10", 300)),
            "example.com. 300 IN A 192.0.2.10"
        );
        assert!(
            explain_record(&record(types::TXT, "\"v=DMARC1; p=none\"", 60)).starts_with("DMARC")
        );
    }

    #[test]
    fn delegation_steps_say_what_they_did() {
        let result = agreeing();
        assert_eq!(
            result.path[0].detail(),
            "a.root-servers.net \u{00B7} 14 ms \u{00B7} delegates com to 2 servers"
        );
        assert_eq!(
            result.path[2].detail(),
            "ns1.example-dns.net \u{00B7} answers for the name"
        );
        let roles: Vec<&str> = result
            .path
            .iter()
            .enumerate()
            .map(|(index, step)| step.role(index == 2))
            .collect();
        assert_eq!(roles, ["Root", "TLD", "Authoritative"]);
        let mut failed = DelegationStep::new("com", "a.gtld-servers.net", "192.5.6.30");
        failed.error = Some("192.5.6.30 didn't answer in time.".into());
        assert_eq!(
            failed.detail(),
            "a.gtld-servers.net: 192.5.6.30 didn't answer in time."
        );
    }

    #[test]
    fn a_result_saved_by_an_older_version_still_loads() {
        let json = r#"{"domain":"example.com","sources":[{"kind":"public","label":"Cloudflare","server":"1.1.1.1","queried_at":1}]}"#;
        let result: DnsResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.sources[0].kind, SourceKind::Public);
        assert_eq!(result.dnssec, Dnssec::Unknown);
        assert!(result.path.is_empty());
    }
}
