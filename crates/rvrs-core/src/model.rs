//! A run and its hops, as the engine's events build them and as history stores them.
//!
//! Stored on disk across versions: a field added later must carry `#[serde(default)]`, or every
//! saved run fails to load and the person's history is gone.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::bgp::{BgpEvent, BgpResult};
use crate::dns::{DnsEvent, DnsResult};
use crate::engine::{Ending, Event, ReplyKind};
use crate::format;
use crate::report::Report;
use crate::{is_private, Family, FamilyChoice};

/// The tools rvrs has. Only the ones `is_available` run in the CLI today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Traceroute,
    Mtr,
    Ping,
    Iperf3,
    Dns,
    Bgp,
}

impl Tool {
    /// The order the Console lists them in. iperf3 is last until the CLI runs it.
    pub const ALL: [Tool; 6] = [
        Tool::Traceroute,
        Tool::Mtr,
        Tool::Ping,
        Tool::Dns,
        Tool::Bgp,
        Tool::Iperf3,
    ];

    /// The subcommand, named after the original tool.
    pub fn command(self) -> &'static str {
        match self {
            Tool::Traceroute => "traceroute",
            Tool::Mtr => "mtr",
            Tool::Ping => "ping",
            Tool::Iperf3 => "iperf3",
            Tool::Dns => "dig",
            Tool::Bgp => "bgp",
        }
    }

    /// The name as prose and reports spell it, matching the app.
    pub fn title(self) -> &'static str {
        match self {
            Tool::Traceroute => "Traceroute",
            Tool::Mtr => "MTR",
            Tool::Ping => "Ping",
            Tool::Iperf3 => "iperf3",
            Tool::Dns => "DNS",
            Tool::Bgp => "BGP",
        }
    }

    pub fn is_available(self) -> bool {
        !matches!(self, Tool::Iperf3)
    }
}

/// The network a host belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    pub number: u32,
    pub name: String,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    /// The announced prefix the address falls in, such as 1.1.1.0/24.
    #[serde(default)]
    pub prefix: Option<String>,
}

impl Network {
    pub fn label(&self) -> String {
        format!("AS{}", self.number)
    }

    pub fn location(&self) -> Option<String> {
        let parts: Vec<&str> = [self.city.as_deref(), self.country.as_deref()]
            .into_iter()
            .flatten()
            .filter(|part| !part.is_empty())
            .collect();
        (!parts.is_empty()).then(|| parts.join(", "))
    }
}

/// One probe of one hop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Slot {
    /// Not sent yet, or sent and still within its wait.
    Pending,
    Lost,
    Reply {
        from: IpAddr,
        /// Round trip in microseconds.
        micros: u64,
        /// The ICMP unreachable code, when the reply was one.
        #[serde(default)]
        unreachable: Option<u8>,
        /// The TTL the reply arrived with. Ping shows it.
        #[serde(default)]
        ttl: Option<u8>,
    },
    /// Ping only: a router answered instead of the target.
    Refused {
        from: IpAddr,
        time_exceeded: bool,
        code: u8,
    },
}

impl Slot {
    pub fn rtt(&self) -> Option<Duration> {
        match self {
            Slot::Reply { micros, .. } => Some(Duration::from_micros(*micros)),
            _ => None,
        }
    }

    pub fn from(&self) -> Option<IpAddr> {
        match self {
            Slot::Reply { from, .. } => Some(*from),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hop {
    pub ttl: u8,
    pub probes: Vec<Slot>,
    #[serde(default)]
    pub is_destination: bool,
    /// The hop's figures as a report states them, for a run that came as a report. They stand
    /// where the samples a report carries are fewer than the probes the run sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported: Option<Reported>,
}

/// What a report says about one hop. Every figure is optional: an origin reports what it
/// measured, and a figure it left out is absent here too, never zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Reported {
    #[serde(default)]
    pub address: Option<IpAddr>,
    #[serde(default)]
    pub sent: Option<u32>,
    #[serde(default)]
    pub received: Option<u32>,
    #[serde(default)]
    pub loss_percent: Option<f64>,
    /// Round trips in microseconds.
    #[serde(default)]
    pub min: Option<u64>,
    #[serde(default)]
    pub mean: Option<u64>,
    #[serde(default)]
    pub max: Option<u64>,
    #[serde(default)]
    pub jitter: Option<u64>,
    #[serde(default)]
    pub p50: Option<u64>,
    #[serde(default)]
    pub p95: Option<u64>,
}

impl Reported {
    /// The report's figures over what the kept samples add up to. A figure the report lacks is
    /// taken from the samples only when they cover every probe the hop sent.
    fn stats(&self, own: HopStats) -> HopStats {
        let sent = self.sent.map_or(own.sent, |sent| sent as usize);
        let received = self
            .received
            .map_or(own.received, |received| received as usize);
        let complete = own.sent == sent && sent > 0;
        let figure = |reported: Option<u64>, computed: Option<Duration>| {
            reported
                .map(Duration::from_micros)
                .or(if complete { computed } else { None })
        };
        HopStats {
            sent,
            received: received.min(sent),
            min: figure(self.min, own.min),
            mean: figure(self.mean, own.mean),
            max: figure(self.max, own.max),
            jitter: figure(self.jitter, own.jitter),
            p50: figure(self.p50, own.p50),
            p95: figure(self.p95, own.p95),
            reported_loss: self.loss_percent.map(|percent| percent / 100.0),
        }
    }
}

impl Hop {
    fn new(ttl: u8, queries: usize) -> Self {
        Self {
            ttl,
            probes: vec![Slot::Pending; queries],
            is_destination: false,
            reported: None,
        }
    }

    /// The first host that answered at this hop.
    pub fn address(&self) -> Option<IpAddr> {
        self.probes
            .iter()
            .find_map(Slot::from)
            .or_else(|| self.reported.as_ref()?.address)
    }

    /// Every host that answered, in the order they first did. More than one means the path
    /// splits here.
    pub fn addresses(&self) -> Vec<IpAddr> {
        let mut seen = Vec::new();
        for address in self.probes.iter().filter_map(Slot::from) {
            if !seen.contains(&address) {
                seen.push(address);
            }
        }
        if seen.is_empty() {
            seen.extend(self.address());
        }
        seen
    }

    pub fn did_reply(&self) -> bool {
        self.address().is_some()
    }

    pub fn is_settled(&self) -> bool {
        self.probes.iter().all(|slot| *slot != Slot::Pending)
    }

    pub fn unreachable_code(&self) -> Option<u8> {
        self.probes.iter().find_map(|slot| match slot {
            Slot::Reply { unreachable, .. } => *unreachable,
            _ => None,
        })
    }

    pub fn stats(&self) -> HopStats {
        let own = HopStats::of(&self.probes);
        match &self.reported {
            Some(reported) => reported.stats(own),
            None => own,
        }
    }
}

/// What a hop's samples add up to. Probes still within their wait count neither as sent nor lost.
#[derive(Debug, Clone, PartialEq)]
pub struct HopStats {
    pub sent: usize,
    pub received: usize,
    pub min: Option<Duration>,
    pub mean: Option<Duration>,
    pub max: Option<Duration>,
    /// Mean absolute difference between consecutive replies, the way mtr reports it.
    pub jitter: Option<Duration>,
    pub p50: Option<Duration>,
    pub p95: Option<Duration>,
    /// The loss a report states, as a fraction, when the run came as one.
    pub reported_loss: Option<f64>,
}

impl HopStats {
    pub fn of(probes: &[Slot]) -> Self {
        let settled: Vec<&Slot> = probes
            .iter()
            .filter(|slot| **slot != Slot::Pending)
            .collect();
        let samples: Vec<Duration> = settled.iter().filter_map(|slot| slot.rtt()).collect();
        let mean =
            (!samples.is_empty()).then(|| samples.iter().sum::<Duration>() / samples.len() as u32);
        let jitter = (samples.len() > 1).then(|| {
            let total: Duration = samples
                .windows(2)
                .map(|pair| pair[0].abs_diff(pair[1]))
                .sum();
            total / (samples.len() as u32 - 1)
        });
        let mut sorted = samples.clone();
        sorted.sort();
        // Nearest rank, as the app computes it.
        let percentile = |fraction: f64| -> Option<Duration> {
            if sorted.is_empty() {
                return None;
            }
            let rank = (sorted.len() as f64 * fraction).ceil() as usize;
            Some(sorted[rank.clamp(1, sorted.len()) - 1])
        };
        Self {
            sent: settled.len(),
            received: samples.len(),
            min: sorted.first().copied(),
            mean,
            max: sorted.last().copied(),
            jitter,
            p50: percentile(0.5),
            p95: percentile(0.95),
            reported_loss: None,
        }
    }

    pub fn lost(&self) -> usize {
        self.sent - self.received
    }

    pub fn loss_fraction(&self) -> f64 {
        if let Some(loss) = self.reported_loss {
            return loss;
        }
        if self.sent == 0 {
            0.0
        } else {
            self.lost() as f64 / self.sent as f64
        }
    }
}

/// How steady a connection looked over a run of packets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grade {
    /// Fewer than ten packets: too few to say.
    Collecting,
    Stable,
    Jittery,
    Lossy,
}

/// A verdict on a ping from its statistics: a title that states the finding and one sentence of
/// evidence. The thresholds and the words are the app's `ConnectionInsight`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub grade: Grade,
    pub title: String,
    pub detail: String,
}

