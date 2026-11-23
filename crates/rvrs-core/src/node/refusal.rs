//! Why a node won't make a run, and the report that says so.
//!
//! A refusal travels the path every result travels: a report with the outcome `refused` and the
//! reason in `error`, sent through `POST /v1/runs` with the run's claim. The person who asked
//! reads why within a second or two.

use crate::model::Tool;
use crate::report::{Report, ReportOrigin, SCHEMA};

use super::address::Space;
use super::request::Request;

/// What kind of rule a refusal comes from. The word is all a log line says about a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The build has no such tool or build feature.
    Tool,
    /// An option the node doesn't know, doesn't take for this tool or can't read.
    Option,
    /// More packets, a longer run or a faster interval than the node allows itself.
    Limit,
    /// A target the node doesn't send packets to.
    Target,
    /// More runs at once or in a minute than the node allows itself.
    Rate,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Tool => "tool",
            Kind::Option => "option",
            Kind::Limit => "limit",
            Kind::Target => "target",
            Kind::Rate => "rate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The build has no such tool, or this version never heard of it. `tool` is the word as a
    /// sentence takes it: "iperf3", "TCP probe".
    Tool {
        tool: String,
    },
    /// An option this version doesn't know. Running without it would measure something else.
    UnknownOption {
        option: String,
    },
    /// An option another tool takes.
    OptionOfAnotherTool {
        tool: &'static str,
        option: String,
    },
    /// An option with a value outside its rule. `rule` completes "must be".
    Option {
        option: String,
        rule: String,
    },
    Packets {
        packets: u64,
        limit: u32,
    },
    Seconds {
        seconds: u64,
        limit: u32,
    },
    Interval {
        milliseconds: u64,
        floor: u64,
    },
    /// The target is no name and no address.
    Target,
    /// An address the node sends nothing to. `subject` starts the sentence: the address, or
    /// "The resolver 10.0.0.53".
    Address {
        subject: String,
        space: Space,
    },
    /// A name of the node's own network, refused before the node asks its resolver.
    LocalName {
        name: String,
    },
    /// A name that resolved to an address the node sends nothing to. The address is left out.
    Resolved {
        name: String,
        space: Space,
    },
    AtOnce {
        limit: u32,
    },
    PerMinute {
        limit: u32,
    },
}

/// Who refuses: the node as a report names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identity<'a> {
    /// The node's name as the API last said it.
    pub name: &'a str,
    /// The node's id, which never changes. None before the node learned it.
    pub id: Option<&'a str>,
    /// The node's rvrs version.
    pub version: &'a str,
}

impl Refusal {
    pub fn kind(&self) -> Kind {
        match self {
            Refusal::Tool { .. } => Kind::Tool,
            Refusal::UnknownOption { .. }
            | Refusal::OptionOfAnotherTool { .. }
            | Refusal::Option { .. } => Kind::Option,
            Refusal::Packets { .. } | Refusal::Seconds { .. } | Refusal::Interval { .. } => {
                Kind::Limit
            }
            Refusal::Target
            | Refusal::Address { .. }
            | Refusal::LocalName { .. }
            | Refusal::Resolved { .. } => Kind::Target,
            Refusal::AtOnce { .. } | Refusal::PerMinute { .. } => Kind::Rate,
        }
    }

