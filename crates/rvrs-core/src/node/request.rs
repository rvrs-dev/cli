//! A run a node was asked for, checked before anything is sent.
//!
//! A node never runs a command it was sent. A request names a tool and a fixed list of options
//! (`schema/node-protocol/README.md`, What a run is). The check turns it into the options of this
//! build's own engines, or into a refusal: a tool or an option it doesn't know, a run over the
//! node's limits, a target it doesn't send packets to.

use std::net::IpAddr;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::engine::{MtrOptions, Options, PingOptions};
use crate::model::Tool;
use crate::FamilyChoice;

use super::address::{is_local_name, Allowed};
use super::refusal::Refusal;
use super::Limits;

/// A run as a claim's answer hands it to the node.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Request {
    /// The run's id.
    pub run: String,
    /// Which claim of the run this is, from 1.
    pub attempt: u32,
    /// The word a build names the tool by in its build features.
    pub tool: String,
    /// What the person typed.
    pub target: String,
    /// What the person set. An option left out takes the tool's default.
    #[serde(default)]
    pub options: Map<String, Value>,
}

/// The tools a request can name, by the word the protocol uses.
const TOOLS: [(&str, Tool); 6] = [
    ("traceroute", Tool::Traceroute),
    ("ping", Tool::Ping),
    ("mtr", Tool::Mtr),
    ("dns", Tool::Dns),
    ("bgp", Tool::Bgp),
    ("iperf3", Tool::Iperf3),
];

pub(super) fn tool_of(word: &str) -> Option<Tool> {
    TOOLS
        .iter()
        .find(|(name, _)| *name == word)
        .map(|(_, tool)| *tool)
}

/// The tool as a sentence spells it: traceroute, ping, MTR, DNS, BGP, iperf3.
fn prose(tool: Tool) -> &'static str {
    match tool {
        Tool::Traceroute => "traceroute",
        Tool::Ping => "ping",
        Tool::Mtr => "MTR",
        Tool::Dns => "DNS",
        Tool::Bgp => "BGP",
        Tool::Iperf3 => "iperf3",
    }
}

/// Every option of version 1 with the tools that take it. A test holds this list against the
/// table on the protocol's page.
pub const OPTIONS: [(&str, &[&str]); 12] = [
    ("family", &["traceroute", "ping", "mtr"]),
    ("rounds", &["traceroute", "mtr"]),
    ("packets", &["ping"]),
    ("intervalMilliseconds", &["ping", "mtr"]),
    ("waitMilliseconds", &["traceroute", "ping", "mtr"]),
    ("maxHops", &["traceroute", "mtr"]),
    ("firstHop", &["traceroute", "mtr"]),
    ("probe", &["traceroute"]),
    ("mtu", &["traceroute"]),
    ("lookups", &["traceroute", "ping", "mtr"]),
    ("type", &["dns"]),
    ("resolver", &["dns"]),
];

/// Ping and MTR go on until stopped on a device. A run on a node always ends on its own, so it
/// sends this many packets or rounds when the request names no number.
pub const DEFAULT_COUNT: u32 = 10;

/// The closest together a node sends the packets of a ping or the rounds of an MTR. It is the
/// interval iputils lets an unprivileged user ask for.
pub const INTERVAL_FLOOR_MILLISECONDS: u64 = 200;

/// The longest wait for one reply a request may ask for.
const WAIT_CEILING_MILLISECONDS: u64 = 60_000;

/// The most rounds a traceroute sends, as the engine has it.
const TRACEROUTE_ROUNDS: u64 = 10;

/// The most probes the path MTU check sends: a search between the smallest packet every path
/// carries and this device's own MTU, after the sizes a device's MTU usually is.
const MTU_PROBES: u64 = 24;

/// What the node runs, in the options of this build's engines.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Traceroute {
        family: FamilyChoice,
        options: Options,
        /// Probe with TCP toward this port.
        tcp_port: Option<u16>,
        /// Find the path MTU once the trace is done.
        mtu: bool,
        lookups: bool,
    },
    Ping {
        family: FamilyChoice,
        options: PingOptions,
        lookups: bool,
    },
    Mtr {
        family: FamilyChoice,
        options: MtrOptions,
        lookups: bool,
    },
    Dns {
        /// The record type as the person wrote it. None is the tool's default.
        record_type: Option<String>,
        /// A resolver of the person's own, asked beside the others.
        resolver: Option<IpAddr>,
    },
    Bgp,
}