impl Verdict {
    pub fn of(stats: &HopStats) -> Self {
        let verdict = |grade, title: &str, detail: String| Self {
            grade,
            title: title.to_string(),
            detail,
        };
        let (Some(mean), Some(p50), Some(p95)) = (stats.mean, stats.p50, stats.p95) else {
            return verdict(
                Grade::Collecting,
                "Collecting packets",
                "A verdict needs at least 10.".to_string(),
            );
        };
        if stats.sent < 10 {
            return verdict(
                Grade::Collecting,
                "Collecting packets",
                "A verdict needs at least 10.".to_string(),
            );
        }
        let loss = stats.loss_fraction() * 100.0;
        let jitter = stats.jitter.map_or(0.0, format::ms_value);
        let spread = format::ms_value(p95) / format::ms_value(p50).max(0.1);
        if loss >= 5.0 {
            verdict(
                Grade::Lossy,
                "Packet loss",
                format!(
                    "{} of packets got no reply.",
                    format::percent(stats.loss_fraction())
                ),
            )
        } else if loss > 0.0 {
            verdict(
                Grade::Jittery,
                "Occasional loss",
                format!("{} of {} packets got no reply.", stats.lost(), stats.sent),
            )
        } else if jitter > (format::ms_value(mean) * 0.3).max(5.0) || spread > 2.0 {
            verdict(
                Grade::Jittery,
                "Jittery",
                format!(
                    "Latency varies. p95 is {}, median {}.",
                    format::ms(p95),
                    format::ms(p50)
                ),
            )
        } else {
            verdict(
                Grade::Stable,
                "Stable",
                format!(
                    "No loss over {} packets. p95 {}, jitter {}.",
                    stats.sent,
                    format::ms(p95),
                    format::ms(stats.jitter.unwrap_or_default())
                ),
            )
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Reached,
    Exhausted,
    Stopped,
    Failed(String),
}

impl Outcome {
    /// The word reports and history use.
    pub fn word(&self) -> &'static str {
        match self {
            Outcome::Reached => "reached",
            Outcome::Exhausted => "exhausted",
            Outcome::Stopped => "stopped",
            Outcome::Failed(_) => "failed",
        }
    }
}

/// One tool, against one target, from one origin, once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// Short and unique within history: what `rvrs show` takes.
    pub id: String,
    pub tool: Tool,
    /// What the person typed.
    pub target: String,
    #[serde(default)]
    pub destination: Option<IpAddr>,
    #[serde(default)]
    pub family: FamilyChoice,
    /// Seconds since 1970.
    pub started_at: u64,
    pub queries: u8,
    pub max_ttl: u8,
    #[serde(default)]
    pub hops: Vec<Hop>,
    #[serde(default)]
    pub outcome: Option<Outcome>,
    /// Reverse DNS names of the hosts that answered.
    #[serde(default)]
    pub names: BTreeMap<IpAddr, String>,
    /// Networks of the hosts that answered. `None` is a lookup that found no network.
    #[serde(default)]
    pub networks: BTreeMap<IpAddr, Option<Network>>,
    /// The address probes left from, when it could be read.
    #[serde(default)]
    pub source: Option<IpAddr>,
    /// The address the internet saw this device as, when lookups are on and it could be asked.
    #[serde(default)]
    pub public_source: Option<IpAddr>,
    /// The install that made the run, when it wasn't this one: a run the account handed this
    /// device. None is a run made here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// MTR only: how many rounds have been dropped from the front of every hop. A long run keeps
    /// its newest [`Run::MAX_ROUNDS`] rounds, so history stays small.
    #[serde(default)]
    pub first_round: u32,
    /// DNS only: what the sources answered, the delegation path and the DNSSEC verdict.
    #[serde(default)]
    pub dns: Option<DnsResult>,
    /// BGP only: the route, its origin validation, paths, history and network.
    #[serde(default)]
    pub bgp: Option<BgpResult>,
    /// A traceroute that probed with TCP: the port and what it did. None is ICMP.
    #[serde(default)]
    pub tcp: Option<crate::tcp::TcpProbe>,
    /// The path MTU, when the run looked for it (`traceroute --mtu`).
    #[serde(default)]
    pub mtu: Option<crate::mtu::PathMtu>,
    /// The report this run came as, when another surface made it: the app, a node, or a
    /// version of the CLI whose record this one can't read. Its sentences and figures are
    /// shown as they came. None is a run with a record of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<Box<Report>>,
}

impl Run {
    pub fn new(tool: Tool, target: &str, family: FamilyChoice, queries: u8, max_ttl: u8) -> Self {
        Self {
            id: new_id(),
            tool,
            target: target.to_string(),
            destination: None,
            family,
            started_at: format::now(),
            queries,
            max_ttl,
            hops: Vec::new(),
            outcome: None,
            names: BTreeMap::new(),
            networks: BTreeMap::new(),
            source: None,
            public_source: None,
            from: None,
            first_round: 0,
            dns: None,
            bgp: None,
            tcp: None,
            mtu: None,
            report: None,
        }
    }