    /// The refusal in a few words, for the report's summary after "Refused · ".
    pub fn brief(&self, node: &str) -> String {
        match self {
            Refusal::Tool { tool } => format!("{node} has no {tool}"),
            Refusal::UnknownOption { option } => {
                format!("{node} doesn't know the option {option}")
            }
            Refusal::OptionOfAnotherTool { tool, option } => {
                format!("{option} is not an option of {tool}")
            }
            Refusal::Option { option, rule } => format!("{option} must be {rule}"),
            Refusal::Packets { packets, limit } => format!(
                "{} packets is over {node}'s limit of {}",
                grouped(*packets),
                grouped(u64::from(*limit))
            ),
            Refusal::Seconds { seconds, limit } => format!(
                "{} s is over {node}'s limit of {} s",
                grouped(*seconds),
                grouped(u64::from(*limit))
            ),
            Refusal::Interval {
                milliseconds,
                floor,
            } => format!("an interval of {milliseconds} ms is under {node}'s {floor} ms"),
            Refusal::Target => "the target is not a name or an address".to_string(),
            Refusal::Address { space, .. } | Refusal::Resolved { space, .. } => {
                format!("{node} sends nothing to {}", space.many())
            }
            Refusal::LocalName { .. } => {
                format!("{node} doesn't look up names of its own network")
            }
            Refusal::AtOnce { limit } => {
                format!("{node} is at its limit of {limit} runs at once")
            }
            Refusal::PerMinute { limit } => {
                format!("{node} is at its limit of {limit} runs a minute")
            }
        }
    }

    /// The reason as one sentence, for the report's `error`.
    pub fn error(&self, node: &Identity) -> String {
        let name = node.name;
        match self {
            Refusal::Tool { tool } => format!(
                "{name} runs rvrs {}, and its build has no {tool}.",
                node.version
            ),
            Refusal::UnknownOption { option } => format!(
                "{name} runs rvrs {}, which doesn't know the option {option}.",
                node.version
            ),
            Refusal::OptionOfAnotherTool { tool, option } => {
                format!("{option} is not an option of {tool}.")
            }
            Refusal::Option { option, rule } => format!("{option} must be {rule}."),
            Refusal::Packets { packets, limit } => format!(
                "The run would send up to {} packets, and {name} sends {} a run at most.",
                grouped(*packets),
                grouped(u64::from(*limit))
            ),
            Refusal::Seconds { seconds, limit } => format!(
                "The run could take {} s, and {name} gives a run {} s at most.",
                grouped(*seconds),
                grouped(u64::from(*limit))
            ),
            Refusal::Interval {
                milliseconds,
                floor,
            } => format!(
                "{name} sends packets {floor} ms apart or slower, and the run asked for {milliseconds} ms."
            ),
            Refusal::Target => "target must be a name or an address.".to_string(),
            Refusal::Address { subject, space } if space.can_be_allowed() => format!(
                "{subject} is {}, and {name}'s owner allowed no range that holds it.",
                space.one()
            ),
            Refusal::Address { subject, space } => {
                format!("{subject} is {}, where {name} sends nothing.", space.one())
            }
            Refusal::LocalName { name: local } => format!(
                "{local} is a name of {name}'s own network, and its owner allowed no range there."
            ),
            Refusal::Resolved { name: asked, space } if space.can_be_allowed() => format!(
                "{asked} resolves to {} on {name}, and its owner allowed no range that holds it.",
                space.one()
            ),
            Refusal::Resolved { name: asked, space } => format!(
                "{asked} resolves to {} on {name}, where it sends nothing.",
                space.one()
            ),
            Refusal::AtOnce { limit } => format!("{name} makes {limit} runs at once at most."),
            Refusal::PerMinute { limit } => {
                format!("{name} starts {limit} runs a minute at most.")
            }
        }
    }

    /// The refusal as one plain sentence pair, for the report's `sentence`.
    pub fn sentence(&self, node: &Identity) -> String {
        let reason = match self {
            Refusal::Tool { tool } => format!("Its build has no {tool}."),
            Refusal::UnknownOption { option } => {
                format!("Its version doesn't know the option {option}.")
            }
            other => other.error(node),
        };
        format!("{} refused the run. {reason}", node.name)
    }

