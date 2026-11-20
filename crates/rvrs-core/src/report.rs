//! A run as data: the report `--json` prints and, once accounts exist, the body the API stores.
//! Field for field the `RunReport` of the iOS app (`rvrs/Models/RunReport.swift`): what was
//! measured (`measured`, `hops`, `outcome`) stays apart from what it likely means
//! (`interpretation`), so a person or an agent can tell a fact from a guess.

use serde::Serialize;

use crate::format;
use crate::model::{Grade, Outcome, Run, Tool, Verdict};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measurement {
    pub name: String,
    pub value: f64,
    pub unit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportHop {
    pub ttl: u8,
    pub address: Option<String>,
    pub hostname: Option<String>,
    pub asn: Option<String>,
    pub network: Option<String>,
    pub loss_percent: f64,
    pub avg_milliseconds: Option<f64>,
}

/// One DNS record of the reference answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReportRecord {
    #[serde(rename = "type")]
    pub type_name: String,
    pub name: String,
    pub ttl: u32,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub tool: String,
    pub target: String,
    pub address: Option<String>,
    pub family: Option<String>,
    pub started_at: String,
    /// reached, exhausted, stopped or failed.
    pub outcome: String,
    /// The run in one line of facts: "6 hops · 12.4 ms".
    pub summary: String,
    pub error: Option<String>,
    pub measured: Vec<Measurement>,
    pub hops: Option<Vec<ReportHop>>,
    /// DNS: the records of the reference answer.
    pub records: Option<Vec<ReportRecord>>,
    /// Inferences only, each a sentence that says it is one.
    pub interpretation: Vec<String>,
    /// The result as one plain sentence.
    pub sentence: String,
}

pub const SILENT_TARGET: &str =
    "The target sent no reply. It may drop ICMP, so a silent target isn't necessarily down.";

fn round(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

impl Report {
    pub fn of(run: &Run) -> Self {
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
        } else if outcome == Outcome::Exhausted && run.unreachable().is_none() {
            interpretation.push(SILENT_TARGET.to_string());
        }
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
                    loss_percent: round(stats.loss_fraction() * 100.0),
                    avg_milliseconds: stats.mean.map(|mean| round(format::ms_value(mean))),
                }
            })
            .collect();
        Self {
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
                                ttl: record.ttl,
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
            interpretation,
            sentence,
        }
    }

    pub fn json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
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
        assert_eq!(hops[1].loss_percent, 100.0);
        assert_eq!(hops[2].asn.as_deref(), Some("AS13335"));
        assert_eq!(hops[2].avg_milliseconds, Some(12.5));
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
        ] {
            assert!(json.contains(key), "{key} missing");
        }
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
        assert_eq!(hops[1].loss_percent, 33.3);
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
            ("A", "192.0.2.10", 300)
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