    /// A run from its report alone. The sentences stay the maker's (`summary`, `reading`), the
    /// figures are the report's, and what it doesn't carry is absent. A probing tool gets its
    /// hops back, with every probe when the report has samples. None is a tool this version
    /// doesn't know.
    pub fn from_report(report: &Report, id: &str, from: Option<String>) -> Option<Self> {
        let tool = Tool::ALL
            .into_iter()
            .find(|tool| tool.title().eq_ignore_ascii_case(&report.tool))?;
        let micros = |milliseconds: f64| (milliseconds * 1000.0).round().max(0.0) as u64;
        let destination: Option<IpAddr> = report
            .address
            .as_deref()
            .and_then(|address| address.parse().ok());
        let slots = |samples: &[Option<f64>], from: Option<IpAddr>| -> Vec<Slot> {
            let Some(from) = from else {
                return Vec::new();
            };
            samples
                .iter()
                .map(|sample| match sample {
                    Some(milliseconds) => Slot::Reply {
                        from,
                        micros: micros(*milliseconds),
                        unreachable: None,
                        ttl: None,
                    },
                    None => Slot::Lost,
                })
                .collect()
        };
        let mut run = Run::new(tool, &report.target, FamilyChoice::Auto, 0, 0);
        run.id = id.to_string();
        run.destination = destination;
        run.family = match report.family.as_deref() {
            Some("IPv4") => FamilyChoice::V4,
            Some("IPv6") => FamilyChoice::V6,
            _ => FamilyChoice::Auto,
        };
        run.started_at = format::seconds_of(&report.started_at).unwrap_or_else(format::now);
        run.outcome = Some(match report.outcome.as_str() {
            "reached" => Outcome::Reached,
            "exhausted" => Outcome::Exhausted,
            "failed" => Outcome::Failed(
                report
                    .error
                    .clone()
                    .unwrap_or_else(|| report.summary.clone()),
            ),
            _ => Outcome::Stopped,
        });
        run.source = report
            .origin
            .source_address
            .as_deref()
            .and_then(|address| address.parse().ok());
        run.public_source = report
            .origin
            .public_address
            .as_deref()
            .and_then(|address| address.parse().ok());
        run.from = from;
        for hop in report.hops.iter().flatten() {
            let address: Option<IpAddr> = hop
                .address
                .as_deref()
                .and_then(|address| address.parse().ok());
            if let Some(address) = address {
                if let Some(name) = &hop.hostname {
                    run.names.insert(address, name.clone());
                }
                let number = hop
                    .asn
                    .as_deref()
                    .and_then(|label| label.trim_start_matches("AS").parse().ok());
                if let Some(number) = number {
                    run.networks.insert(
                        address,
                        Some(Network {
                            number,
                            name: hop.network.clone().unwrap_or_default(),
                            country: None,
                            city: None,
                            prefix: None,
                        }),
                    );
                }
            }
            run.hops.push(Hop {
                ttl: hop.ttl,
                probes: slots(hop.samples.as_deref().unwrap_or_default(), address),
                is_destination: false,
                reported: Some(Reported {
                    address,
                    sent: hop.sent,
                    received: hop.received,
                    loss_percent: hop.loss_percent,
                    min: hop.min_milliseconds.map(micros),
                    mean: hop.avg_milliseconds.map(micros),
                    max: hop.max_milliseconds.map(micros),
                    jitter: hop.jitter_milliseconds.map(micros),
                    p50: None,
                    p95: None,
                }),
            });
        }
        // The destination's figures are the run's own: `measured` names them.
        let whole = Reported {
            address: destination,
            sent: report.figure("sent").map(|sent| sent as u32),
            received: report.figure("received").map(|received| received as u32),
            loss_percent: report.figure("loss"),
            min: report.figure("min").map(micros),
            mean: report.figure("avg").map(micros),
            max: report.figure("max").map(micros),
            jitter: report.figure("jitter").map(micros),
            p50: report.figure("p50").map(micros),
            p95: report.figure("p95").map(micros),
        };
        if tool == Tool::Ping {
            run.hops = vec![Hop {
                ttl: 0,
                probes: slots(report.samples.as_deref().unwrap_or_default(), destination),
                is_destination: true,
                reported: Some(whole),
            }];
        } else {
            // The destination is the last hop that answered from the run's own address. A run
            // that was stopped by hand reached it too, so the outcome alone doesn't say.
            let reached = report.outcome == "reached";
            let is_destination = run.hops.last().is_some_and(|last| match destination {
                Some(destination) => last.address() == Some(destination),
                None => reached,
            }) || (reached && !run.hops.is_empty());
            if let Some(last) = run.hops.last_mut().filter(|_| is_destination) {
                last.is_destination = true;
                if let Some(samples) = report.samples.as_deref().filter(|kept| !kept.is_empty()) {
                    last.probes = slots(samples, last.address().or(destination));
                }
                let address = last.address();
                last.reported = Some(Reported { address, ..whole });
            }
        }
        run.queries = run
            .hops
            .iter()
            .map(|hop| hop.probes.len())
            .max()
            .unwrap_or(0)
            .min(usize::from(u8::MAX)) as u8;
        run.max_ttl = run.hops.last().map_or(0, |hop| hop.ttl);
        if tool == Tool::Mtr {
            // The report keeps a hop's newest rounds. The ones before them were dropped.
            let rounds = run
                .hops
                .iter()
                .map(|hop| hop.probes.len())
                .max()
                .unwrap_or(0);
            let sent = run
                .hops
                .iter()
                .filter_map(|hop| hop.reported.as_ref()?.sent)
                .max()
                .unwrap_or(0) as usize;
            run.first_round = sent.saturating_sub(rounds) as u32;
            // Every hop of an MTR has a slot per round, so short hops are padded at the front.
            for hop in &mut run.hops {
                if !hop.probes.is_empty() && hop.probes.len() < rounds {
                    let mut padded = vec![Slot::Lost; rounds - hop.probes.len()];
                    padded.append(&mut hop.probes);
                    hop.probes = padded;
                }
            }
        }
        run.tcp = report.probe.as_ref().map(|probe| crate::tcp::TcpProbe {
            port: probe.port,
            state: match probe.state.as_deref() {
                Some("open") => Some(crate::tcp::PortState::Open),
                Some("closed") => Some(crate::tcp::PortState::Closed),
                Some("filtered") => Some(crate::tcp::PortState::Filtered),
                _ => None,
            },
            connect_micros: report.figure("connect").map(micros),
            mss: report.figure("mss").map(|mss| mss as u16),
        });
        run.mtu = report.mtu.as_ref().map(|mtu| crate::mtu::PathMtu {
            finding: match mtu.finding.as_str() {
                "full" => crate::mtu::MtuFinding::Full,
                "limited" => crate::mtu::MtuFinding::Limited,
                "silent" => crate::mtu::MtuFinding::Silent,
                _ => crate::mtu::MtuFinding::Unknown,
            },
            mtu: mtu.bytes,
            limited_by: mtu
                .limited_by
                .as_deref()
                .and_then(|address| address.parse().ok()),
            tried: mtu.tried,
        });
        run.report = Some(Box::new(report.clone()));
        Some(run)
    }

    /// What the result likely means, as the maker of the run put it, for a run that came as a
    /// report. A reader shows these sentences and never works them out again from figures it
    /// only has part of. None is a run made here.
    pub fn reading(&self) -> Option<&[String]> {
        self.report
            .as_ref()
            .map(|report| report.interpretation.as_slice())
    }

    /// True for a run that came as a report and whose tool keeps its result in a record of
    /// its own (DNS, BGP, iperf3), which a report doesn't carry.
    pub fn is_report_only(&self) -> bool {
        self.report.is_some()
            && match self.tool {
                Tool::Dns => self.dns.is_none(),
                Tool::Bgp => self.bgp.is_none(),
                Tool::Iperf3 => true,
                _ => false,
            }
    }

    pub fn is_running(&self) -> bool {
        self.outcome.is_none()
    }

    /// True when the target was typed as an address.
    pub fn has_literal_target(&self) -> bool {
        self.target.parse::<IpAddr>().is_ok()
    }

    pub fn resolved_family(&self) -> Option<Family> {
        self.destination.map(Family::of)
    }

    fn hop_mut(&mut self, ttl: u8) -> &mut Hop {
        while self.hops.len() < usize::from(ttl) {
            let next = self.hops.len() as u8 + 1;
            self.hops.push(Hop::new(next, usize::from(self.queries)));
        }
        &mut self.hops[usize::from(ttl) - 1]
    }

    /// Folds one engine event into the run. Returns the address when a host answered for the
    /// first time in this run, which is the moment to look up its name and network.
    pub fn apply(&mut self, event: &Event) -> Option<IpAddr> {
        match event {
            Event::Sent { ttl, .. } => {
                if !self.past_destination(*ttl) {
                    self.hop_mut(*ttl);
                }
                None
            }
            Event::Lost { ttl, round } => {
                if !self.past_destination(*ttl) {
                    if let Some(slot) = self.hop_mut(*ttl).probes.get_mut(usize::from(*round)) {
                        *slot = Slot::Lost;
                    }
                }
                None
            }
            Event::Reply {
                ttl,
                round,
                from,
                rtt,
                kind,
            } => {
                if self.past_destination(*ttl) {
                    return None;
                }
                let is_new = !self.hops.iter().any(|hop| hop.addresses().contains(from));
                let unreachable = match kind {
                    ReplyKind::Unreachable(code) => Some(*code),
                    _ => None,
                };
                let hop = self.hop_mut(*ttl);
                if let Some(slot) = hop.probes.get_mut(usize::from(*round)) {
                    *slot = Slot::Reply {
                        from: *from,
                        micros: rtt.as_micros() as u64,
                        unreachable,
                        ttl: None,
                    };
                }
                if *kind != ReplyKind::Intermediate {
                    hop.is_destination = *kind == ReplyKind::Destination;
                    self.hops.truncate(usize::from(*ttl));
                }
                is_new.then_some(*from)
            }
            Event::Port {
                state,
                connect,
                mss,
            } => {
                if let Some(tcp) = self.tcp.as_mut() {
                    tcp.state = Some(*state);
                    tcp.connect_micros = connect.map(|time| time.as_micros() as u64);
                    tcp.mss = *mss;
                }
                None
            }
            Event::Finished(ending) => {
                self.outcome = Some(match ending {
                    Ending::Reached(_) if self.destination_hop().is_some() => Outcome::Reached,
                    Ending::Reached(_) | Ending::Exhausted => Outcome::Exhausted,
                    Ending::Stopped => Outcome::Stopped,
                });
                // Probes a stop cut short stay pending: they were never given their wait, so
                // they are not losses. A stop also leaves hops nothing answered at, and those
                // say nothing about the path.
                if self.tool == Tool::Ping {
                    // Packets a stop cut short were never given their wait. They are not losses.
                    if let Some(hop) = self.hops.first_mut() {
                        while hop.probes.last() == Some(&Slot::Pending) {
                            hop.probes.pop();
                        }
                    }
                } else if self.outcome != Some(Outcome::Exhausted) {
                    let answered = self.path_len();
                    self.hops.truncate(answered);
                }
                None
            }
            Event::Failed(message) => {
                self.outcome = Some(Outcome::Failed(message.clone()));
                None
            }
            Event::MtrSent { ttl, round } => {
                if !self.past_destination(*ttl) {
                    self.mtr_slot(*ttl, *round);
                }
                None
            }
            Event::MtrLost { ttl, round } => {
                if !self.past_destination(*ttl) {
                    if let Some(slot) = self.mtr_slot(*ttl, *round) {
                        *slot = Slot::Lost;
                    }
                }
                None
            }
            Event::MtrReply {
                ttl,
                round,
                from,
                rtt,
                kind,
            } => {
                if self.past_destination(*ttl) {
                    return None;
                }
                let is_new = !self.hops.iter().any(|hop| hop.addresses().contains(from));
                let unreachable = match kind {
                    ReplyKind::Unreachable(code) => Some(*code),
                    _ => None,
                };
                if let Some(slot) = self.mtr_slot(*ttl, *round) {
                    *slot = Slot::Reply {
                        from: *from,
                        micros: rtt.as_micros() as u64,
                        unreachable,
                        ttl: None,
                    };
                }
                if *kind != ReplyKind::Intermediate {
                    self.hops[usize::from(*ttl) - 1].is_destination =
                        *kind == ReplyKind::Destination;
                    self.hops.truncate(usize::from(*ttl));
                }
                is_new.then_some(*from)
            }
            Event::PingSent { seq } => {
                self.ping_slot(*seq);
                None
            }
            Event::PingLost { seq } => {
                *self.ping_slot(*seq) = Slot::Lost;
                None
            }
            Event::PingRefused {
                seq,
                from,
                time_exceeded,
                code,
            } => {
                *self.ping_slot(*seq) = Slot::Refused {
                    from: *from,
                    time_exceeded: *time_exceeded,
                    code: *code,
                };
                None
            }
            Event::PingReply {
                seq,
                from,
                rtt,
                ttl,
            } => {
                let is_new = !self.hops.iter().any(|hop| hop.addresses().contains(from));
                *self.ping_slot(*seq) = Slot::Reply {
                    from: *from,
                    micros: rtt.as_micros() as u64,
                    unreachable: None,
                    ttl: *ttl,
                };
                self.hops[0].is_destination = true;
                is_new.then_some(*from)
            }
        }
    }