    /// The report a node sends for a run it won't make: the outcome `refused`, the reason in
    /// `error`, and nothing measured. `id` is the node's own id for the report and
    /// `started_at` the moment the node refused, in seconds since 1970.
    pub fn report(
        &self,
        request: &Request,
        node: &Identity,
        id: impl Into<String>,
        started_at: u64,
    ) -> Report {
        Report {
            schema: SCHEMA,
            id: id.into(),
            tool: tool_title(&request.tool),
            target: request.target.clone(),
            address: None,
            family: None,
            started_at: crate::format::iso8601(started_at),
            origin: ReportOrigin {
                kind: "node".into(),
                node: Some(node.name.to_string()),
                node_id: node.id.map(str::to_string),
                install: None,
                source_address: None,
                public_address: None,
            },
            outcome: "refused".into(),
            summary: format!("Refused \u{00B7} {}", self.brief(node.name)),
            error: Some(self.error(node)),
            measured: Vec::new(),
            hops: None,
            records: None,
            sources: None,
            probe: None,
            mtu: None,
            samples: None,
            interpretation: Vec::new(),
            sentence: self.sentence(node),
        }
    }

    /// What a log line says about a refusal: the run, the attempt and the kind of rule. Never the
    /// target, an option's value or an address.
    pub fn log_line(&self, request: &Request) -> String {
        format!(
            "refused run {} attempt {}: {}",
            request.run,
            request.attempt,
            self.kind().word()
        )
    }
}

/// The tool as a report names it (`schema/run-report.schema.json`): Traceroute, MTR, Ping, DNS,
/// BGP or iPerf3. A word this version doesn't know is kept as it came.
fn tool_title(word: &str) -> String {
    match super::request::tool_of(word) {
        Some(Tool::Iperf3) => "iPerf3",
        Some(tool) => tool.title(),
        None => word,
    }
    .to_string()
}

