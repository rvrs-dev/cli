//! A run as data: the report `--json` prints and, once accounts exist, the body the API stores.
//! Field for field the `RunReport` of the iOS app (`rvrs/Models/RunReport.swift`): what was
//! measured (`measured`, `hops`, `outcome`) stays apart from what it likely means
//! (`interpretation`), so a person or an agent can tell a fact from a guess.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dns::message::types;
use crate::dns::result::Diff;
use crate::format;
use crate::model::{Grade, Outcome, Run, Tool, Verdict};
use crate::node_config::DeclaredLocation;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub name: String,
    pub value: f64,
    #[serde(default)]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportHop {
    pub ttl: u8,
    pub address: Option<String>,
    pub hostname: Option<String>,
    pub asn: Option<String>,
    pub network: Option<String>,
    pub loss_percent: Option<f64>,
    pub avg_milliseconds: Option<f64>,
    pub min_milliseconds: Option<f64>,
    pub max_milliseconds: Option<f64>,
    pub jitter_milliseconds: Option<f64>,
    /// Probes that were answered or lost. One still within its wait counts as neither.
    pub sent: Option<u32>,
    pub received: Option<u32>,
    /// The round trip of each probe in the order they were sent, the newest
    /// [`HOP_SAMPLES`]. `null` in the list is a probe that got no reply. A report from an
    /// origin that keeps no samples has none.
    pub samples: Option<Vec<Option<f64>>>,
}

/// How many samples a hop carries in a report: enough to draw every round of a usual MTR.
pub const HOP_SAMPLES: usize = 120;
/// How many samples a report carries for the destination: a ping or MTR left running.
pub const RUN_SAMPLES: usize = 600;

/// The round trips of settled probes in milliseconds, the newest `most`. `None` is a loss.
fn samples_of(probes: &[crate::model::Slot], most: usize) -> Vec<Option<f64>> {
    let settled: Vec<Option<f64>> = probes
        .iter()
        .filter(|slot| **slot != crate::model::Slot::Pending)
        .map(|slot| slot.rtt().map(|rtt| round(format::ms_value(rtt))))
        .collect();
    settled[settled.len().saturating_sub(most)..].to_vec()
}

/// One DNS record of the reference answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReportRecord {
    #[serde(rename = "type")]
    pub type_name: String,
    pub name: String,
    pub ttl: Option<u32>,
    pub value: String,
}

/// One source of a DNS lookup: the device's resolver, a public resolver or the zone's own server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSource {
    /// "Cloudflare", "Router", "ns1.example.net".
    pub name: String,
    /// device, public or authoritative.
    pub kind: String,
    pub server: Option<String>,
    /// match, differs or error, against the reference answer.
    pub status: String,
    pub milliseconds: Option<f64>,
    /// The A and AAAA values it answered, sorted.
    #[serde(default)]
    pub addresses: Vec<String>,
    /// Why it has no answer: a failure to ask, or the rcode by type ("A: SERVFAIL").
    pub error: Option<String>,
}

/// The largest packet that crossed the path unfragmented, and how the search ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportMtu {
    /// The largest packet seen to cross, with its IP header. None when the target was silent.
    pub bytes: Option<u16>,
    /// full (this device's own MTU crossed), limited (a router said packets must be smaller),
    /// silent (larger packets vanish and no router says why) or unknown.
    #[serde(default)]
    pub finding: String,
    /// The router that said so.
    pub limited_by: Option<String>,
    /// The largest size tried: this device's own MTU.
    pub tried: Option<u16>,
}

/// The probes of a run that didn't send ICMP echo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportProbe {
    /// tcp.
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub port: u16,
    /// open (a SYN-ACK came back), closed (a reset came back) or filtered (nothing did).
    pub state: Option<String>,
}

/// Where a run was made from. A comparison only holds between runs of one origin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportOrigin {
    /// What made the run: `device` (a person's own install) today, `node` once nodes exist.
    /// A reader that meets a kind it doesn't know treats the run as from somewhere else.
    pub kind: String,
    /// The node's name. `null` is this device, the only origin until nodes exist.
    pub node: Option<String>,
    /// The node's id, which never changes. A report a device made has none, and the key is
    /// left out of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// The install that made the run, when the account handed it to this device. `null` is a
    /// run made here.
    pub install: Option<String>,
    /// The address probes left from.
    pub source_address: Option<String>,
    /// The address the internet saw this device as, when it was asked.
    pub public_address: Option<String>,
    /// The origin's tags as they were when the run was stored. The API writes them, whatever the
    /// sender put here, so a run made on this device has none. A reader that can't read them
    /// (a value that isn't text) leaves them out and still reads the run.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "or_none"
    )]
    pub tags: Option<BTreeMap<String, String>>,
    /// The origin's location, rounded to its precision, as it was when the run was stored. The
    /// API writes it. A location a reader can't read is left out.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "or_none"
    )]
    pub location: Option<DeclaredLocation>,
    /// The revision of the origin's tags and location the run was made under. A node writes the
    /// one its API last told it and keeps it with the run in its outbox, so the API tags the run
    /// as it was made and not as it is when the result arrives. The API checks it and writes the
    /// tags and location itself. A run made on this device has none.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "or_none"
    )]
    pub metadata_revision: Option<u32>,
}

/// A value that is there and isn't the shape this version reads is the same as one that is
/// missing: a report from a newer maker must never be the reason a run can't be shown.
fn or_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).ok())
}