    /// Folds one event of a DNS lookup into the run.
    pub fn apply_dns(&mut self, event: &DnsEvent) {
        let target = self.target.clone();
        let dns = self.dns.get_or_insert_with(|| DnsResult::new(&target));
        match event {
            DnsEvent::Plan(sources) => dns.pending_sources = *sources,
            DnsEvent::Source(source) => {
                dns.sources.push(source.clone());
                dns.pending_sources = dns.pending_sources.saturating_sub(1);
            }
            DnsEvent::Step(step) => dns.path.push(step.clone()),
            DnsEvent::Dnssec(status, detail) => {
                dns.dnssec = *status;
                dns.dnssec_detail = detail.clone();
            }
            DnsEvent::Finished | DnsEvent::Stopped => {
                dns.pending_sources = 0;
                let answered = dns
                    .sources
                    .iter()
                    .any(|source| source.error.is_none() && !source.records.is_empty());
                self.outcome = Some(match event {
                    DnsEvent::Stopped => Outcome::Stopped,
                    _ if answered => Outcome::Reached,
                    _ => Outcome::Exhausted,
                });
            }
            DnsEvent::Failed(message) => {
                dns.pending_sources = 0;
                self.outcome = Some(Outcome::Failed(message.clone()));
            }
        }
    }

    /// Folds one event of a BGP lookup into the run. A lookup that found neither a route nor
    /// a network ends as exhausted, so a script can tell "no route observed" from a result.
    pub fn apply_bgp(&mut self, event: &BgpEvent) {
        let Some(bgp) = self.bgp.as_mut() else {
            return;
        };
        bgp.apply(event);
        match event {
            BgpEvent::Finished => {
                let found = bgp.origin().is_some()
                    || bgp.network.is_some()
                    || bgp.unrouted_reason.is_some();
                self.outcome = Some(if found {
                    Outcome::Reached
                } else {
                    Outcome::Exhausted
                });
            }
            BgpEvent::Stopped => self.outcome = Some(Outcome::Stopped),
            BgpEvent::Failed(message) => self.outcome = Some(Outcome::Failed(message.clone())),
            _ => {}
        }
    }

    /// The network this device is in, when lookups are on and it could be asked. A BGP result
    /// marks it among the collectors' paths.
    pub fn own_asn(&self) -> Option<u32> {
        self.network(self.public_source?)
            .map(|network| network.number)
    }

    /// The most rounds an MTR run keeps. Older ones are dropped from the front of every hop.
    pub const MAX_ROUNDS: usize = 1200;

    /// The slot of one MTR probe. A hop grows a slot per round, and a round that has slid out
    /// of the kept window has none.
    fn mtr_slot(&mut self, ttl: u8, round: u32) -> Option<&mut Slot> {
        if round < self.first_round {
            return None;
        }
        let mut index = (round - self.first_round) as usize;
        if index >= Self::MAX_ROUNDS {
            let drop = index + 1 - Self::MAX_ROUNDS;
            for hop in &mut self.hops {
                let count = drop.min(hop.probes.len());
                hop.probes.drain(..count);
            }
            self.first_round += drop as u32;
            index -= drop;
        }
        let hop = self.hop_mut(ttl);
        if hop.probes.len() <= index {
            hop.probes.resize(index + 1, Slot::Pending);
        }
        hop.probes.get_mut(index)
    }

    /// The hop whose replies describe the whole run: the destination once it answered, else the
    /// farthest hop that did, so a silent target still shows the latency of the path toward it.
    pub fn chart_hop(&self) -> Option<&Hop> {
        self.destination_hop()
            .or_else(|| self.hops.iter().rev().find(|hop| hop.did_reply()))
    }

    /// Whether a hop's loss reaches the end of the path. Loss at a middle hop while the farthest
    /// answering hop is clean is that router rate-limiting its replies, not loss on the path.
    pub fn loss_carries(&self, hop: &Hop) -> bool {
        match self.chart_hop() {
            Some(farthest) if hop.ttl < farthest.ttl => farthest.stats().lost() > 0,
            _ => true,
        }
    }

    /// The hops that drop replies while later hops are clean. One lost probe is noise, so a hop
    /// needs at least two losses and 10% before it is named.
    pub fn rate_limited_hops(&self) -> Vec<u8> {
        self.path()
            .iter()
            .filter(|hop| {
                let stats = hop.stats();
                hop.did_reply()
                    && stats.lost() >= 2
                    && stats.loss_fraction() >= 0.1
                    && !self.loss_carries(hop)
            })
            .map(|hop| hop.ttl)
            .collect()
    }

    /// The sentence that names the hops whose loss doesn't carry, in the app's words, or None.
    pub fn rate_limit_note(&self) -> Option<String> {
        let hops = self.rate_limited_hops();
        let names: Vec<String> = hops.iter().map(u8::to_string).collect();
        let (lead, after) = match names.as_slice() {
            [] => return None,
            [one] => (format!("Hop {one} answers"), "it"),
            [first, second] => (format!("Hops {first} and {second} answer"), "them"),
            [rest @ .., last] => (
                format!("Hops {}, and {last} answer", rest.join(", ")),
                "them",
            ),
        };
        Some(format!(
            "{lead} fewer probes than the hops after {after}. Routers often rate-limit these replies, so the path itself is likely fine."
        ))
    }

    /// "20 pkt · 0.0% loss · 11.2 ms avg" for the destination of an MTR run.
    pub fn mtr_summary(&self) -> String {
        let stats = self
            .destination_hop()
            .map_or_else(|| HopStats::of(&[]), Hop::stats);
        let mut parts = vec![
            format!("{} pkt", stats.sent),
            format!("{} loss", format::percent(stats.loss_fraction())),
        ];
        if let Some(mean) = stats.mean {
            parts.push(format!("{} avg", format::ms(mean)));
        }
        parts.join(" \u{00B7} ")
    }

    /// Ping keeps every packet in one hop, numbered by sequence.
    fn ping_slot(&mut self, seq: u32) -> &mut Slot {
        if self.hops.is_empty() {
            self.hops.push(Hop::new(1, 0));
        }
        let probes = &mut self.hops[0].probes;
        let index = seq as usize;
        if probes.len() <= index {
            probes.resize(index + 1, Slot::Pending);
        }
        &mut probes[index]
    }

    /// The packets of a ping run, in the order they were sent.
    pub fn packets(&self) -> &[Slot] {
        self.hops.first().map_or(&[], |hop| hop.probes.as_slice())
    }

    /// What the packets of a ping run add up to.
    pub fn ping_stats(&self) -> HopStats {
        self.hops
            .first()
            .map_or_else(|| HopStats::of(&[]), Hop::stats)
    }

    /// "20 pkt · 0.0% loss · 11.2 ms avg", the words the app's rows use for ping.
    pub fn packet_summary(&self) -> String {
        let stats = self.ping_stats();
        let mut parts = vec![
            format!("{} pkt", stats.sent),
            format!("{} loss", format::percent(stats.loss_fraction())),
        ];
        if let Some(mean) = stats.mean {
            parts.push(format!("{} avg", format::ms(mean)));
        }
        parts.join(" \u{00B7} ")
    }