/// 1000 as "1,000".
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE: Identity = Identity {
        name: "fra-1",
        id: Some("9c2f1a7e-5b3d-4e8f-8a6c-1d2e3f4a5b6c"),
        version: "0.14.2",
    };

    fn request(tool: &str, target: &str) -> Request {
        Request {
            run: "3f9e8d7c-6b5a-4c3d-9e2f-1a0b9c8d7e6f".into(),
            attempt: 1,
            tool: tool.into(),
            target: target.into(),
            options: serde_json::Map::new(),
        }
    }

    #[test]
    fn numbers_are_grouped() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(86_400), "86,400");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn every_refusal_reads_as_sentences() {
        let cases: Vec<(Refusal, &str, &str)> = vec![
            (
                Refusal::Tool { tool: "iperf3".into() },
                "fra-1 has no iperf3",
                "fra-1 runs rvrs 0.14.2, and its build has no iperf3.",
            ),
            (
                Refusal::UnknownOption { option: "burst".into() },
                "fra-1 doesn't know the option burst",
                "fra-1 runs rvrs 0.14.2, which doesn't know the option burst.",
            ),
            (
                Refusal::OptionOfAnotherTool { tool: "ping", option: "maxHops".into() },
                "maxHops is not an option of ping",
                "maxHops is not an option of ping.",
            ),
            (
                Refusal::Option {
                    option: "rounds".into(),
                    rule: "a whole number from 1 to 10".into(),
                },
                "rounds must be a whole number from 1 to 10",
                "rounds must be a whole number from 1 to 10.",
            ),
            (
                Refusal::Packets { packets: 3000, limit: 1000 },
                "3,000 packets is over fra-1's limit of 1,000",
                "The run would send up to 3,000 packets, and fra-1 sends 1,000 a run at most.",
            ),
            (
                Refusal::Seconds { seconds: 602, limit: 300 },
                "602 s is over fra-1's limit of 300 s",
                "The run could take 602 s, and fra-1 gives a run 300 s at most.",
            ),
            (
                Refusal::Interval { milliseconds: 50, floor: 200 },
                "an interval of 50 ms is under fra-1's 200 ms",
                "fra-1 sends packets 200 ms apart or slower, and the run asked for 50 ms.",
            ),
            (
                Refusal::Target,
                "the target is not a name or an address",
                "target must be a name or an address.",
            ),
            (
                Refusal::Address { subject: "10.0.0.1".into(), space: Space::Private },
                "fra-1 sends nothing to private addresses",
                "10.0.0.1 is a private address, and fra-1's owner allowed no range that holds it.",
            ),
            (
                Refusal::Address { subject: "169.254.169.254".into(), space: Space::LinkLocal },
                "fra-1 sends nothing to link-local addresses",
                "169.254.169.254 is a link-local address, where fra-1 sends nothing.",
            ),
            (
                Refusal::LocalName { name: "nas.lan".into() },
                "fra-1 doesn't look up names of its own network",
                "nas.lan is a name of fra-1's own network, and its owner allowed no range there.",
            ),
            (
                Refusal::Resolved { name: "rebind.example.net".into(), space: Space::Private },
                "fra-1 sends nothing to private addresses",
                "rebind.example.net resolves to a private address on fra-1, and its owner allowed no range that holds it.",
            ),
            (
                Refusal::Resolved { name: "rebind.example.net".into(), space: Space::Loopback },
                "fra-1 sends nothing to loopback addresses",
                "rebind.example.net resolves to a loopback address on fra-1, where it sends nothing.",
            ),
            (
                Refusal::AtOnce { limit: 2 },
                "fra-1 is at its limit of 2 runs at once",
                "fra-1 makes 2 runs at once at most.",
            ),
            (
                Refusal::PerMinute { limit: 12 },
                "fra-1 is at its limit of 12 runs a minute",
                "fra-1 starts 12 runs a minute at most.",
            ),
        ];
        for (refusal, brief, error) in cases {
            assert_eq!(refusal.brief(NODE.name), brief);
            assert_eq!(refusal.error(&NODE), error);
            assert!(refusal
                .sentence(&NODE)
                .starts_with("fra-1 refused the run. "));
        }
    }

    #[test]
    fn a_refusal_is_a_report_with_nothing_measured() {
        let refusal = Refusal::Address {
            subject: "192.168.1.1".into(),
            space: Space::Private,
        };
        let report = refusal.report(
            &request("ping", "192.168.1.1"),
            &NODE,
            "k3v9q2",
            1_794_735_000,
        );
        assert_eq!(report.schema, SCHEMA);
        assert_eq!(report.id, "k3v9q2");
        assert_eq!(report.tool, "Ping");
        assert_eq!(report.target, "192.168.1.1");
        assert_eq!(report.started_at, "2026-11-15T09:30:00Z");
        assert_eq!(report.outcome, "refused");
        assert_eq!(report.origin.kind, "node");
        assert_eq!(report.origin.node.as_deref(), Some("fra-1"));
        assert_eq!(report.origin.node_id.as_deref(), NODE.id);
        // A refused run sent nothing, so it has no address of its own to say.
        assert_eq!(report.origin.source_address, None);
        assert_eq!(report.origin.public_address, None);
        assert_eq!(report.address, None);
        assert!(report.measured.is_empty());
        assert!(report.hops.is_none());
        assert_eq!(
            report.summary,
            "Refused \u{00B7} fra-1 sends nothing to private addresses"
        );
        assert_eq!(
            report.sentence,
            "fra-1 refused the run. 192.168.1.1 is a private address, and fra-1's owner allowed no range that holds it."
        );
    }

    #[test]
    fn a_tool_this_version_never_heard_of_keeps_its_word() {
        let refusal = Refusal::Tool {
            tool: "nmap".into(),
        };
        let report = refusal.report(&request("nmap", "example.com"), &NODE, "a", 0);
        assert_eq!(report.tool, "nmap");
        assert_eq!(
            report.error.as_deref(),
            Some("fra-1 runs rvrs 0.14.2, and its build has no nmap.")
        );
    }

    #[test]
    fn a_log_line_names_the_run_and_the_kind_and_nothing_of_the_request() {
        let mut asked = request("ping", "secret-host.example.com");
        asked.options.insert("resolver".into(), "10.9.8.7".into());
        let refusal = Refusal::Address {
            subject: "The resolver 10.9.8.7".into(),
            space: Space::Private,
        };
        let line = refusal.log_line(&asked);
        assert_eq!(
            line,
            "refused run 3f9e8d7c-6b5a-4c3d-9e2f-1a0b9c8d7e6f attempt 1: target"
        );
        assert!(!line.contains("secret-host"));
        assert!(!line.contains("10.9.8.7"));
    }
}