impl ReportOrigin {
    fn of(run: &Run) -> Self {
        Self {
            kind: "device".into(),
            node: None,
            node_id: None,
            install: run.from.clone(),
            source_address: run.source.map(|address| address.to_string()),
            public_address: run.public_source.map(|address| address.to_string()),
            tags: None,
            location: None,
            metadata_revision: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    /// The version of this shape. Fields are only ever added within a version, and a reader
    /// ignores the ones it doesn't know. A change that breaks a reader raises the number.
    pub schema: u32,
    /// The run's id in history: what `rvrs show` and `rvrs save` take. Unique on the install
    /// that made the run, which is what an account tells runs apart by.
    pub id: String,
    pub tool: String,
    pub target: String,
    pub address: Option<String>,
    pub family: Option<String>,
    pub started_at: String,
    /// Where the run was made from.
    pub origin: ReportOrigin,
    /// reached, exhausted, stopped or failed.
    pub outcome: String,
    /// The run in one line of facts: "6 hops · 12.4 ms".
    pub summary: String,
    pub error: Option<String>,
    #[serde(default)]
    pub measured: Vec<Measurement>,
    pub hops: Option<Vec<ReportHop>>,
    /// DNS: the records of the reference answer.
    pub records: Option<Vec<ReportRecord>>,
    /// DNS: what each source answered and how it compares with the reference answer.
    pub sources: Option<Vec<ReportSource>>,
    /// What the probes were when they were not ICMP echo: TCP to a port, and what the port did.
    pub probe: Option<ReportProbe>,
    /// The path MTU, when the run looked for it.
    pub mtu: Option<ReportMtu>,
    /// The round trip of each probe to the destination in the order they were sent, in
    /// milliseconds, the newest [`RUN_SAMPLES`]. `null` in the list is a probe that got no
    /// reply. `null` for a tool that sends no probes.
    pub samples: Option<Vec<Option<f64>>>,
    /// Inferences only, each a sentence that says it is one.
    #[serde(default)]
    pub interpretation: Vec<String>,
    /// The result as one plain sentence.
    #[serde(default)]
    pub sentence: String,
}

/// The report's shape version. See `schema/run-report.schema.json`.
pub const SCHEMA: u32 = 1;

pub const SILENT_TARGET: &str =
    "The target sent no reply. It may drop ICMP, so a silent target isn't necessarily down.";

fn round(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

impl Report {
    pub fn of(run: &Run) -> Self {
        // A run that came as a report is that report, word for word.
        if let Some(report) = &run.report {
            let mut report = (**report).clone();
            if run.from.is_some() {
                report.origin.install = run.from.clone();
            }
            return report;
        }
        if run.tool == Tool::Ping {
            return Self::of_ping(run);
        }
        if run.tool == Tool::Dns {
            return Self::of_dns(run);
        }
        if run.tool == Tool::Bgp {
            return Self::of_bgp(run);
        }
        let outcome = run.outcome.clone().unwrap_or(Outcome::Stopped);
        let error = match &outcome {
            Outcome::Failed(message) => Some(message.clone()),
            _ => None,
        };
        let path = run.path();
        let mut measured = vec![Measurement {
            name: "hops".into(),
            value: path.len() as f64,
            unit: None,
        }];
        let mut interpretation = Vec::new();
        if let Some(hop) = run.destination_hop() {
            let stats = hop.stats();
            measured.push(Measurement {
                name: "sent".into(),
                value: stats.sent as f64,
                unit: None,
            });
            measured.push(Measurement {
                name: "received".into(),
                value: stats.received as f64,
                unit: None,
            });
            measured.push(Measurement {
                name: "loss".into(),
                value: round(stats.loss_fraction() * 100.0),
                unit: Some("%".into()),
            });
            let figures = [
                ("min", stats.min),
                ("avg", stats.mean),
                ("max", stats.max),
                ("jitter", stats.jitter),
                ("p50", stats.p50),
                ("p95", stats.p95),
            ];
            for (name, value) in figures {
                if let Some(value) = value {
                    measured.push(Measurement {
                        name: name.into(),
                        value: round(format::ms_value(value)),
                        unit: Some("ms".into()),
                    });
                }
            }
            // MTR sends enough packets to the destination to say how steady it is.
            if run.tool == Tool::Mtr {
                let verdict = Verdict::of(&stats);
                if verdict.grade != Grade::Collecting {
                    interpretation.push(format!("{}. {}", verdict.title, verdict.detail));
                }
            }
        } else if let Some(note) = run.tcp_filter_note() {
            interpretation.push(note);
        } else if outcome == Outcome::Exhausted && run.unreachable().is_none() {
            interpretation.push(SILENT_TARGET.to_string());
        }
        if let Some(note) = run.mtu_note() {
            interpretation.push(note);
        }
        let mtu = run.mtu.as_ref().map(|found| {
            if let Some(bytes) = found.mtu {
                measured.push(Measurement {
                    name: "path mtu".into(),
                    value: f64::from(bytes),
                    unit: Some("bytes".into()),
                });
            }
            ReportMtu {
                bytes: found.mtu,
                finding: match found.finding {
                    crate::mtu::MtuFinding::Full => "full",
                    crate::mtu::MtuFinding::Limited => "limited",
                    crate::mtu::MtuFinding::Silent => "silent",
                    crate::mtu::MtuFinding::Unknown => "unknown",
                }
                .into(),
                limited_by: found.limited_by.map(|address| address.to_string()),
                tried: found.tried,
            }
        });
        let probe = run.tcp.as_ref().map(|tcp| {
            if let Some(micros) = tcp.connect_micros {
                measured.push(Measurement {
                    name: "connect".into(),
                    value: round(micros as f64 / 1000.0),
                    unit: Some("ms".into()),
                });
            }
            if let Some(mss) = tcp.mss {
                measured.push(Measurement {
                    name: "mss".into(),
                    value: f64::from(mss),
                    unit: Some("bytes".into()),
                });
            }
            ReportProbe {
                protocol: "tcp".into(),
                port: tcp.port,
                state: tcp.state.map(|state| state.word().to_string()),
            }
        });
        if run.tool == Tool::Mtr {
            if let Some(note) = run.rate_limit_note() {
                interpretation.push(note);
            }
        }
        let hops = path
            .iter()
            .map(|hop| {
                let stats = hop.stats();
                let address = hop.address();
                let network = address.and_then(|address| run.network(address));
                ReportHop {
                    ttl: hop.ttl,
                    address: address.map(|address| address.to_string()),
                    hostname: address
                        .and_then(|address| run.name(address))
                        .map(str::to_string),
                    asn: network.map(|network| network.label()),
                    network: network.map(|network| network.name.clone()),
                    loss_percent: Some(round(stats.loss_fraction() * 100.0)),
                    avg_milliseconds: stats.mean.map(|mean| round(format::ms_value(mean))),
                    min_milliseconds: stats.min.map(|min| round(format::ms_value(min))),
                    max_milliseconds: stats.max.map(|max| round(format::ms_value(max))),
                    jitter_milliseconds: stats.jitter.map(|jitter| round(format::ms_value(jitter))),
                    sent: Some(stats.sent as u32),
                    received: Some(stats.received as u32),
                    samples: Some(samples_of(&hop.probes, HOP_SAMPLES)),
                }
            })
            .collect();
        Self {
            schema: SCHEMA,
            id: run.id.clone(),
            origin: ReportOrigin::of(run),
            tool: run.tool.title().to_string(),
            target: run.target.clone(),
            address: run.destination.map(|address| address.to_string()),
            family: run
                .resolved_family()
                .map(|family| family.label().to_string()),
            started_at: format::iso8601(run.started_at),
            outcome: outcome.word().to_string(),
            summary: run.summary(),
            sentence: sentence(run, &outcome),
            error,
            measured,
            hops: Some(hops),
            records: None,
            sources: None,
            probe,
            mtu,
            samples: run
                .destination_hop()
                .map(|hop| samples_of(&hop.probes, RUN_SAMPLES)),
            interpretation,
        }
    }

    /// A BGP lookup: visibility, paths and the origin AS as measurements, and the verdict as
    /// the interpretation, since it joins observation and reading.
    fn of_bgp(run: &Run) -> Self {
        let outcome = run.outcome.clone().unwrap_or(Outcome::Stopped);
        let error = match &outcome {
            Outcome::Failed(message) => Some(message.clone()),
            _ => None,
        };
        let mut measured = Vec::new();
        let mut interpretation = Vec::new();
        let mut sentence = error
            .clone()
            .unwrap_or_else(|| format!("No route information for {}.", run.target));
        let mut summary = run.summary();
        let mut address = None;
        if let Some(bgp) = run.bgp.as_ref().filter(|bgp| !bgp.is_empty()) {
            let mut measure = |name: &str, value: f64| {
                measured.push(Measurement {
                    name: name.into(),
                    value,
                    unit: None,
                });
            };
            if let Some(seeing) = bgp.seeing {
                measure("peers seeing", seeing as f64);
            }
            if let Some(total) = bgp.total_peers {
                measure("peers total", total as f64);
            }
            if bgp.path_count > 0 {
                measure("distinct paths", bgp.path_count as f64);
            }
            if let Some(origin) = bgp.origin() {
                measure("origin asn", f64::from(origin.asn));
            }
            if let Some(network) = &bgp.network {
                if let Some(v4) = network.v4_count {
                    measure("ipv4 prefixes", v4 as f64);
                }
                if let Some(v6) = network.v6_count {
                    measure("ipv6 prefixes", v6 as f64);
                }
            }
            let verdict = bgp.verdict();
            interpretation.push(format!("{}. {}", verdict.title, verdict.detail));
            address = bgp.address.clone();
            if error.is_none() {
                sentence = format!("{}. {}", verdict.title, verdict.lead);
                summary = bgp.headline();
            }
        }
        Self {
            schema: SCHEMA,
            id: run.id.clone(),
            origin: ReportOrigin::of(run),
            tool: run.tool.title().to_string(),
            target: run.target.clone(),
            address,
            family: None,
            started_at: format::iso8601(run.started_at),
            outcome: outcome.word().to_string(),
            summary,
            error,
            measured,
            hops: None,
            records: None,
            sources: None,
            probe: None,
            mtu: None,
            samples: None,
            interpretation,
            sentence,
        }
    }

    /// A DNS lookup: how many addresses and sources, the reference answer's records, and the
    /// verdict as the interpretation.
    fn of_dns(run: &Run) -> Self {
        let outcome = run.outcome.clone().unwrap_or(Outcome::Stopped);
        let error = match &outcome {
            Outcome::Failed(message) => Some(message.clone()),
            _ => None,
        };
        let mut measured = Vec::new();
        let mut interpretation = Vec::new();
        let mut records = None;
        let mut sources = None;
        let mut sentence = error
            .clone()
            .unwrap_or_else(|| format!("No answer for {}.", run.target));
        let mut summary = run.summary();
        if let Some(dns) = run.dns.as_ref().filter(|dns| !dns.is_empty()) {
            measured.push(Measurement {
                name: "addresses".into(),
                value: dns.addresses().len() as f64,
                unit: None,
            });
            measured.push(Measurement {
                name: "sources".into(),
                value: dns.sources.len() as f64,
                unit: None,
            });
            if let Some(fastest) = dns.fastest_ms() {
                measured.push(Measurement {
                    name: "fastest".into(),
                    value: round(fastest),
                    unit: Some("ms".into()),
                });
            }
            let diff = Diff::of(dns);
            sources = Some(
                dns.ordered_sources()
                    .into_iter()
                    .map(|source| {
                        let mut addresses = source.values(types::A);
                        addresses.extend(source.values(types::AAAA));
                        let failures: Vec<String> = source
                            .failures
                            .iter()
                            .map(|(kind, rcode)| format!("{kind}: {rcode}"))
                            .collect();
                        ReportSource {
                            name: source.label.clone(),
                            kind: source.kind.title().to_lowercase(),
                            server: Some(source.server.clone()),
                            status: diff.status(source).title().to_lowercase(),
                            milliseconds: source.rtt_ms.map(round),
                            addresses,
                            error: source
                                .error
                                .clone()
                                .or_else(|| (!failures.is_empty()).then(|| failures.join(", "))),
                        }
                    })
                    .collect(),
            );
            let verdict = dns.verdict();
            // The screen's verdict points at its Compare tab. A report has none.
            let detail = verdict.detail.replace(" Compare shows why.", "");
            interpretation.push(format!("{}. {detail}", verdict.title));
            records = Some(
                dns.reference()
                    .map(|reference| {
                        reference
                            .records
                            .iter()
                            .map(|record| ReportRecord {
                                type_name: record.type_name(),
                                name: record.name.clone(),
                                ttl: Some(record.ttl),
                                value: record.value.clone(),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            );
            if error.is_none() {
                let first = detail.split(". ").next().unwrap_or(&detail);
                sentence = format!(
                    "{}. {}{}",
                    verdict.title,
                    first,
                    if first.ends_with('.') { "" } else { "." }
                );
                // The status line of a finished lookup, as the result screen shows it.
                summary = dns.headline();
            }
        }
        Self {
            schema: SCHEMA,
            id: run.id.clone(),
            origin: ReportOrigin::of(run),
            tool: run.tool.title().to_string(),
            target: run.target.clone(),
            address: None,
            family: None,
            started_at: format::iso8601(run.started_at),
            outcome: outcome.word().to_string(),
            summary,
            error,
            measured,
            hops: None,
            records,
            sources,
            probe: None,
            mtu: None,
            samples: None,
            interpretation,
            sentence,
        }
    }

    /// A ping: the packet figures, no hops, and the verdict as the interpretation.
    fn of_ping(run: &Run) -> Self {
        let outcome = run.outcome.clone().unwrap_or(Outcome::Stopped);
        let error = match &outcome {
            Outcome::Failed(message) => Some(message.clone()),
            _ => None,
        };
        let stats = run.ping_stats();
        let target = run
            .destination
            .map_or_else(|| run.target.clone(), |address| address.to_string());
        let mut measured = Vec::new();
        let mut interpretation = Vec::new();
        let mut sentence = error
            .clone()
            .unwrap_or_else(|| format!("No reply from {target}."));
        if stats.sent > 0 {
            measured.push(Measurement {
                name: "sent".into(),
                value: stats.sent as f64,
                unit: None,
            });
            measured.push(Measurement {
                name: "received".into(),
                value: stats.received as f64,
                unit: None,
            });
            measured.push(Measurement {
                name: "loss".into(),
                value: round(stats.loss_fraction() * 100.0),
                unit: Some("%".into()),
            });
            let figures = [
                ("min", stats.min),
                ("avg", stats.mean),
                ("max", stats.max),
                ("jitter", stats.jitter),
                ("p50", stats.p50),
                ("p95", stats.p95),
            ];
            for (name, value) in figures {
                if let Some(value) = value {
                    measured.push(Measurement {
                        name: name.into(),
                        value: round(format::ms_value(value)),
                        unit: Some("ms".into()),
                    });
                }
            }
            if error.is_none() {
                sentence = format!(
                    "{} packets to {target}, {} loss",
                    stats.sent,
                    format::percent(stats.loss_fraction())
                );
                if let Some(mean) = stats.mean {
                    sentence.push_str(&format!(", {} average", format::ms(mean)));
                }
                sentence.push('.');
            }
            if stats.received == 0 {
                interpretation.push(SILENT_TARGET.to_string());
            } else {
                let verdict = Verdict::of(&stats);
                if verdict.grade != Grade::Collecting {
                    interpretation.push(format!("{}. {}", verdict.title, verdict.detail));
                    if error.is_none() {
                        sentence.push_str(&format!(" {}.", verdict.title));
                    }
                }
            }
        }
        Self {
            schema: SCHEMA,
            id: run.id.clone(),
            origin: ReportOrigin::of(run),
            tool: run.tool.title().to_string(),
            target: run.target.clone(),
            address: run.destination.map(|address| address.to_string()),
            family: run
                .resolved_family()
                .map(|family| family.label().to_string()),
            started_at: format::iso8601(run.started_at),
            outcome: outcome.word().to_string(),
            summary: run.summary(),
            error,
            measured,
            hops: None,
            records: None,
            sources: None,
            probe: None,
            mtu: None,
            samples: Some(samples_of(run.packets(), RUN_SAMPLES)),
            interpretation,
            sentence,
        }
    }

    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The figures as one line of facts, the way the status lines join them:
    /// "20 sent / 20 received · 0.0% loss · 10.5 ms min · 16.9 ms max · 12.3 ms avg". A figure
    /// this version doesn't know joins the end as value and name. The percentiles are left to
    /// the chart that draws them.
    pub fn figure_line(&self) -> String {
        let find = |name: &str| self.measured.iter().find(|figure| figure.name == name);
        let mut used = vec!["p50", "p95"];
        let mut parts: Vec<String> = Vec::new();
        if let Some(hops) = find("hops") {
            let noun = if hops.value == 1.0 { "hop" } else { "hops" };
            parts.push(format!("{} {noun}", figure_text(hops.value, None)));
            used.push("hops");
        }
        if let (Some(sent), Some(received)) = (find("sent"), find("received")) {
            parts.push(format!(
                "{} sent / {} received",
                figure_text(sent.value, None),
                figure_text(received.value, None)
            ));
            used.extend(["sent", "received"]);
        }
        if let Some(loss) = find("loss") {
            parts.push(format!("{} loss", figure_text(loss.value, Some("%"))));
            used.push("loss");
        }
        for name in ["min", "max", "avg", "jitter"] {
            if let Some(found) = find(name) {
                parts.push(format!(
                    "{} {name}",
                    figure_text(found.value, found.unit.as_deref())
                ));
                used.push(name);
            }
        }
        for figure in &self.measured {
            if used.contains(&figure.name.as_str()) {
                continue;
            }
            // An AS number is a name, so it takes no thousands separator.
            match figure.name.strip_suffix(" asn") {
                Some(name) => parts.push(format!("AS{} {name}", figure.value as u64)),
                None => parts.push(format!(
                    "{} {}",
                    figure_text(figure.value, figure.unit.as_deref()),
                    figure.name
                )),
            }
        }
        parts.join(" \u{00B7} ")
    }

    /// One measured figure by its name. None is a figure this run didn't measure.
    pub fn figure(&self, name: &str) -> Option<f64> {
        self.measured
            .iter()
            .find(|measurement| measurement.name == name)
            .map(|measurement| measurement.value)
    }
}

/// A figure with its unit, as the status lines write it: "12.4 ms", "0.0%", "1,204".
/// Latency and loss always have one decimal. A count has none.
pub fn figure_text(value: f64, unit: Option<&str>) -> String {
    let number = if matches!(unit, Some("ms" | "%")) || value.fract() != 0.0 {
        format!("{value:.1}")
    } else {
        let digits = format!("{}", value.abs() as u64);
        let mut grouped = String::new();
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        if value < 0.0 {
            format!("-{grouped}")
        } else {
            grouped
        }
    };
    match unit {
        None => number,
        Some("%") => format!("{number}%"),
        Some(unit) => format!("{number} {unit}"),
    }
}

/// The run as one plain sentence, without middle dots, the way Siri says it in the app.
pub fn sentence(run: &Run, outcome: &Outcome) -> String {
    if let Outcome::Failed(message) = outcome {
        return message.clone();
    }
    let target = run
        .destination
        .map_or_else(|| run.target.clone(), |address| address.to_string());
    let count = run.path_len();
    let hops = if count == 1 {
        "1 hop".to_string()
    } else {
        format!("{count} hops")
    };
    if let Some((ttl, _)) = run.unreachable() {
        return format!("Hop {ttl} reports {target} unreachable.");
    }
    if run.tool == Tool::Mtr {
        // The app's sentence for MTR and ping: the packets, then the verdict's title.
        let Some(stats) = run
            .destination_hop()
            .map(|hop| hop.stats())
            .filter(|stats| stats.sent > 0)
        else {
            return format!("No reply from {target}.");
        };
        let mut text = format!(
            "{} packets to {target}, {} loss",
            stats.sent,
            format::percent(stats.loss_fraction())
        );
        if let Some(mean) = stats.mean {
            text.push_str(&format!(", {} average", format::ms(mean)));
        }
        text.push('.');
        let verdict = Verdict::of(&stats);
        if verdict.grade != Grade::Collecting {
            text.push_str(&format!(" {}.", verdict.title));
        }
        return text;
    }
    if let Some(tcp) = &run.tcp {
        let port = tcp.port;
        return match (outcome, run.total_rtt(), tcp.state) {
            (Outcome::Exhausted, _, _) => {
                format!("No reply from {target} on TCP port {port} after {hops}.")
            }
            (_, Some(rtt), Some(state)) => format!(
                "Reached {target} on TCP port {port} in {hops}, {} round trip. The port is {}.",
                format::ms(rtt),
                state.word()
            ),
            _ => format!("Stopped after {hops}."),
        };
    }
    match (outcome, run.total_rtt()) {
        (Outcome::Exhausted, _) => format!("No reply from {target} after {hops}."),
        (_, Some(rtt)) => format!(
            "Reached {target} in {hops}, {} round trip.",
            format::ms(rtt)
        ),
        _ => format!("Stopped after {hops}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Ending, Event, ReplyKind};
    use crate::model::tests::{reached_run, reply};
    use crate::model::{Network, Tool};
    use crate::FamilyChoice;

    #[test]
    fn a_reached_run_reports_what_was_measured_and_infers_nothing() {
        let mut run = reached_run();
        run.started_at = 1_794_735_000;
        run.names
            .insert("1.1.1.1".parse().unwrap(), "one.one.one.one".into());
        run.networks.insert(
            "1.1.1.1".parse().unwrap(),
            Some(Network {
                number: 13335,
                name: "Cloudflare, Inc.".into(),
                country: None,
                city: None,
                prefix: None,
            }),
        );
        let report = Report::of(&run);
        assert_eq!(report.tool, "Traceroute");
        assert_eq!(report.outcome, "reached");
        assert_eq!(report.summary, "3 hops \u{00B7} 12.5 ms");
        assert_eq!(
            report.sentence,
            "Reached 1.1.1.1 in 3 hops, 12.5 ms round trip."
        );
        assert_eq!(report.family.as_deref(), Some("IPv4"));
        assert_eq!(report.started_at, "2026-11-15T09:30:00Z");
        assert!(report.interpretation.is_empty());
        let names: Vec<&str> = report.measured.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["hops", "sent", "received", "loss", "min", "avg", "max", "jitter", "p50", "p95"]
        );
        let hops = report.hops.as_ref().unwrap();
        assert_eq!(hops[1].address, None);
        assert_eq!(hops[1].loss_percent, Some(100.0));
        assert_eq!(hops[2].asn.as_deref(), Some("AS13335"));
        assert_eq!(hops[2].avg_milliseconds, Some(12.5));
    }

    /// Every key of `value` is one the schema names, all the way down.
    fn named_by_schema(value: &serde_json::Value, schema: &serde_json::Value, path: &str) {
        match value {
            serde_json::Value::Object(fields) => {
                for (key, inner) in fields {
                    // A map, such as the tags, names its values and not its keys.
                    let known = if schema["properties"][key].is_object() {
                        &schema["properties"][key]
                    } else {
                        &schema["additionalProperties"]
                    };
                    assert!(known.is_object(), "the schema doesn't name {path}{key}");
                    named_by_schema(inner, known, &format!("{path}{key}."));
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    named_by_schema(item, &schema["items"], path);
                }
            }
            _ => {}
        }
    }

    fn schema_folder() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema")
    }

    fn read(path: std::path::PathBuf) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
    }

    /// Every fixture by its file name, in order.
    fn fixtures() -> Vec<(String, serde_json::Value)> {
        let mut names: Vec<String> = std::fs::read_dir(schema_folder().join("fixtures"))
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with(".json"))
            .collect();
        names.sort();
        names
            .into_iter()
            .map(|name| {
                let report = read(schema_folder().join("fixtures").join(&name));
                (name, report)
            })
            .collect()
    }

    /// An MTR of twelve rounds over three hops whose middle hop drops every third reply.
    fn mtr_fixture_run() -> Run {
        let mut run = Run::new(Tool::Mtr, "example.com", FamilyChoice::Auto, 0, 30);
        run.id = "mtr9xa".into();
        run.started_at = 1_794_735_000;
        run.destination = Some("203.0.113.80".parse().unwrap());
        run.source = Some("192.0.2.10".parse().unwrap());
        let answer = |ttl: u8, round: u32, from: &str, ms: u64, kind: ReplyKind| Event::MtrReply {
            ttl,
            round,
            from: from.parse().unwrap(),
            rtt: std::time::Duration::from_millis(ms),
            kind,
        };
        for round in 0..12 {
            for ttl in 1..=3 {
                run.apply(&Event::MtrSent { ttl, round });
            }
            run.apply(&answer(1, round, "192.0.2.1", 1, ReplyKind::Intermediate));
            if round % 3 == 0 {
                run.apply(&Event::MtrLost { ttl: 2, round });
            } else {
                run.apply(&answer(
                    2,
                    round,
                    "198.51.100.1",
                    8,
                    ReplyKind::Intermediate,
                ));
            }
            let ms = 12 + u64::from(round % 2);
            run.apply(&answer(
                3,
                round,
                "203.0.113.80",
                ms,
                ReplyKind::Destination,
            ));
        }
        run.apply(&Event::Finished(Ending::Reached(3)));
        run.networks.insert(
            "203.0.113.80".parse().unwrap(),
            Some(Network {
                number: 64500,
                name: "Example Net".into(),
                country: None,
                city: None,
                prefix: None,
            }),
        );
        run
    }

    fn bgp_fixture_run() -> Run {
        let mut run = Run::new(Tool::Bgp, "example.com", FamilyChoice::Auto, 0, 0);
        run.id = "bgp7ka".into();
        run.started_at = 1_794_735_000;
        run.bgp = Some(crate::bgp::result::fixtures::routed());
        run.outcome = Some(Outcome::Reached);
        run
    }

    #[test]
    fn the_schema_names_every_field_of_a_report_and_of_the_fixtures() {
        let folder = schema_folder();
        let schema = read(folder.join("run-report.schema.json"));
        let mut run = reached_run();
        run.id = "tr4c3a".into();
        run.started_at = 1_794_735_000;
        run.source = Some("192.0.2.10".parse().unwrap());
        // These fixtures are the reports of runs built here, so the two can't drift apart.
        for (name, run) in [
            ("traceroute", run),
            ("mtr", mtr_fixture_run()),
            ("bgp", bgp_fixture_run()),
        ] {
            let report: serde_json::Value = serde_json::from_str(&Report::of(&run).json()).unwrap();
            let fixture = folder.join(format!("fixtures/{name}.json"));
            if std::env::var("RVRS_WRITE_FIXTURES").is_ok() {
                std::fs::write(&fixture, Report::of(&run).json() + "\n").unwrap();
            }
            assert_eq!(read(fixture), report, "{name}");
        }
        let required = schema["required"].as_array().unwrap();
        let all = fixtures();
        let names: Vec<&str> = all.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "bgp.json",
                "dns.json",
                "iperf3.json",
                "mtr.json",
                "ping.json",
                "sparse.json",
                "tagged.json",
                "traceroute.json"
            ]
        );
        for (name, report) in &all {
            named_by_schema(report, &schema, "");
            for key in required {
                assert!(
                    !report[key.as_str().unwrap()].is_null(),
                    "{name} lacks {key}"
                );
            }
            assert_eq!(report["schema"], SCHEMA);
        }
    }

    /// A report as JSON without its nulls and with every number a float, so a field left out
    /// and a field set to null read the same, as do 5 and 5.0.
    fn plain(value: &serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        match value {
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .filter(|(_, inner)| !inner.is_null())
                    .map(|(key, inner)| (key.clone(), plain(inner)))
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(plain).collect()),
            Value::Number(number) => serde_json::json!(number.as_f64().unwrap()),
            other => other.clone(),
        }
    }

    #[test]
    fn every_fixture_is_read_and_written_back_unchanged() {
        for (name, fixture) in fixtures() {
            let report: Report = serde_json::from_value(fixture.clone())
                .unwrap_or_else(|error| panic!("{name} can't be read: {error}"));
            let written = serde_json::to_value(&report).unwrap();
            // Samples keep their nulls: a null there is a probe that got no reply.
            let keep_samples = |value: &serde_json::Value| value["samples"].clone();
            assert_eq!(plain(&written), plain(&fixture), "{name}");
            assert_eq!(
                keep_samples(&written).as_array().map(Vec::len),
                keep_samples(&fixture).as_array().map(Vec::len),
                "{name}"
            );
        }
    }

    #[test]
    fn every_fixture_becomes_a_run_that_says_what_its_maker_said() {
        for (name, fixture) in fixtures() {
            let report: Report = serde_json::from_value(fixture).unwrap();
            let run = Run::from_report(&report, "abc234", Some("iPhone".into()))
                .unwrap_or_else(|| panic!("{name} names a tool this version doesn't know"));
            assert_eq!(run.id, "abc234");
            assert_eq!(run.target, report.target, "{name}");
            assert_eq!(run.summary(), report.summary, "{name}");
            assert_eq!(run.reading(), Some(report.interpretation.as_slice()));
            assert_eq!(run.outcome.as_ref().unwrap().word(), report.outcome);
            assert_eq!(format::iso8601(run.started_at), report.started_at);
            // The report of such a run is the report it came as, with where it came from.
            let again = Report::of(&run);
            assert_eq!(again.origin.install.as_deref(), Some("iPhone"));
            assert_eq!(again.measured, report.measured, "{name}");
            assert_eq!(again.hops, report.hops, "{name}");
            assert_eq!(again.sentence, report.sentence, "{name}");
            // Hops come back with the figures the report states.
            for (hop, reported) in run.hops.iter().zip(report.hops.iter().flatten()) {
                let stats = hop.stats();
                assert_eq!(
                    stats.mean.map(|mean| round(format::ms_value(mean))),
                    reported.avg_milliseconds,
                    "{name} hop {}",
                    hop.ttl
                );
                if let Some(sent) = reported.sent {
                    assert_eq!(stats.sent, sent as usize);
                }
                if let Some(loss) = reported.loss_percent {
                    assert_eq!(round(stats.loss_fraction() * 100.0), loss);
                }
            }
        }
    }

    #[test]
    fn an_origin_keeps_its_tags_and_location_and_a_reader_never_needs_them() {
        let fixture = read(schema_folder().join("fixtures/tagged.json"));
        let report: Report = serde_json::from_value(fixture.clone()).unwrap();
        let tags = report.origin.tags.as_ref().unwrap();
        assert_eq!(tags["isp"], "telia");
        let location = report.origin.location.as_ref().unwrap();
        assert_eq!((location.lat, location.lon), (55.5, 8.5));
        assert_eq!(location.place.as_deref(), Some("Esbjerg, DK"));
        assert_eq!(location.precision, "city");
        assert_eq!(report.origin.metadata_revision, Some(3));
        assert_eq!(
            report.origin.node_id.as_deref(),
            Some("7b0c5a2e-8d41-4f6a-9c3e-2a5d1e9f0b64")
        );
        // The run is shown whatever the origin says about itself.
        let run = Run::from_report(&report, "abc234", Some("esb-1".into())).unwrap();
        assert_eq!(run.summary(), report.summary);

        // A precision left out is the coarsest.
        let mut bare = fixture.clone();
        bare["origin"]["location"] = serde_json::json!({ "lat": 55.5, "lon": 8.5 });
        let report: Report = serde_json::from_value(bare).unwrap();
        assert_eq!(report.origin.location.unwrap().precision, "city");

        // Tags or a location of a shape this version doesn't read are left out, not an error.
        let mut odd = fixture;
        odd["origin"]["tags"] = serde_json::json!({ "isp": 1 });
        odd["origin"]["location"] = serde_json::json!("Esbjerg");
        odd["origin"]["metadataRevision"] = serde_json::json!("three");
        let report: Report = serde_json::from_value(odd).unwrap();
        assert_eq!(report.origin.tags, None);
        assert_eq!(report.origin.location, None);
        assert_eq!(report.origin.metadata_revision, None);
        assert_eq!(report.origin.node.as_deref(), Some("esb-1"));

        // A run made here writes neither.
        let written = serde_json::to_value(Report::of(&reached_run())).unwrap();
        assert!(written["origin"].get("tags").is_none());
        assert!(written["origin"].get("location").is_none());
        assert!(written["origin"].get("metadataRevision").is_none());
    }

    #[test]
    fn figures_join_into_one_line_of_facts() {
        let line = |name: &str| {
            let report: Report =
                serde_json::from_value(read(schema_folder().join(format!("fixtures/{name}.json"))))
                    .unwrap();
            report.figure_line()
        };
        assert_eq!(
            line("mtr"),
            "3 hops \u{00B7} 12 sent / 12 received \u{00B7} 0.0% loss \u{00B7} 12.0 ms min \u{00B7} 13.0 ms max \u{00B7} 12.5 ms avg \u{00B7} 1.0 ms jitter"
        );
        assert_eq!(
            line("bgp"),
            "318 peers seeing \u{00B7} 320 peers total \u{00B7} 3 distinct paths \u{00B7} AS64500 origin \u{00B7} 1 ipv4 prefixes \u{00B7} 1 ipv6 prefixes"
        );
        assert_eq!(
            line("iperf3"),
            "968.2 Mbps download peak \u{00B7} 902.4 Mbps upload peak"
        );
        assert_eq!(line("sparse"), "3 hops \u{00B7} 14.8 ms avg");
        assert_eq!(figure_text(1204.0, None), "1,204");
        assert_eq!(figure_text(0.0, Some("%")), "0.0%");
    }

    #[test]
    fn a_figure_a_report_leaves_out_is_absent_and_never_zero() {
        let fixture = read(schema_folder().join("fixtures/sparse.json"));
        let report: Report = serde_json::from_value(fixture).unwrap();
        let run = Run::from_report(&report, "abc234", None).unwrap();
        assert_eq!(run.path_len(), 3);
        let first = run.hops[0].stats();
        assert_eq!(first.mean, Some(std::time::Duration::from_micros(1200)));
        assert_eq!(first.min, None);
        assert_eq!(first.max, None);
        assert_eq!(first.jitter, None);
        assert_eq!(first.p95, None);
        assert!(run.hops[0].probes.is_empty());
        // A hop with no address and no figures is a silent hop.
        assert!(!run.hops[1].did_reply());
        assert_eq!(run.hops[1].stats().mean, None);
        assert!(run.hops[2].is_destination);
        assert_eq!(run.destination_hop().unwrap().address(), run.destination);
    }

    #[test]
    fn a_report_with_fewer_samples_than_probes_keeps_its_own_figures() {
        let mut report = Report::of(&mtr_fixture_run());
        // A long MTR: 300 rounds sent, the newest 12 kept as samples.
        for hop in report.hops.iter_mut().flatten() {
            hop.sent = Some(300);
            hop.received = Some(290);
            hop.loss_percent = Some(3.3);
        }
        report.measured.retain(|figure| figure.name != "p50");
        for figure in &mut report.measured {
            match figure.name.as_str() {
                "sent" => figure.value = 300.0,
                "received" => figure.value = 290.0,
                "loss" => figure.value = 3.3,
                _ => {}
            }
        }
        let run = Run::from_report(&report, "abc234", None).unwrap();
        let stats = run.destination_hop().unwrap().stats();
        assert_eq!(stats.sent, 300);
        assert_eq!(stats.received, 290);
        assert_eq!(round(stats.loss_fraction() * 100.0), 3.3);
        // The median isn't in the report, and twelve samples of 300 can't stand in for it.
        assert_eq!(stats.p50, None);
        assert!(stats.p95.is_some());
        assert_eq!(run.first_round, 288);
        assert_eq!(run.hops[0].probes.len(), 12);
    }

    #[test]
    fn a_run_stopped_by_hand_still_has_the_destination_it_reached() {
        let mut report = Report::of(&mtr_fixture_run());
        report.outcome = "stopped".into();
        let run = Run::from_report(&report, "abc234", None).unwrap();
        assert_eq!(run.outcome, Some(Outcome::Stopped));
        let destination = run
            .destination_hop()
            .expect("the last hop is the destination");
        assert_eq!(destination.stats().sent, 12);
        // A path that never got there has none.
        report.address = Some("203.0.113.99".into());
        let short = Run::from_report(&report, "abc234", None).unwrap();
        assert!(short.destination_hop().is_none());
    }

    #[test]
    fn a_report_from_a_newer_surface_is_read_with_what_this_version_knows() {
        let mut fixture = read(schema_folder().join("fixtures/ping.json"));
        fixture["schedule"] = serde_json::json!("sch_1");
        fixture["origin"]["kind"] = serde_json::json!("somewhere-new");
        let report: Report = serde_json::from_value(fixture.clone()).unwrap();
        assert_eq!(report.origin.kind, "somewhere-new");
        assert!(Run::from_report(&report, "abc234", None).is_some());
        fixture["tool"] = serde_json::json!("Port Scan");
        let unknown: Report = serde_json::from_value(fixture).unwrap();
        assert!(Run::from_report(&unknown, "abc234", None).is_none());
    }

    #[test]
    fn the_json_uses_the_apps_field_names() {
        let json = Report::of(&reached_run()).json();
        for key in [
            "\"startedAt\"",
            "\"lossPercent\"",
            "\"avgMilliseconds\"",
            "\"interpretation\"",
            "\"sentence\"",
            "\"measured\"",
            "\"id\"",
            "\"origin\"",
            "\"sourceAddress\"",
            "\"publicAddress\"",
        ] {
            assert!(json.contains(key), "{key} missing");
        }
    }

    #[test]
    fn a_report_names_its_run_and_where_it_was_made_from() {
        let mut run = reached_run();
        run.source = Some("192.0.2.7".parse().unwrap());
        run.public_source = Some("203.0.113.9".parse().unwrap());
        let report = Report::of(&run);
        assert_eq!(report.id, run.id);
        // Until nodes exist every run is made from this device.
        assert_eq!(report.origin.kind, "device");
        assert_eq!(report.origin.node, None);
        assert_eq!(report.schema, 1);
        assert_eq!(report.origin.source_address.as_deref(), Some("192.0.2.7"));
        assert_eq!(report.origin.public_address.as_deref(), Some("203.0.113.9"));
        assert!(report.sources.is_none());
    }

    #[test]
    fn a_silent_target_is_an_inference_worded_as_one() {
        let mut run = Run::new(Tool::Traceroute, "192.0.2.1", FamilyChoice::Auto, 1, 3);
        run.destination = Some("192.0.2.1".parse().unwrap());
        run.apply(&reply(1, 0, "10.0.0.1", 2.0, ReplyKind::Intermediate));
        run.apply(&Event::Finished(Ending::Exhausted));
        let report = Report::of(&run);
        assert_eq!(report.outcome, "exhausted");
        assert_eq!(report.sentence, "No reply from 192.0.2.1 after 1 hop.");
        assert_eq!(report.interpretation, vec![SILENT_TARGET.to_string()]);
    }

    #[test]
    fn a_ping_reports_packets_and_the_verdict_and_no_hops() {
        let mut run = Run::new(Tool::Ping, "one.one.one.one", FamilyChoice::Auto, 1, 64);
        run.destination = Some("1.1.1.1".parse().unwrap());
        for seq in 0..12u32 {
            run.apply(&Event::PingSent { seq });
            run.apply(&Event::PingReply {
                seq,
                from: "1.1.1.1".parse().unwrap(),
                rtt: std::time::Duration::from_millis(10),
                ttl: Some(57),
            });
        }
        run.apply(&Event::Finished(Ending::Reached(64)));
        let report = Report::of(&run);
        assert_eq!(report.tool, "Ping");
        assert_eq!(report.outcome, "reached");
        assert_eq!(
            report.summary,
            "12 pkt \u{00B7} 0.0% loss \u{00B7} 10.0 ms avg"
        );
        assert!(report.hops.is_none());
        assert_eq!(
            report.interpretation,
            vec!["Stable. No loss over 12 packets. p95 10.0 ms, jitter 0.0 ms."]
        );
        assert_eq!(
            report.sentence,
            "12 packets to 1.1.1.1, 0.0% loss, 10.0 ms average. Stable."
        );
        assert_eq!(report.measured[0].name, "sent");
    }

    #[test]
    fn an_mtr_reports_hops_the_verdict_and_the_rate_limit_note() {
        let mut run = crate::model::tests::mtr_run(12, |round| round % 3 == 0);
        run.apply(&Event::Finished(Ending::Reached(3)));
        let report = Report::of(&run);
        assert_eq!(report.tool, "MTR");
        assert_eq!(report.outcome, "reached");
        assert_eq!(
            report.summary,
            "12 pkt \u{00B7} 0.0% loss \u{00B7} 12.0 ms avg"
        );
        assert_eq!(
            report.sentence,
            "12 packets to 93.184.216.34, 0.0% loss, 12.0 ms average. Stable."
        );
        let hops = report.hops.as_ref().unwrap();
        assert_eq!(hops.len(), 3);
        assert_eq!(hops[1].loss_percent, Some(33.3));
        assert_eq!(report.interpretation.len(), 2);
        assert!(report.interpretation[0].starts_with("Stable. No loss over 12 packets."));
        assert!(report.interpretation[1].starts_with("Hop 2 answers fewer probes"));
    }

    #[test]
    fn a_bgp_lookup_reports_visibility_the_origin_and_the_verdict() {
        let mut run = Run::new(Tool::Bgp, "example.com", FamilyChoice::Auto, 0, 0);
        let mut bgp = crate::bgp::result::fixtures::routed();
        bgp.origins[0].holder = Some("Example Net, Inc.".to_string());
        run.bgp = Some(bgp);
        run.outcome = Some(Outcome::Reached);
        let report = Report::of(&run);
        assert_eq!(report.tool, "BGP");
        assert_eq!(report.address.as_deref(), Some("192.0.2.10"));
        assert_eq!(
            report.summary,
            "AS64500 \u{00B7} Valid \u{00B7} 318/320 peers"
        );
        let measured: Vec<(&str, f64)> = report
            .measured
            .iter()
            .map(|measurement| (measurement.name.as_str(), measurement.value))
            .collect();
        assert_eq!(
            measured,
            [
                ("peers seeing", 318.0),
                ("peers total", 320.0),
                ("distinct paths", 3.0),
                ("origin asn", 64500.0),
                ("ipv4 prefixes", 1.0),
                ("ipv6 prefixes", 1.0)
            ]
        );
        // The first sentence is kept whole, full stop in the holder's name and all.
        assert_eq!(
            report.sentence,
            "Routed and authorized. 192.0.2.0/24 is announced by AS64500 Example Net, Inc., seen by 318 of 320 collector peers."
        );
        assert!(report.interpretation[0].ends_with("A ROA authorizes this origin AS."));
        assert!(report.hops.is_none() && report.records.is_none());

        let mut failed = Run::new(Tool::Bgp, "example.invalid", FamilyChoice::Auto, 0, 0);
        failed.bgp = Some(crate::bgp::BgpResult::new(
            crate::bgp::TargetKind::Host,
            "example.invalid",
        ));
        failed.apply_bgp(&crate::bgp::BgpEvent::Failed(
            "Couldn't find \u{201C}example.invalid\u{201D}.".to_string(),
        ));
        let report = Report::of(&failed);
        assert_eq!(report.outcome, "failed");
        assert_eq!(
            report.sentence,
            "Couldn't find \u{201C}example.invalid\u{201D}."
        );
    }

    #[test]
    fn a_dns_lookup_reports_records_and_the_verdict() {
        use crate::dns::result::tests::agreeing;
        let mut run = Run::new(Tool::Dns, "example.com", FamilyChoice::Auto, 0, 0);
        run.dns = Some(agreeing());
        run.outcome = Some(Outcome::Reached);
        let report = Report::of(&run);
        assert_eq!(report.tool, "DNS");
        assert_eq!(
            report.summary,
            "Resolved \u{00B7} 9 ms \u{00B7} all agree \u{00B7} signed"
        );
        let names: Vec<&str> = report
            .measured
            .iter()
            .map(|measurement| measurement.name.as_str())
            .collect();
        assert_eq!(names, ["addresses", "sources", "fastest"]);
        assert_eq!(report.measured[0].value, 2.0);
        assert!(report.hops.is_none());
        let records = report.records.as_ref().unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(
            (
                records[0].type_name.as_str(),
                records[0].value.as_str(),
                records[0].ttl
            ),
            ("A", "192.0.2.10", Some(300))
        );
        assert_eq!(
            report.interpretation,
            vec!["All sources agree. Resolves to 2 addresses, zone served by example-dns.net. DNSSEC records are present from the root down."]
        );
        assert_eq!(
            report.sentence,
            "All sources agree. Resolves to 2 addresses, zone served by example-dns.net."
        );
        assert!(report.json().contains("\"type\": \"A\""));

        // A difference is told without the pointer to a tab the report doesn't have.
        let mut differing = agreeing();
        differing.sources[0].records[0].value = "203.0.113.5".into();
        run.dns = Some(differing);
        let report = Report::of(&run);
        assert!(report.interpretation[0].ends_with("1 source answers differently."));
        // The report says which source it is and what it answered.
        let sources = report.sources.as_ref().unwrap();
        let differing: Vec<&ReportSource> = sources
            .iter()
            .filter(|source| source.status == "differs")
            .collect();
        assert_eq!(differing.len(), 1);
        assert!(differing[0].addresses.contains(&"203.0.113.5".to_string()));
        assert!(sources.iter().any(|source| source.status == "match"));
        assert!(report.json().contains("\"kind\": \"device\""));
    }

    /// A TCP trace of three hops whose last hop is the destination or stays silent.
    fn tcp_run(state: crate::tcp::PortState) -> Run {
        use crate::tcp::{PortState, TcpProbe};
        use std::time::Duration;
        let mut run = Run::new(Tool::Traceroute, "example.com", FamilyChoice::Auto, 1, 30);
        run.tcp = Some(TcpProbe::new(443));
        run.destination = Some("192.0.2.10".parse().unwrap());
        run.apply(&Event::Port {
            state,
            connect: (state != PortState::Filtered).then(|| Duration::from_micros(11_300)),
            mss: (state == PortState::Open).then_some(1460),
        });
        for ttl in 1..=2u8 {
            run.apply(&Event::Sent { ttl, round: 0 });
            run.apply(&Event::Reply {
                ttl,
                round: 0,
                from: format!("198.51.100.{ttl}").parse().unwrap(),
                rtt: Duration::from_millis(u64::from(ttl) * 4),
                kind: ReplyKind::Intermediate,
            });
        }
        run.apply(&Event::Sent { ttl: 3, round: 0 });
        if state == PortState::Filtered {
            run.apply(&Event::Lost { ttl: 3, round: 0 });
            run.apply(&Event::Finished(Ending::Exhausted));
        } else {
            run.apply(&Event::Reply {
                ttl: 3,
                round: 0,
                from: "192.0.2.10".parse().unwrap(),
                rtt: Duration::from_millis(12),
                kind: ReplyKind::Destination,
            });
            run.apply(&Event::Finished(Ending::Reached(3)));
        }
        run
    }

    #[test]
    fn a_tcp_trace_reports_what_the_port_did() {
        use crate::tcp::PortState;
        let report = Report::of(&tcp_run(PortState::Open));
        assert_eq!(report.outcome, "reached");
        assert_eq!(
            report.summary,
            "3 hops \u{00B7} 12.0 ms \u{00B7} port 443 open"
        );
        assert_eq!(
            report.sentence,
            "Reached 192.0.2.10 on TCP port 443 in 3 hops, 12.0 ms round trip. The port is open."
        );
        let probe = report.probe.as_ref().unwrap();
        assert_eq!((probe.protocol.as_str(), probe.port), ("tcp", 443));
        assert_eq!(probe.state.as_deref(), Some("open"));
        let figure = |name: &str| {
            report
                .measured
                .iter()
                .find(|measurement| measurement.name == name)
                .map(|measurement| measurement.value)
        };
        assert_eq!(figure("connect"), Some(11.3));
        assert_eq!(figure("mss"), Some(1460.0));
        assert!(report.interpretation.is_empty());

        // A reset is an answer: the host is there and nothing listens.
        let closed = Report::of(&tcp_run(PortState::Closed));
        assert_eq!(closed.outcome, "reached");
        assert!(closed.summary.ends_with("port 443 closed"));
        assert_eq!(figure_of(&closed, "mss"), None);
    }

    fn figure_of(report: &Report, name: &str) -> Option<f64> {
        report
            .measured
            .iter()
            .find(|measurement| measurement.name == name)
            .map(|measurement| measurement.value)
    }

    #[test]
    fn a_filtered_port_names_the_last_router_as_an_inference() {
        use crate::tcp::PortState;
        let report = Report::of(&tcp_run(PortState::Filtered));
        assert_eq!(report.outcome, "exhausted");
        assert_eq!(
            report.summary,
            "Port 443 filtered \u{00B7} no reply after 2 hops"
        );
        assert_eq!(
            report.sentence,
            "No reply from 192.0.2.10 on TCP port 443 after 2 hops."
        );
        assert_eq!(
            report.probe.as_ref().unwrap().state.as_deref(),
            Some("filtered")
        );
        assert_eq!(figure_of(&report, "connect"), None);
        // The place is the last router that answered, and the filter is worded as likely.
        assert_eq!(
            report.interpretation,
            vec!["Nothing answered on TCP port 443. The last router to answer is hop 2, 198.51.100.2, so a filter likely drops the SYN after it or at the destination."]
        );
        // An ICMP run has no probe and keeps its own sentence about a silent target.
        assert!(Report::of(&reached_run()).probe.is_none());
    }

    #[test]
    fn a_run_stored_before_tcp_probes_still_loads() {
        let mut run = reached_run();
        run.tcp = Some(crate::tcp::TcpProbe::new(443));
        let mut value = serde_json::to_value(&run).unwrap();
        value.as_object_mut().unwrap().remove("tcp");
        let old: Run = serde_json::from_value(value).unwrap();
        assert!(old.tcp.is_none());
    }

    #[test]
    fn a_failure_is_its_own_sentence() {
        let mut run = Run::new(Tool::Traceroute, "nope.invalid", FamilyChoice::Auto, 3, 30);
        run.apply(&Event::Failed(
            "Couldn't find \u{201C}nope.invalid\u{201D}.".into(),
        ));
        let report = Report::of(&run);
        assert_eq!(report.outcome, "failed");
        assert_eq!(
            report.error.as_deref(),
            Some("Couldn't find \u{201C}nope.invalid\u{201D}.")
        );
        assert_eq!(report.sentence, report.error.clone().unwrap());
    }
}