    /// True for a TTL beyond a hop that already ended the path.
    fn past_destination(&self, ttl: u8) -> bool {
        self.hops
            .iter()
            .any(|hop| hop.ttl < ttl && (hop.is_destination || hop.unreachable_code().is_some()))
    }

    /// The hops up to the farthest one that answered. Silent hops behind it are not part of
    /// what the run learned about the path.
    /// What the search for the path MTU means when it didn't simply find the full size: an
    /// inference for packets that vanish, a plain statement for a router that said so.
    pub fn mtu_note(&self) -> Option<String> {
        use crate::mtu::MtuFinding;
        let found = self.mtu.as_ref()?;
        match (found.finding, found.mtu) {
            (MtuFinding::Full, _) => None,
            (MtuFinding::Limited, Some(mtu)) => {
                let router = match found.limited_by {
                    Some(address) => {
                        let hop = self
                            .hops
                            .iter()
                            .find(|hop| hop.addresses().contains(&address))
                            .map(|hop| format!("hop {}, ", hop.ttl))
                            .unwrap_or_default();
                        match self.network(address) {
                            Some(network) => {
                                format!("{hop}{address} in {} {}", network.label(), network.name)
                            }
                            None => format!("{hop}{address}"),
                        }
                    }
                    None => "A router".to_string(),
                };
                Some(format!(
                    "The path carries packets of up to {mtu} bytes. {router} reports that larger ones don't fit, so path MTU discovery works here."
                ))
            }
            (MtuFinding::Silent, Some(mtu)) => Some(format!(
                "Packets of up to {mtu} bytes cross. Larger ones get no answer and no router says why, so a router on the path likely drops them silently. Connections then hang once they send full-size packets, which shows first in TLS handshakes and large transfers."
            )),
            _ => Some(
                "The target answered no ping of any size, so the path MTU couldn't be measured. A TCP trace with -T -p reports the MSS of a handshake."
                    .to_string(),
            ),
        }
    }

    /// TCP: what a trace to a port that never answered found, worded as the inference it is.
    /// None for an ICMP run, and for a port that answered.
    pub fn tcp_filter_note(&self) -> Option<String> {
        let tcp = self.tcp.as_ref()?;
        if self.outcome != Some(Outcome::Exhausted) || self.unreachable().is_some() {
            return None;
        }
        let port = tcp.port;
        let Some(hop) = self.path().last() else {
            return Some(format!(
                "Nothing answered on TCP port {port}, and no router on the way did either. A filter close to this device likely drops the SYN."
            ));
        };
        let router = match hop.address() {
            Some(address) => match self.network(address) {
                Some(network) => format!("{address} in {} {}", network.label(), network.name),
                None => address.to_string(),
            },
            None => "a silent router".to_string(),
        };
        Some(format!(
            "Nothing answered on TCP port {port}. The last router to answer is hop {}, {router}, so a filter likely drops the SYN after it or at the destination.",
            hop.ttl
        ))
    }

    pub fn path(&self) -> &[Hop] {
        &self.hops[..self.path_len()]
    }

    pub fn path_len(&self) -> usize {
        self.hops
            .iter()
            .rposition(Hop::did_reply)
            .map_or(0, |index| index + 1)
    }

    pub fn destination_hop(&self) -> Option<&Hop> {
        self.hops.iter().rev().find(|hop| hop.is_destination)
    }

    /// Mean round trip to the destination.
    pub fn total_rtt(&self) -> Option<Duration> {
        self.destination_hop()?.stats().mean
    }

    /// The hop that reported the destination unreachable, with the ICMP code.
    pub fn unreachable(&self) -> Option<(u8, u8)> {
        self.hops
            .iter()
            .find_map(|hop| hop.unreachable_code().map(|code| (hop.ttl, code)))
    }

    pub fn name(&self, address: IpAddr) -> Option<&str> {
        self.names.get(&address).map(String::as_str)
    }

    pub fn network(&self, address: IpAddr) -> Option<&Network> {
        self.networks.get(&address)?.as_ref()
    }

    /// How many different public networks the path crosses.
    pub fn network_count(&self) -> usize {
        let mut numbers: Vec<u32> = self
            .path()
            .iter()
            .filter_map(|hop| self.network(hop.address()?).map(|network| network.number))
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        numbers.len()
    }

    /// "8 hops · 12.4 ms", the same words the app's rows use.
    pub fn summary(&self) -> String {
        // A run that came as a report says it in its maker's words.
        if let Some(report) = &self.report {
            return report.summary.clone();
        }
        if self.tool == Tool::Dns {
            // The app's words for DNS: the addresses, whether the sources agree, and DNSSEC.
            let found = self.dns.as_ref().filter(|dns| !dns.is_empty());
            return match (&self.outcome, found) {
                (Some(Outcome::Failed(_)), _) => "Failed".to_string(),
                (None, None) => "Running".to_string(),
                (_, None) => "No result".to_string(),
                (Some(Outcome::Stopped), Some(dns)) => {
                    format!("Stopped \u{00B7} {}", dns.summary())
                }
                (_, Some(dns)) => dns.summary(),
            };
        }
        if self.tool == Tool::Bgp {
            let found = self.bgp.as_ref().filter(|bgp| !bgp.is_empty());
            return match (&self.outcome, found) {
                (Some(Outcome::Failed(_)), _) => "Failed".to_string(),
                (None, None) => "Running".to_string(),
                (_, None) => "No result".to_string(),
                (Some(Outcome::Stopped), Some(bgp)) => {
                    format!("Stopped \u{00B7} {}", bgp.summary())
                }
                (_, Some(bgp)) => bgp.summary(),
            };
        }
        if self.tool == Tool::Ping {
            return match &self.outcome {
                Some(Outcome::Failed(_)) => "Failed".to_string(),
                None if self.packets().is_empty() => "Running".to_string(),
                _ => self.packet_summary(),
            };
        }
        let count = self.path_len();
        let hops = if count == 1 {
            "1 hop".to_string()
        } else {
            format!("{count} hops")
        };
        if self.tool == Tool::Mtr {
            // The app's words for MTR: the packets to the destination, reached or stopped.
            return match &self.outcome {
                Some(Outcome::Failed(_)) => "Failed".to_string(),
                Some(Outcome::Exhausted) => format!("No reply after {hops}"),
                None if self.hops.is_empty() => "Running".to_string(),
                _ if self.destination_hop().is_none() && self.outcome.is_some() => {
                    format!("No reply after {hops}")
                }
                _ => self.mtr_summary(),
            };
        }
        // A TCP traceroute says what the port did.
        let port = self
            .tcp
            .as_ref()
            .and_then(|tcp| Some((tcp.port, tcp.state?)));
        match &self.outcome {
            Some(Outcome::Reached) => {
                let mut text = match self.total_rtt() {
                    Some(rtt) => format!("{hops} \u{00B7} {}", format::ms(rtt)),
                    None => hops,
                };
                if let Some((port, state)) = port {
                    text.push_str(&format!(" \u{00B7} port {port} {}", state.word()));
                }
                text
            }
            Some(Outcome::Exhausted) if self.tcp.is_some() && self.unreachable().is_none() => {
                let port = self.tcp.as_ref().map_or(0, |tcp| tcp.port);
                format!("Port {port} filtered \u{00B7} no reply after {hops}")
            }
            Some(Outcome::Exhausted) => format!("No reply after {hops}"),
            Some(Outcome::Stopped) => format!("Stopped after {hops}"),
            Some(Outcome::Failed(_)) => "Failed".to_string(),
            None => "Running".to_string(),
        }
    }
}

/// Consecutive hops that belong to the same network.
#[derive(Debug, Clone, PartialEq)]
pub struct Group<'a> {
    pub network: Option<&'a Network>,
    pub hops: Vec<&'a Hop>,
}

/// Groups hops by network, in path order. Private and silent hops have no network of their own
/// and join the group around them: leading ones fold into the first known network, later ones
/// stay with the network before them. A public hop whose network is unknown forms a group
/// without a header and never borrows a neighbor's.
pub fn groups(run: &Run) -> Vec<Group<'_>> {
    struct Building<'a> {
        network: Option<&'a Network>,
        hops: Vec<&'a Hop>,
        can_adopt: bool,
    }
    let mut groups: Vec<Building> = Vec::new();
    for hop in run.path() {
        let address = hop.address();
        let network = address.and_then(|address| run.network(address));
        let anonymous = address.is_none_or(is_private);
        match (network, groups.last_mut()) {
            (Some(network), Some(last)) if last.network.is_none() && last.can_adopt => {
                last.network = Some(network);
                last.can_adopt = false;
                last.hops.push(hop);
            }
            (Some(network), Some(last))
                if last
                    .network
                    .is_some_and(|known| known.number == network.number) =>
            {
                last.hops.push(hop);
            }
            (Some(network), _) => groups.push(Building {
                network: Some(network),
                hops: vec![hop],
                can_adopt: false,
            }),
            (None, Some(last)) if anonymous => last.hops.push(hop),
            (None, None) if anonymous => groups.push(Building {
                network: None,
                hops: vec![hop],
                can_adopt: true,
            }),
            (None, Some(last)) if last.network.is_none() => {
                last.hops.push(hop);
                last.can_adopt = false;
            }
            (None, _) => groups.push(Building {
                network: None,
                hops: vec![hop],
                can_adopt: false,
            }),
        }
    }
    groups
        .into_iter()
        .map(|group| Group {
            network: group.network,
            hops: group.hops,
        })
        .collect()
}