/// A request that passed the check.
#[derive(Debug, Clone, PartialEq)]
pub struct Admitted {
    pub tool: Tool,
    /// What the person typed, as the report will name it.
    pub target: String,
    pub plan: Plan,
    /// The most packets the run sends toward its target and the routers on the way.
    pub packets: u64,
    /// The longest the run takes by its own options, in seconds. 0 where the options don't say.
    pub seconds: u64,
    /// When the node ends the run, whatever it has measured by then.
    pub deadline: Duration,
}

impl Admitted {
    /// Whether the run sends packets to a destination that has to be resolved and checked first.
    pub fn probes(&self) -> bool {
        matches!(
            self.plan,
            Plan::Traceroute { .. } | Plan::Ping { .. } | Plan::Mtr { .. }
        )
    }

    /// The family a name is resolved in.
    pub fn family(&self) -> FamilyChoice {
        match &self.plan {
            Plan::Traceroute { family, .. }
            | Plan::Ping { family, .. }
            | Plan::Mtr { family, .. } => *family,
            Plan::Dns { .. } | Plan::Bgp => FamilyChoice::Auto,
        }
    }
}

/// Checks a request against what the build has, what the node allows itself and where it sends
/// packets. `features` are the build features this build has, by name.
pub fn check(
    request: &Request,
    features: &[&str],
    limits: &Limits,
    allowed: &Allowed,
) -> Result<Admitted, Refusal> {
    let has = |feature: &str| features.contains(&feature);
    let Some((word, tool)) = TOOLS
        .iter()
        .find(|(name, _)| *name == request.tool)
        .copied()
    else {
        return Err(Refusal::Tool {
            tool: request.tool.clone(),
        });
    };
    if !has(word) {
        return Err(Refusal::Tool {
            tool: prose(tool).into(),
        });
    }

    let asked = Asked {
        word,
        prose: prose(tool),
        options: &request.options,
    };
    asked.known()?;

    let lookups = asked.flag("lookups")?.unwrap_or(true);
    let family = asked.family()?;
    let (plan, packets, milliseconds) = match tool {
        Tool::Traceroute => {
            let defaults = Options::default();
            let rounds = asked
                .whole("rounds", 1, TRACEROUTE_ROUNDS)?
                .unwrap_or(u64::from(defaults.queries));
            let (first, last) = asked.hops(defaults.first_ttl, defaults.max_ttl)?;
            let wait = asked.wait(defaults.wait)?;
            let tcp_port = asked.probe()?;
            if tcp_port.is_some() && !has("tcp") {
                return Err(Refusal::Tool {
                    tool: "TCP probe".into(),
                });
            }
            let mtu = asked.flag("mtu")?.unwrap_or(false);
            if mtu && !has("mtu") {
                return Err(Refusal::Tool {
                    tool: "path MTU check".into(),
                });
            }
            let hops = u64::from(last - first) + 1;
            let extra = if mtu { MTU_PROBES } else { 0 };
            let packets = rounds * hops + extra;
            let spacing = defaults.spacing.as_millis() as u64;
            let milliseconds = (rounds + extra) * wait.as_millis() as u64 + rounds * hops * spacing;
            let options = Options {
                first_ttl: first,
                max_ttl: last,
                queries: rounds as u8,
                wait,
                ..defaults
            };
            (
                Plan::Traceroute {
                    family,
                    options,
                    tcp_port,
                    mtu,
                    lookups,
                },
                packets,
                milliseconds,
            )
        }
        Tool::Ping => {
            let defaults = PingOptions::default();
            let packets = asked
                .whole("packets", 1, u64::from(u32::MAX))?
                .unwrap_or(u64::from(DEFAULT_COUNT));
            let interval = asked.interval(defaults.interval)?;
            let wait = asked.wait(defaults.wait)?;
            let milliseconds = (packets - 1)
                .saturating_mul(interval.as_millis() as u64)
                .saturating_add(wait.as_millis() as u64);
            let options = PingOptions {
                count: Some(packets as u32),
                interval,
                wait,
                deadline: Some(Duration::from_secs(u64::from(limits.seconds_per_run))),
                ..defaults
            };
            (
                Plan::Ping {
                    family,
                    options,
                    lookups,
                },
                packets,
                milliseconds,
            )
        }
        Tool::Mtr => {
            let defaults = MtrOptions::default();
            let rounds = asked
                .whole("rounds", 1, u64::from(u32::MAX))?
                .unwrap_or(u64::from(DEFAULT_COUNT));
            let (first, last) = asked.hops(defaults.first_ttl, defaults.max_ttl)?;
            let interval = asked.interval(defaults.interval)?;
            let wait = asked.wait(defaults.wait)?;
            let hops = u64::from(last - first) + 1;
            let milliseconds = rounds
                .saturating_mul(interval.as_millis() as u64)
                .saturating_add(wait.as_millis() as u64);
            let options = MtrOptions {
                first_ttl: first,
                max_ttl: last,
                count: Some(rounds as u32),
                interval,
                wait,
                ..defaults
            };
            (
                Plan::Mtr {
                    family,
                    options,
                    lookups,
                },
                rounds.saturating_mul(hops),
                milliseconds,
            )
        }
        Tool::Dns => {
            let record_type = asked.record_type()?;
            let resolver = asked.resolver(allowed)?;
            (
                Plan::Dns {
                    record_type,
                    resolver,
                },
                0,
                0,
            )
        }
        Tool::Bgp => (Plan::Bgp, 0, 0),
        // No build runs iperf3 yet, so no build has the feature and the check ended above.
        Tool::Iperf3 => {
            return Err(Refusal::Tool {
                tool: prose(tool).into(),
            });
        }
    };

    if packets > u64::from(limits.packets_per_run) {
        return Err(Refusal::Packets {
            packets,
            limit: limits.packets_per_run,
        });
    }
    let seconds = milliseconds.div_ceil(1000);
    if seconds > u64::from(limits.seconds_per_run) {
        return Err(Refusal::Seconds {
            seconds,
            limit: limits.seconds_per_run,
        });
    }

    target(tool, &request.target, allowed)?;

    Ok(Admitted {
        tool,
        target: request.target.clone(),
        plan,
        packets,
        seconds,
        deadline: Duration::from_secs(u64::from(limits.seconds_per_run)),
    })
}