/// How many characters a new id has. With 31 letters to pick from that is 128 bits, so an id
/// never repeats on an install however many runs it makes. The account tells an install's runs
/// apart by it for good. The six characters ids had until rvrs 0.15 repeated within some ten
/// thousand runs, and the API answered for the second run with the first one's.
pub const ID_LENGTH: usize = 26;

/// How many characters of an id a list shows, and a message names a run by. `History::find`
/// takes the start of an id, so what a list shows is what a person types.
pub const SHORT_ID: usize = 6;

/// The first `width` characters of an id.
pub fn short_id(id: &str, width: usize) -> &str {
    id.char_indices()
        .nth(width)
        .map_or(id, |(end, _)| &id[..end])
}

/// How many characters tell these ids apart: `SHORT_ID`, or more where two of them start
/// alike. Cut to that width, each id is the start of one run's id only.
pub fn id_width<'a>(ids: impl Iterator<Item = &'a str>) -> usize {
    let mut ids: Vec<&str> = ids.collect();
    ids.sort_unstable();
    ids.dedup();
    let alike = |pair: &[&str]| {
        pair[0]
            .chars()
            .zip(pair[1].chars())
            .take_while(|(a, b)| a == b)
            .count()
    };
    ids.windows(2)
        .map(|pair| alike(pair) + 1)
        .max()
        .unwrap_or(0)
        .max(SHORT_ID)
}

/// Random bytes from the system. Where it has no random device, as in a sandbox without `/dev`,
/// they come from the keys the standard library seeds its hash maps with.
fn random_bytes<const N: usize>() -> [u8; N] {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::io::Read;
    let mut bytes = [0u8; N];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
    if read.is_err() {
        for part in bytes.chunks_mut(8) {
            let word = RandomState::new().build_hasher().finish().to_le_bytes();
            part.copy_from_slice(&word[..part.len()]);
        }
    }
    bytes
}