/// The check on what the person typed, before any name is resolved. An address is judged now. A
/// name of the node's own network is refused unless the owner allowed a range, and every other
/// name is judged again by the address it resolves to ([`resolved`]).
pub fn target(tool: Tool, text: &str, allowed: &Allowed) -> Result<(), Refusal> {
    let shaped = !text.is_empty()
        && text.len() <= 255
        && !text.starts_with('-')
        && !text
            .chars()
            .any(|letter| letter.is_whitespace() || letter.is_control());
    if !shaped {
        return Err(Refusal::Target);
    }
    // A BGP lookup asks RIPEstat about an address, a prefix or a network. Nothing goes to the
    // target and nothing is asked of the node's own resolver.
    if tool == Tool::Bgp {
        return Ok(());
    }
    if let Ok(address) = text.parse::<IpAddr>() {
        return allowed.permit(address).map_err(|space| Refusal::Address {
            subject: text.to_string(),
            space,
        });
    }
    let named = text
        .chars()
        .all(|letter| letter.is_alphanumeric() || matches!(letter, '.' | '-' | '_'));
    if !named {
        return Err(Refusal::Target);
    }
    if allowed.is_empty() && is_local_name(text) {
        return Err(Refusal::LocalName {
            name: text.to_string(),
        });
    }
    Ok(())
}

/// The check after a name resolved: the address is the one the run probes, so it has to be one
/// the node sends packets to. A name that was public a moment ago and is private now is refused
/// here.
pub fn resolved(name: &str, address: IpAddr, allowed: &Allowed) -> Result<IpAddr, Refusal> {
    match allowed.permit(address) {
        Ok(()) => Ok(address),
        // Asked by its address, the sentence can name it. Asked by a name, it leaves out what
        // the name resolved to.
        Err(space) if name.parse::<IpAddr>().is_ok() => Err(Refusal::Address {
            subject: name.to_string(),
            space,
        }),
        Err(space) => Err(Refusal::Resolved {
            name: name.to_string(),
            space,
        }),
    }
}

/// The options of one request, read by name.
struct Asked<'a> {
    /// The tool by the protocol's word.
    word: &'static str,
    /// The tool as a sentence spells it.
    prose: &'static str,
    options: &'a Map<String, Value>,
}