pub(crate) fn new_id() -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    // The largest multiple of 31 a byte holds. A byte past it is left out, since taking it
    // would make the first letters of the alphabet more likely than the rest.
    const FAIR: u8 = 248;
    let mut id = String::with_capacity(ID_LENGTH);
    while id.len() < ID_LENGTH {
        for byte in random_bytes::<32>() {
            if byte < FAIR && id.len() < ID_LENGTH {
                id.push(ALPHABET[usize::from(byte) % ALPHABET.len()] as char);
            }
        }
    }
    id
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_new_id_is_26_letters_and_no_two_are_alike() {
        let ids: Vec<String> = (0..2000).map(|_| new_id()).collect();
        for id in &ids {
            assert_eq!(id.len(), ID_LENGTH);
            assert!(id
                .bytes()
                .all(|letter| b"abcdefghjkmnpqrstuvwxyz23456789".contains(&letter)));
        }
        let mut distinct = ids.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(distinct.len(), ids.len());
        // Every letter turns up, so no part of the alphabet is left out.
        let letters: std::collections::BTreeSet<u8> =
            ids.iter().flat_map(|id| id.bytes()).collect();
        assert_eq!(letters.len(), 31);
    }

    #[test]
    fn a_list_shows_six_characters_of_an_id_or_as_many_as_tell_two_apart() {
        assert_eq!(short_id("k3m9abzzzz", 6), "k3m9ab");
        // An id from before 0.15 has six characters and is shown whole.
        assert_eq!(short_id("abc234", 8), "abc234");
        assert_eq!(id_width(std::iter::empty()), SHORT_ID);
        assert_eq!(id_width(["k3m9abxx", "p7q2rsxx"].into_iter()), 6);
        // Two ids that start alike for seven characters need eight.
        assert_eq!(
            id_width(["k3m9abc1", "k3m9abc2", "p7q2rsxx"].into_iter()),
            8
        );
        // An old id that is the start of a new one: the new one needs one character more.
        assert_eq!(id_width(["abc234", "abc234k9"].into_iter()), 7);
        // The same id twice, as a run that is in history and saved, is one id.
        assert_eq!(id_width(["k3m9abxx", "k3m9abxx"].into_iter()), 6);
    }

    pub fn reply(ttl: u8, round: u8, from: &str, ms: f64, kind: ReplyKind) -> Event {
        Event::Reply {
            ttl,
            round,
            from: from.parse().unwrap(),
            rtt: Duration::from_secs_f64(ms / 1000.0),
            kind,
        }
    }

    /// Three hops, the last one the destination, two probes each.
    pub fn reached_run() -> Run {
        let mut run = Run::new(
            Tool::Traceroute,
            "one.one.one.one",
            FamilyChoice::Auto,
            2,
            30,
        );
        run.destination = Some("1.1.1.1".parse().unwrap());
        for round in 0..2 {
            for ttl in 1..=5 {
                run.apply(&Event::Sent { ttl, round });
            }
            run.apply(&reply(
                1,
                round,
                "192.168.1.1",
                1.0 + f64::from(round),
                ReplyKind::Intermediate,
            ));
            run.apply(&Event::Lost { ttl: 2, round });
            run.apply(&reply(
                3,
                round,
                "1.1.1.1",
                12.0 + f64::from(round),
                ReplyKind::Destination,
            ));
        }
        run.apply(&Event::Finished(Ending::Reached(3)));
        run
    }

    #[test]
    fn a_destination_reply_trims_the_hops_behind_it() {
        let run = reached_run();
        assert_eq!(run.hops.len(), 3);
        assert_eq!(run.outcome, Some(Outcome::Reached));
        assert!(run.hops[2].is_destination);
        assert_eq!(run.hops[1].probes, vec![Slot::Lost, Slot::Lost]);
        assert_eq!(run.summary(), "3 hops \u{00B7} 12.5 ms");
    }

    #[test]
    fn the_first_reply_of_a_host_asks_for_lookups_once() {
        let mut run = Run::new(Tool::Traceroute, "example.com", FamilyChoice::Auto, 3, 30);
        assert!(run
            .apply(&reply(1, 0, "10.0.0.1", 1.0, ReplyKind::Intermediate))
            .is_some());
        assert!(run
            .apply(&reply(1, 1, "10.0.0.1", 1.0, ReplyKind::Intermediate))
            .is_none());
        assert!(run
            .apply(&reply(1, 2, "10.0.0.2", 1.0, ReplyKind::Intermediate))
            .is_some());
        assert_eq!(run.hops[0].addresses().len(), 2);
    }

    #[test]
    fn a_silent_target_keeps_only_the_hops_that_answered_in_its_path() {
        let mut run = Run::new(Tool::Traceroute, "192.0.2.1", FamilyChoice::Auto, 1, 5);
        for ttl in 1..=5 {
            run.apply(&Event::Sent { ttl, round: 0 });
        }
        run.apply(&reply(2, 0, "10.0.0.1", 3.0, ReplyKind::Intermediate));
        for ttl in [1, 3, 4, 5] {
            run.apply(&Event::Lost { ttl, round: 0 });
        }
        run.apply(&Event::Finished(Ending::Exhausted));
        assert_eq!(run.hops.len(), 5);
        assert_eq!(run.path_len(), 2);
        assert_eq!(run.summary(), "No reply after 2 hops");
    }

    #[test]
    fn a_stop_drops_what_never_answered() {
        let mut run = Run::new(Tool::Traceroute, "example.com", FamilyChoice::Auto, 3, 30);
        for ttl in 1..=4 {
            run.apply(&Event::Sent { ttl, round: 0 });
        }
        run.apply(&reply(1, 0, "10.0.0.1", 1.0, ReplyKind::Intermediate));
        run.apply(&Event::Finished(Ending::Stopped));
        assert_eq!(run.hops.len(), 1);
        assert_eq!(run.hops[0].stats().sent, 1);
        assert_eq!(run.summary(), "Stopped after 1 hop");
    }

    #[test]
    fn an_unreachable_reply_ends_the_path_without_reaching() {
        let mut run = Run::new(Tool::Traceroute, "example.com", FamilyChoice::Auto, 1, 30);
        run.apply(&reply(1, 0, "10.0.0.1", 1.0, ReplyKind::Intermediate));
        run.apply(&reply(2, 0, "10.0.0.2", 2.0, ReplyKind::Unreachable(1)));
        run.apply(&Event::Finished(Ending::Reached(2)));
        assert_eq!(run.outcome, Some(Outcome::Exhausted));
        assert_eq!(run.unreachable(), Some((2, 1)));
    }

    #[test]
    fn stats_match_the_apps_definitions() {
        let slots: Vec<Slot> = [10u64, 14, 12, 30]
            .iter()
            .map(|ms| Slot::Reply {
                ttl: None,
                from: "1.1.1.1".parse().unwrap(),
                micros: ms * 1000,
                unreachable: None,
            })
            .chain([Slot::Lost, Slot::Pending])
            .collect();
        let stats = HopStats::of(&slots);
        assert_eq!((stats.sent, stats.received, stats.lost()), (5, 4, 1));
        assert_eq!(stats.mean, Some(Duration::from_micros(16_500)));
        // |14-10| + |12-14| + |30-12| = 24, over 3 steps.
        assert_eq!(stats.jitter, Some(Duration::from_millis(8)));
        assert_eq!(stats.p50, Some(Duration::from_millis(12)));
        assert_eq!(stats.p95, Some(Duration::from_millis(30)));
        assert!((stats.loss_fraction() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn private_hops_join_the_network_around_them() {
        let mut run = Run::new(Tool::Traceroute, "example.com", FamilyChoice::Auto, 1, 30);
        let hosts = [
            "192.168.1.1",
            "198.51.100.1",
            "10.9.9.9",
            "198.51.100.2",
            "1.1.1.1",
        ];
        for (index, host) in hosts.iter().enumerate() {
            run.apply(&reply(
                index as u8 + 1,
                0,
                host,
                1.0,
                ReplyKind::Intermediate,
            ));
        }
        let example_net = Network {
            number: 64500,
            name: "Example Net".into(),
            country: None,
            city: None,
            prefix: None,
        };
        let cloudflare = Network {
            number: 13335,
            name: "Cloudflare, Inc.".into(),
            country: None,
            city: None,
            prefix: None,
        };
        run.networks
            .insert(hosts[1].parse().unwrap(), Some(example_net.clone()));
        run.networks
            .insert(hosts[3].parse().unwrap(), Some(example_net));
        run.networks
            .insert(hosts[4].parse().unwrap(), Some(cloudflare));
        let groups = groups(&run);
        let shape: Vec<(Option<u32>, usize)> = groups
            .iter()
            .map(|group| (group.network.map(|n| n.number), group.hops.len()))
            .collect();
        assert_eq!(shape, vec![(Some(64500), 4), (Some(13335), 1)]);
    }

    #[test]
    fn a_public_hop_without_a_network_stands_alone() {
        let mut run = Run::new(Tool::Traceroute, "example.com", FamilyChoice::Auto, 1, 30);
        run.apply(&reply(1, 0, "198.51.100.1", 1.0, ReplyKind::Intermediate));
        run.apply(&reply(2, 0, "8.8.8.8", 1.0, ReplyKind::Intermediate));
        run.networks.insert(
            "198.51.100.1".parse().unwrap(),
            Some(Network {
                number: 64500,
                name: "T".into(),
                country: None,
                city: None,
                prefix: None,
            }),
        );
        let groups = groups(&run);
        assert_eq!(groups.len(), 2);
        assert!(groups[1].network.is_none());
    }

    fn ping_run(times: &[Option<f64>]) -> Run {
        let mut run = Run::new(Tool::Ping, "1.1.1.1", FamilyChoice::Auto, 1, 64);
        run.destination = Some("1.1.1.1".parse().unwrap());
        for (seq, time) in times.iter().enumerate() {
            let seq = seq as u32;
            run.apply(&Event::PingSent { seq });
            match time {
                Some(ms) => run.apply(&Event::PingReply {
                    seq,
                    from: "1.1.1.1".parse().unwrap(),
                    rtt: Duration::from_secs_f64(ms / 1000.0),
                    ttl: Some(57),
                }),
                None => run.apply(&Event::PingLost { seq }),
            };
        }
        run
    }

    #[test]
    fn ping_keeps_every_packet_in_one_hop() {
        let mut run = ping_run(&[Some(10.0), None, Some(12.0)]);
        run.apply(&Event::PingSent { seq: 3 });
        run.apply(&Event::Finished(Ending::Stopped));
        assert_eq!(run.hops.len(), 1);
        // The packet a stop cut short is dropped: it was never given its wait.
        assert_eq!(run.packets().len(), 3);
        assert_eq!(
            run.summary(),
            "3 pkt \u{00B7} 33.3% loss \u{00B7} 11.0 ms avg"
        );
        assert_eq!(run.outcome, Some(Outcome::Stopped));
    }

    #[test]
    fn a_refusal_counts_as_sent_and_not_received() {
        let mut run = ping_run(&[Some(10.0)]);
        run.apply(&Event::PingSent { seq: 1 });
        run.apply(&Event::PingRefused {
            seq: 1,
            from: "10.0.0.1".parse().unwrap(),
            time_exceeded: true,
            code: 0,
        });
        let stats = run.ping_stats();
        assert_eq!((stats.sent, stats.received), (2, 1));
    }

    #[test]
    fn verdicts_use_the_apps_thresholds_and_words() {
        let steady: Vec<Option<f64>> = (0..12)
            .map(|i| Some(10.0 + f64::from(i % 2) * 0.4))
            .collect();
        let verdict = Verdict::of(&ping_run(&steady).ping_stats());
        assert_eq!(verdict.grade, Grade::Stable);
        assert_eq!(
            verdict.detail,
            "No loss over 12 packets. p95 10.4 ms, jitter 0.4 ms."
        );

        let mut lossy = steady.clone();
        lossy[3] = None;
        let verdict = Verdict::of(&ping_run(&lossy).ping_stats());
        assert_eq!(
            (verdict.title.as_str(), verdict.detail.as_str()),
            ("Packet loss", "8.3% of packets got no reply.")
        );

        let mut long: Vec<Option<f64>> = (0..40).map(|_| Some(10.0)).collect();
        long[7] = None;
        let verdict = Verdict::of(&ping_run(&long).ping_stats());
        assert_eq!(
            (verdict.title.as_str(), verdict.detail.as_str()),
            ("Occasional loss", "1 of 40 packets got no reply.")
        );

        let spiky: Vec<Option<f64>> = (0..12)
            .map(|i| Some(if i % 3 == 0 { 60.0 } else { 10.0 }))
            .collect();
        assert_eq!(Verdict::of(&ping_run(&spiky).ping_stats()).title, "Jittery");

        assert_eq!(
            Verdict::of(&ping_run(&[Some(1.0); 5]).ping_stats()).grade,
            Grade::Collecting
        );
    }

    /// An MTR run of `rounds` rounds over three hops: hop 2 drops what `drops` says, the
    /// destination at hop 3 answers everything.
    pub fn mtr_run(rounds: u32, drops: impl Fn(u32) -> bool) -> Run {
        let mut run = Run::new(Tool::Mtr, "example.com", FamilyChoice::Auto, 0, 30);
        run.destination = Some("93.184.216.34".parse().unwrap());
        let reply = |ttl: u8, round: u32, from: &str, ms: u64, kind: ReplyKind| Event::MtrReply {
            ttl,
            round,
            from: from.parse().unwrap(),
            rtt: Duration::from_millis(ms),
            kind,
        };
        for round in 0..rounds {
            for ttl in 1..=3 {
                run.apply(&Event::MtrSent { ttl, round });
            }
            run.apply(&reply(1, round, "192.168.1.1", 1, ReplyKind::Intermediate));
            if drops(round) {
                run.apply(&Event::MtrLost { ttl: 2, round });
            } else {
                run.apply(&reply(2, round, "198.51.100.1", 8, ReplyKind::Intermediate));
            }
            run.apply(&reply(
                3,
                round,
                "93.184.216.34",
                12,
                ReplyKind::Destination,
            ));
        }
        run
    }

    #[test]
    fn mtr_grows_a_slot_per_round_on_every_hop() {
        let mut run = mtr_run(10, |_| false);
        assert_eq!(run.hops.len(), 3);
        assert!(run.hops.iter().all(|hop| hop.probes.len() == 10));
        assert!(run.hops[2].is_destination);
        run.apply(&Event::Finished(Ending::Reached(3)));
        assert_eq!(run.outcome, Some(Outcome::Reached));
        assert_eq!(
            run.summary(),
            "10 pkt \u{00B7} 0.0% loss \u{00B7} 12.0 ms avg"
        );
        assert_eq!(run.rate_limit_note(), None);
    }

    #[test]
    fn loss_at_a_middle_hop_with_a_clean_destination_is_rate_limiting() {
        let run = mtr_run(10, |round| round % 3 == 0);
        assert_eq!(run.hops[1].stats().lost(), 4);
        assert!(!run.loss_carries(&run.hops[1]));
        assert!(run.loss_carries(&run.hops[2]));
        assert_eq!(run.rate_limited_hops(), vec![2]);
        assert_eq!(
            run.rate_limit_note().unwrap(),
            "Hop 2 answers fewer probes than the hops after it. Routers often rate-limit these replies, so the path itself is likely fine."
        );
        // One lost probe is noise and names nobody.
        assert_eq!(mtr_run(10, |round| round == 4).rate_limit_note(), None);
    }

    #[test]
    fn loss_that_reaches_the_destination_carries() {
        let mut run = mtr_run(10, |round| round < 4);
        run.apply(&Event::MtrSent { ttl: 3, round: 10 });
        run.apply(&Event::MtrLost { ttl: 3, round: 10 });
        run.apply(&Event::MtrSent { ttl: 3, round: 11 });
        run.apply(&Event::MtrLost { ttl: 3, round: 11 });
        assert!(run.loss_carries(&run.hops[1]));
        assert_eq!(run.rate_limit_note(), None);
    }

    #[test]
    fn several_rate_limited_hops_are_listed_as_the_app_lists_them() {
        let mut run = Run::new(Tool::Mtr, "example.com", FamilyChoice::Auto, 0, 30);
        for round in 0..10u32 {
            for ttl in 1..=4u8 {
                run.apply(&Event::MtrSent { ttl, round });
                if ttl < 4 && round < 5 {
                    run.apply(&Event::MtrLost { ttl, round });
                } else {
                    run.apply(&Event::MtrReply {
                        ttl,
                        round,
                        from: format!("198.51.100.{ttl}").parse().unwrap(),
                        rtt: Duration::from_millis(5),
                        kind: if ttl == 4 {
                            ReplyKind::Destination
                        } else {
                            ReplyKind::Intermediate
                        },
                    });
                }
            }
        }
        assert!(run
            .rate_limit_note()
            .unwrap()
            .starts_with("Hops 1, 2, and 3 answer fewer probes than the hops after them."));
    }

    #[test]
    fn a_long_mtr_run_keeps_its_newest_rounds() {
        let rounds = Run::MAX_ROUNDS as u32 + 25;
        let run = mtr_run(rounds, |round| round < 10);
        assert_eq!(run.first_round, 25);
        assert!(run
            .hops
            .iter()
            .all(|hop| hop.probes.len() == Run::MAX_ROUNDS));
        // The ten early losses at hop 2 have slid out of the window.
        assert_eq!(run.hops[1].stats().lost(), 0);
    }

    #[test]
    fn a_stopped_mtr_keeps_what_was_measured() {
        let mut run = mtr_run(5, |_| false);
        run.apply(&Event::MtrSent { ttl: 1, round: 5 });
        run.apply(&Event::Finished(Ending::Stopped));
        assert_eq!(run.outcome, Some(Outcome::Stopped));
        // The probe the stop cut short is neither sent nor lost.
        assert_eq!(run.hops[0].stats().sent, 5);
        assert_eq!(
            run.summary(),
            "5 pkt \u{00B7} 0.0% loss \u{00B7} 12.0 ms avg"
        );
    }

    #[test]
    fn a_dns_run_collects_what_the_sources_answer() {
        use crate::dns::result::tests::agreeing;
        let lookup = agreeing();
        let mut run = Run::new(Tool::Dns, "example.com", FamilyChoice::Auto, 0, 0);
        assert_eq!(run.summary(), "Running");
        run.apply_dns(&DnsEvent::Plan(3));
        for source in &lookup.sources {
            run.apply_dns(&DnsEvent::Source(source.clone()));
        }
        assert_eq!(run.dns.as_ref().unwrap().pending_sources, 0);
        for step in &lookup.path {
            run.apply_dns(&DnsEvent::Step(step.clone()));
        }
        run.apply_dns(&DnsEvent::Dnssec(
            lookup.dnssec,
            lookup.dnssec_detail.clone(),
        ));
        assert!(run.is_running());
        run.apply_dns(&DnsEvent::Finished);
        assert_eq!(run.outcome, Some(Outcome::Reached));
        assert_eq!(
            run.summary(),
            "2 addresses \u{00B7} all agree \u{00B7} signed"
        );
        // A run stored with its DNS result reads back whole.
        let stored: Run = serde_json::from_str(&serde_json::to_string(&run).unwrap()).unwrap();
        assert_eq!(stored, run);
    }

    #[test]
    fn a_bgp_run_ends_reached_with_a_route_and_exhausted_without_one() {
        use crate::bgp::{BgpEvent, BgpResult, Origin, TargetKind};
        let start = |target: &str, kind| {
            let mut run = Run::new(Tool::Bgp, target, FamilyChoice::Auto, 0, 0);
            run.bgp = Some(BgpResult::new(kind, target));
            run
        };
        let mut routed = start("192.0.2.0/24", TargetKind::Prefix);
        assert_eq!(routed.summary(), "Running");
        routed.apply_bgp(&BgpEvent::Route("192.0.2.0/24".to_string()));
        routed.apply_bgp(&BgpEvent::Overview {
            announced: Some(true),
            origins: vec![Origin {
                asn: 64500,
                holder: None,
                route_objects: Vec::new(),
            }],
            registry: None,
        });
        routed.apply_bgp(&BgpEvent::Finished);
        assert_eq!(routed.outcome, Some(Outcome::Reached));
        assert_eq!(routed.summary(), "AS64500");

        let mut unseen = start("192.0.2.0/24", TargetKind::Prefix);
        unseen.apply_bgp(&BgpEvent::Route("192.0.2.0/24".to_string()));
        unseen.apply_bgp(&BgpEvent::Overview {
            announced: Some(false),
            origins: Vec::new(),
            registry: None,
        });
        unseen.apply_bgp(&BgpEvent::Finished);
        assert_eq!(unseen.outcome, Some(Outcome::Exhausted));
        assert_eq!(unseen.summary(), "No route observed");

        let mut private = start("192.168.1.1", TargetKind::Address);
        private.apply_bgp(&BgpEvent::Unrouted("No public route.".to_string()));
        private.apply_bgp(&BgpEvent::Finished);
        assert_eq!(private.outcome, Some(Outcome::Reached));
        assert_eq!(private.summary(), "Not publicly routed");

        let mut stopped = start("AS64500", TargetKind::Asn);
        stopped.apply_bgp(&BgpEvent::Network(64500));
        stopped.apply_bgp(&BgpEvent::Plan(4));
        stopped.apply_bgp(&BgpEvent::Stopped);
        assert_eq!(stopped.outcome, Some(Outcome::Stopped));
        assert_eq!(stopped.bgp.as_ref().unwrap().pending, 0);
        assert_eq!(stopped.summary(), "Stopped \u{00B7} No result");
        assert_eq!(stopped.own_asn(), None);
    }

    #[test]
    fn a_dns_run_nobody_answered_is_exhausted_and_a_stop_is_a_stop() {
        let mut silent = Run::new(Tool::Dns, "example.com", FamilyChoice::Auto, 0, 0);
        silent.apply_dns(&DnsEvent::Plan(1));
        let mut source =
            crate::dns::SourceResult::new(crate::dns::SourceKind::Public, "Cloudflare", "1.1.1.1");
        source.error = Some("1.1.1.1 didn't answer in time.".into());
        silent.apply_dns(&DnsEvent::Source(source));
        silent.apply_dns(&DnsEvent::Finished);
        assert_eq!(silent.outcome, Some(Outcome::Exhausted));
        assert_eq!(silent.summary(), "0 addresses \u{00B7} all agree");

        let mut stopped = Run::new(Tool::Dns, "example.com", FamilyChoice::Auto, 0, 0);
        stopped.apply_dns(&DnsEvent::Stopped);
        assert_eq!(stopped.outcome, Some(Outcome::Stopped));
        assert_eq!(stopped.summary(), "No result");
        let mut failed = Run::new(Tool::Dns, "example.com", FamilyChoice::Auto, 0, 0);
        failed.apply_dns(&DnsEvent::Failed(
            "No resolver is configured on this device.".into(),
        ));
        assert_eq!(failed.summary(), "Failed");
    }

    #[test]
    fn runs_saved_by_an_older_version_still_load() {
        let json = r#"{"id":"abc234","tool":"traceroute","target":"1.1.1.1","started_at":1,"queries":3,"max_ttl":30}"#;
        let run: Run = serde_json::from_str(json).unwrap();
        assert!(run.hops.is_empty());
        assert_eq!(run.family, FamilyChoice::Auto);
    }
}