impl Asked<'_> {
    /// Every option is one this version knows and this tool takes.
    fn known(&self) -> Result<(), Refusal> {
        for option in self.options.keys() {
            match OPTIONS.iter().find(|(name, _)| name == option) {
                None => {
                    return Err(Refusal::UnknownOption {
                        option: option.clone(),
                    })
                }
                Some((_, tools)) if !tools.contains(&self.word) => {
                    return Err(Refusal::OptionOfAnotherTool {
                        tool: self.prose,
                        option: option.clone(),
                    })
                }
                Some(_) => {}
            }
        }
        Ok(())
    }

    fn wrong(&self, option: &str, rule: impl Into<String>) -> Refusal {
        Refusal::Option {
            option: option.to_string(),
            rule: rule.into(),
        }
    }

    fn whole(&self, option: &str, least: u64, most: u64) -> Result<Option<u64>, Refusal> {
        let Some(value) = self.options.get(option) else {
            return Ok(None);
        };
        let rule = || {
            if most == u64::from(u32::MAX) {
                format!("a whole number of {least} or more")
            } else {
                format!("a whole number from {least} to {most}")
            }
        };
        value
            .as_u64()
            .filter(|number| (least..=most).contains(number))
            .map(Some)
            .ok_or_else(|| self.wrong(option, rule()))
    }

    fn flag(&self, option: &str) -> Result<Option<bool>, Refusal> {
        match self.options.get(option) {
            None => Ok(None),
            Some(Value::Bool(flag)) => Ok(Some(*flag)),
            Some(_) => Err(self.wrong(option, "true or false")),
        }
    }

    fn family(&self) -> Result<FamilyChoice, Refusal> {
        match self.options.get("family") {
            None => Ok(FamilyChoice::Auto),
            Some(Value::String(family)) if family == "IPv4" => Ok(FamilyChoice::V4),
            Some(Value::String(family)) if family == "IPv6" => Ok(FamilyChoice::V6),
            Some(_) => Err(self.wrong("family", "IPv4 or IPv6")),
        }
    }

    /// The first and the last hop to probe.
    fn hops(&self, first: u8, last: u8) -> Result<(u8, u8), Refusal> {
        let last = self.whole("maxHops", 1, 255)?.map_or(last, |hop| hop as u8);
        let first = self
            .whole("firstHop", 1, 255)?
            .map_or(first, |hop| hop as u8);
        if first > last {
            return Err(self.wrong("firstHop", format!("{last} or less, the last hop to probe")));
        }
        Ok((first, last))
    }

    fn wait(&self, default: Duration) -> Result<Duration, Refusal> {
        Ok(self
            .whole("waitMilliseconds", 1, WAIT_CEILING_MILLISECONDS)?
            .map_or(default, Duration::from_millis))
    }

    fn interval(&self, default: Duration) -> Result<Duration, Refusal> {
        let Some(milliseconds) = self.whole("intervalMilliseconds", 1, u64::from(u32::MAX))? else {
            return Ok(default);
        };
        if milliseconds < INTERVAL_FLOOR_MILLISECONDS {
            return Err(Refusal::Interval {
                milliseconds,
                floor: INTERVAL_FLOOR_MILLISECONDS,
            });
        }
        Ok(Duration::from_millis(milliseconds))
    }

    /// `{ "protocol": "tcp", "port": 443 }`, the one probe beside ICMP echo that version 1 has.
    fn probe(&self) -> Result<Option<u16>, Refusal> {
        let Some(value) = self.options.get("probe") else {
            return Ok(None);
        };
        let rule = "{ \"protocol\": \"tcp\", \"port\": 1 to 65535 }";
        let probe = value.as_object().ok_or_else(|| self.wrong("probe", rule))?;
        let tcp = probe.get("protocol").and_then(Value::as_str) == Some("tcp");
        let port = probe
            .get("port")
            .and_then(Value::as_u64)
            .filter(|port| (1..=65_535).contains(port));
        let only_these = probe.keys().all(|key| key == "protocol" || key == "port");
        match port {
            Some(port) if tcp && only_these => Ok(Some(port as u16)),
            _ => Err(self.wrong("probe", rule)),
        }
    }

    fn record_type(&self) -> Result<Option<String>, Refusal> {
        match self.options.get("type") {
            None => Ok(None),
            Some(Value::String(name))
                if (1..=20).contains(&name.len())
                    && name.chars().all(|letter| letter.is_ascii_alphanumeric()) =>
            {
                Ok(Some(name.clone()))
            }
            Some(_) => Err(self.wrong("type", "a record type such as A or AAAA")),
        }
    }

    /// A resolver of the person's own is an address the node sends questions to, so it is held
    /// to the same rule as a target.
    fn resolver(&self, allowed: &Allowed) -> Result<Option<IpAddr>, Refusal> {
        let Some(value) = self.options.get("resolver") else {
            return Ok(None);
        };
        let address = value
            .as_str()
            .and_then(|text| text.parse::<IpAddr>().ok())
            .ok_or_else(|| self.wrong("resolver", "an address"))?;
        allowed
            .permit(address)
            .map(|()| Some(address))
            .map_err(|space| Refusal::Address {
                subject: format!("The resolver {address}"),
                space,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::address::Space;
    use crate::node::refusal::Kind;
    use serde_json::json;

    const FULL: [&str; 7] = ["traceroute", "ping", "mtr", "dns", "bgp", "tcp", "mtu"];

    fn request(tool: &str, target: &str, options: Value) -> Request {
        Request {
            run: "0e6d2f0a-8c0e-4f58-9a55-0b8f3f6f1c11".into(),
            attempt: 1,
            tool: tool.into(),
            target: target.into(),
            options: options.as_object().cloned().unwrap_or_default(),
        }
    }

    fn admit(tool: &str, target: &str, options: Value) -> Result<Admitted, Refusal> {
        check(
            &request(tool, target, options),
            &FULL,
            &Limits::default(),
            &Allowed::none(),
        )
    }

    #[test]
    fn a_request_becomes_the_engines_own_options() {
        let mtr = admit(
            "mtr",
            "example.com",
            json!({ "rounds": 12, "family": "IPv4" }),
        )
        .unwrap();
        assert_eq!(mtr.tool, Tool::Mtr);
        assert_eq!(mtr.target, "example.com");
        assert_eq!(mtr.packets, 12 * 30);
        assert_eq!(mtr.seconds, 14);
        assert_eq!(mtr.deadline, Duration::from_secs(300));
        assert!(mtr.probes());
        assert_eq!(mtr.family(), FamilyChoice::V4);
        let Plan::Mtr {
            options, lookups, ..
        } = mtr.plan
        else {
            panic!("not an MTR plan");
        };
        assert_eq!(options.count, Some(12));
        assert_eq!(options.interval, Duration::from_secs(1));
        assert!(lookups);

        let trace = admit(
            "traceroute",
            "example.com",
            json!({
                "rounds": 2, "maxHops": 20, "firstHop": 3, "waitMilliseconds": 1000,
                "probe": { "protocol": "tcp", "port": 443 }, "mtu": true, "lookups": false
            }),
        )
        .unwrap();
        assert_eq!(trace.packets, 2 * 18 + MTU_PROBES);
        let Plan::Traceroute {
            options,
            tcp_port,
            mtu,
            lookups,
            family,
        } = trace.plan
        else {
            panic!("not a traceroute plan");
        };
        assert_eq!(
            (options.first_ttl, options.max_ttl, options.queries),
            (3, 20, 2)
        );
        assert_eq!(options.wait, Duration::from_secs(1));
        assert_eq!(tcp_port, Some(443));
        assert!(mtu);
        assert!(!lookups);
        assert_eq!(family, FamilyChoice::Auto);

        let dns = admit(
            "dns",
            "example.com",
            json!({ "type": "AAAA", "resolver": "9.9.9.9" }),
        )
        .unwrap();
        assert!(!dns.probes());
        assert_eq!(
            dns.plan,
            Plan::Dns {
                record_type: Some("AAAA".into()),
                resolver: Some("9.9.9.9".parse().unwrap()),
            }
        );

        assert_eq!(admit("bgp", "AS13335", json!({})).unwrap().plan, Plan::Bgp);
    }

    #[test]
    fn a_run_on_a_node_always_ends_on_its_own() {
        let ping = admit("ping", "example.com", json!({})).unwrap();
        assert_eq!(ping.packets, u64::from(DEFAULT_COUNT));
        let Plan::Ping { options, .. } = ping.plan else {
            panic!("not a ping plan");
        };
        assert_eq!(options.count, Some(DEFAULT_COUNT));
        // The run's own deadline is the node's limit, whatever was sent by then.
        assert_eq!(options.deadline, Some(Duration::from_secs(300)));

        let mtr = admit("mtr", "example.com", json!({})).unwrap();
        let Plan::Mtr { options, .. } = mtr.plan else {
            panic!("not an MTR plan");
        };
        assert_eq!(options.count, Some(DEFAULT_COUNT));
    }

    #[test]
    fn a_tool_the_build_lacks_is_refused() {
        // A build without DNS and without the TCP probe.
        let small = ["traceroute", "ping", "mtr"];
        let check = |tool: &str, options: Value| {
            super::check(
                &request(tool, "example.com", options),
                &small,
                &Limits::default(),
                &Allowed::none(),
            )
        };
        assert_eq!(
            check("dns", json!({})),
            Err(Refusal::Tool { tool: "DNS".into() })
        );
        assert_eq!(
            check(
                "traceroute",
                json!({ "probe": { "protocol": "tcp", "port": 443 } })
            ),
            Err(Refusal::Tool {
                tool: "TCP probe".into()
            })
        );
        assert_eq!(
            check("traceroute", json!({ "mtu": true })),
            Err(Refusal::Tool {
                tool: "path MTU check".into()
            })
        );
        // A path MTU check that wasn't asked for needs no build feature.
        assert!(check("traceroute", json!({ "mtu": false })).is_ok());
        // No build runs iperf3 yet, and a word this version never heard of is no tool at all.
        assert_eq!(
            admit("iperf3", "iperf.example.net", json!({})),
            Err(Refusal::Tool {
                tool: "iperf3".into()
            })
        );
        let unknown = admit("nmap", "example.com", json!({})).unwrap_err();
        assert_eq!(
            unknown,
            Refusal::Tool {
                tool: "nmap".into()
            }
        );
        assert_eq!(unknown.kind(), Kind::Tool);
    }

    #[test]
    fn a_node_never_runs_a_command_it_was_sent() {
        // The tool is a word from a fixed list. Anything else is refused as it stands.
        for tool in ["sh", "rvrs ping", "ping; reboot", "../ping", "PING", ""] {
            assert_eq!(
                admit(tool, "example.com", json!({})).unwrap_err().kind(),
                Kind::Tool,
                "{tool}"
            );
        }
        // A target is a name or an address. It is never a flag and never more than one word.
        for target in [
            "",
            "-c 100000 example.com",
            "--help",
            "example.com; reboot",
            "example.com\n10.0.0.1",
            "$(reboot)",
            "example.com/path",
            "a b",
        ] {
            assert_eq!(
                admit("ping", target, json!({})),
                Err(Refusal::Target),
                "{target:?}"
            );
        }
        assert_eq!(
            admit("ping", &"a".repeat(256), json!({})),
            Err(Refusal::Target)
        );
    }

    #[test]
    fn an_option_the_node_does_not_know_is_refused() {
        assert_eq!(
            admit("ping", "example.com", json!({ "burst": 100 })),
            Err(Refusal::UnknownOption {
                option: "burst".into()
            })
        );
        assert_eq!(
            admit("ping", "example.com", json!({ "maxHops": 5 })),
            Err(Refusal::OptionOfAnotherTool {
                tool: "ping",
                option: "maxHops".into()
            })
        );
        assert_eq!(
            admit("bgp", "AS13335", json!({ "lookups": false })),
            Err(Refusal::OptionOfAnotherTool {
                tool: "BGP",
                option: "lookups".into()
            })
        );
        // An allowed range is the owner's to write. A request that carries one is refused.
        assert_eq!(
            admit("ping", "10.0.0.1", json!({ "allow": "10.0.0.0/8" })),
            Err(Refusal::UnknownOption {
                option: "allow".into()
            })
        );
    }

    #[test]
    fn an_option_outside_its_rule_is_refused() {
        let wrong = |tool: &str, options: Value| {
            let refusal = admit(tool, "example.com", options).unwrap_err();
            assert_eq!(refusal.kind(), Kind::Option);
            refusal.error(&crate::node::refusal::Identity {
                name: "fra-1",
                id: None,
                version: "0.15.0",
            })
        };
        assert_eq!(
            wrong("traceroute", json!({ "rounds": 11 })),
            "rounds must be a whole number from 1 to 10."
        );
        assert_eq!(
            wrong("ping", json!({ "packets": 0 })),
            "packets must be a whole number of 1 or more."
        );
        assert_eq!(
            wrong("ping", json!({ "packets": 2.5 })),
            "packets must be a whole number of 1 or more."
        );
        assert_eq!(
            wrong("ping", json!({ "packets": "10" })),
            "packets must be a whole number of 1 or more."
        );
        assert_eq!(
            wrong("mtr", json!({ "maxHops": 256 })),
            "maxHops must be a whole number from 1 to 255."
        );
        assert_eq!(
            wrong("mtr", json!({ "maxHops": 5, "firstHop": 6 })),
            "firstHop must be 5 or less, the last hop to probe."
        );
        assert_eq!(
            wrong("ping", json!({ "waitMilliseconds": 60_001 })),
            "waitMilliseconds must be a whole number from 1 to 60000."
        );
        assert_eq!(
            wrong("ping", json!({ "family": "ipv4" })),
            "family must be IPv4 or IPv6."
        );
        assert_eq!(
            wrong("ping", json!({ "lookups": "no" })),
            "lookups must be true or false."
        );
        for probe in [
            json!("tcp"),
            json!({ "protocol": "udp", "port": 53 }),
            json!({ "protocol": "tcp" }),
            json!({ "protocol": "tcp", "port": 0 }),
            json!({ "protocol": "tcp", "port": 70_000 }),
            json!({ "protocol": "tcp", "port": 443, "flags": "FIN" }),
        ] {
            assert_eq!(
                wrong("traceroute", json!({ "probe": probe })),
                "probe must be { \"protocol\": \"tcp\", \"port\": 1 to 65535 }."
            );
        }
        assert_eq!(
            wrong("dns", json!({ "type": "A; drop" })),
            "type must be a record type such as A or AAAA."
        );
        assert_eq!(
            wrong("dns", json!({ "resolver": "dns.example.net" })),
            "resolver must be an address."
        );
    }

    #[test]
    fn a_request_over_the_nodes_limits_is_refused() {
        // 100 rounds to 30 hops is 3,000 packets.
        assert_eq!(
            admit("mtr", "example.com", json!({ "rounds": 100 })),
            Err(Refusal::Packets {
                packets: 3000,
                limit: 1000
            })
        );
        assert_eq!(
            admit(
                "ping",
                "example.com",
                json!({ "packets": 1001, "intervalMilliseconds": 200 })
            ),
            Err(Refusal::Packets {
                packets: 1001,
                limit: 1000
            })
        );
        // The same MTR to fewer hops fits.
        assert!(admit(
            "mtr",
            "example.com",
            json!({ "rounds": 100, "maxHops": 10 })
        )
        .is_ok());
        // 600 packets a second apart take ten minutes.
        assert_eq!(
            admit("ping", "example.com", json!({ "packets": 600 })),
            Err(Refusal::Seconds {
                seconds: 601,
                limit: 300
            })
        );
        assert_eq!(
            admit(
                "mtr",
                "example.com",
                json!({ "rounds": 30, "intervalMilliseconds": 20_000 })
            ),
            Err(Refusal::Seconds {
                seconds: 602,
                limit: 300
            })
        );
        // A flood is packets close together, whatever their number.
        assert_eq!(
            admit(
                "ping",
                "example.com",
                json!({ "packets": 5, "intervalMilliseconds": 10 })
            ),
            Err(Refusal::Interval {
                milliseconds: 10,
                floor: 200
            })
        );
        assert_eq!(
            admit("mtr", "example.com", json!({ "intervalMilliseconds": 199 })),
            Err(Refusal::Interval {
                milliseconds: 199,
                floor: 200
            })
        );
        assert!(admit(
            "ping",
            "example.com",
            json!({ "intervalMilliseconds": 200 })
        )
        .is_ok());
        for refusal in [
            Refusal::Packets {
                packets: 1,
                limit: 1,
            },
            Refusal::Seconds {
                seconds: 1,
                limit: 1,
            },
            Refusal::Interval {
                milliseconds: 1,
                floor: 1,
            },
        ] {
            assert_eq!(refusal.kind(), Kind::Limit);
        }
    }

    #[test]
    fn the_nodes_own_limits_decide() {
        let tight = Limits {
            packets_per_run: 20,
            seconds_per_run: 10,
            ..Limits::default()
        };
        let check = |tool: &str, options: Value| {
            super::check(
                &request(tool, "example.com", options),
                &FULL,
                &tight,
                &Allowed::none(),
            )
        };
        assert!(check("ping", json!({ "packets": 5 })).is_ok());
        assert_eq!(
            check(
                "ping",
                json!({ "packets": 21, "intervalMilliseconds": 200 })
            ),
            Err(Refusal::Packets {
                packets: 21,
                limit: 20
            })
        );
        // A traceroute to 30 hops is 90 packets, which this node never sends.
        assert_eq!(
            check("traceroute", json!({})),
            Err(Refusal::Packets {
                packets: 90,
                limit: 20
            })
        );
        assert_eq!(
            check("ping", json!({ "packets": 12 })),
            Err(Refusal::Seconds {
                seconds: 13,
                limit: 10
            })
        );
    }

    #[test]
    fn a_private_target_is_refused() {
        for (target, space) in [
            ("10.0.0.1", Space::Private),
            ("192.168.1.1", Space::Private),
            ("172.20.0.5", Space::Private),
            ("fd00::1", Space::Private),
            ("100.64.0.1", Space::Shared),
            ("127.0.0.1", Space::Loopback),
            ("::1", Space::Loopback),
            ("169.254.169.254", Space::LinkLocal),
            ("fe80::1", Space::LinkLocal),
            ("224.0.0.1", Space::Multicast),
            ("255.255.255.255", Space::Broadcast),
            ("0.0.0.0", Space::Reserved),
            ("::ffff:10.0.0.1", Space::Private),
        ] {
            for tool in ["ping", "traceroute", "mtr", "dns"] {
                assert_eq!(
                    admit(tool, target, json!({})),
                    Err(Refusal::Address {
                        subject: target.into(),
                        space
                    }),
                    "{tool} {target}"
                );
            }
        }
        assert!(admit("ping", "1.1.1.1", json!({})).is_ok());
        assert!(admit("ping", "2606:4700:4700::1111", json!({})).is_ok());
        // A resolver is an address the node sends questions to.
        assert_eq!(
            admit("dns", "example.com", json!({ "resolver": "192.168.1.1" })),
            Err(Refusal::Address {
                subject: "The resolver 192.168.1.1".into(),
                space: Space::Private
            })
        );
        // A BGP lookup sends nothing to its target, so a private prefix is a question like any.
        assert!(admit("bgp", "10.0.0.0/8", json!({})).is_ok());
    }

    #[test]
    fn a_name_of_the_nodes_own_network_is_refused_before_it_is_looked_up() {
        for target in [
            "router",
            "nas.lan",
            "printer.local",
            "localhost",
            "db.internal",
        ] {
            for tool in ["ping", "dns"] {
                assert_eq!(
                    admit(tool, target, json!({})),
                    Err(Refusal::LocalName {
                        name: target.into()
                    }),
                    "{tool} {target}"
                );
            }
        }
    }

    #[test]
    fn the_owner_opens_a_range_and_only_that_range() {
        let allowed = Allowed::parse("192.168.1.0/24").unwrap();
        let check = |tool: &str, target: &str, options: Value| {
            super::check(
                &request(tool, target, options),
                &FULL,
                &Limits::default(),
                &allowed,
            )
        };
        assert!(check("ping", "192.168.1.1", json!({})).is_ok());
        assert!(check("dns", "example.com", json!({ "resolver": "192.168.1.1" })).is_ok());
        assert_eq!(
            check("ping", "192.168.2.1", json!({})),
            Err(Refusal::Address {
                subject: "192.168.2.1".into(),
                space: Space::Private
            })
        );
        assert_eq!(
            check("ping", "127.0.0.1", json!({})),
            Err(Refusal::Address {
                subject: "127.0.0.1".into(),
                space: Space::Loopback
            })
        );
        // With a range allowed, a local name is looked up and judged by its address.
        assert!(check("ping", "nas.lan", json!({})).is_ok());
        assert_eq!(
            resolved("nas.lan", "192.168.1.20".parse().unwrap(), &allowed),
            Ok("192.168.1.20".parse().unwrap())
        );
        assert_eq!(
            resolved("nas.lan", "192.168.2.20".parse().unwrap(), &allowed),
            Err(Refusal::Resolved {
                name: "nas.lan".into(),
                space: Space::Private
            })
        );
    }

    #[test]
    fn a_name_is_judged_by_the_address_it_resolves_to() {
        let none = Allowed::none();
        assert_eq!(
            resolved("example.com", "93.184.216.34".parse().unwrap(), &none),
            Ok("93.184.216.34".parse().unwrap())
        );
        let refusal =
            resolved("rebind.example.net", "10.0.0.5".parse().unwrap(), &none).unwrap_err();
        assert_eq!(
            refusal,
            Refusal::Resolved {
                name: "rebind.example.net".into(),
                space: Space::Private
            }
        );
        // The sentence says what kind of address it was and never which one.
        let node = crate::node::refusal::Identity {
            name: "fra-1",
            id: None,
            version: "0.15.0",
        };
        assert!(!refusal.error(&node).contains("10.0.0.5"));
        assert!(!refusal.brief("fra-1").contains("10.0.0.5"));
        // Asked by address, the address is what the person typed.
        assert_eq!(
            resolved("10.0.0.5", "10.0.0.5".parse().unwrap(), &none),
            Err(Refusal::Address {
                subject: "10.0.0.5".into(),
                space: Space::Private
            })
        );
    }

    #[test]
    fn the_options_are_the_ones_the_page_lists() {
        let page = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../schema/node-protocol/README.md"),
        )
        .unwrap();
        let mut listed: Vec<(String, Vec<String>)> = page
            .lines()
            .skip_while(|line| !line.starts_with("| Option | Tools |"))
            .skip(2)
            .take_while(|line| line.starts_with('|'))
            .map(|line| {
                let cells: Vec<&str> = line.split('|').map(str::trim).collect();
                let option = cells[1].trim_matches('`').to_string();
                let tools = cells[2]
                    .split(',')
                    .map(|tool| tool.trim().to_string())
                    .collect();
                (option, tools)
            })
            .collect();
        listed.sort();
        let mut known: Vec<(String, Vec<String>)> = OPTIONS
            .iter()
            .map(|(option, tools)| {
                (
                    option.to_string(),
                    tools.iter().map(|tool| tool.to_string()).collect(),
                )
            })
            .collect();
        known.sort();
        assert_eq!(known, listed);
    }

    #[test]
    fn a_claims_run_is_read_as_a_request() {
        let claim: Value = serde_json::from_str(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../schema/node-protocol/fixtures/claim.response.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let request: Request = serde_json::from_value(claim["runs"][0].clone()).unwrap();
        assert_eq!(request.tool, "mtr");
        assert_eq!(request.attempt, 1);
        let admitted = check(&request, &FULL, &Limits::default(), &Allowed::none()).unwrap();
        assert_eq!(admitted.packets, 360);
    }
}
