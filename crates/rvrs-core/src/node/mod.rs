//! What a node keeps itself, whatever it is asked (`docs/node-safety.md`).
//!
//! A node is a machine somewhere else that makes runs on request. The requests come from an API
//! the node doesn't have to trust: everything here is checked on the node, with rules the node
//! was built with and settings only its owner writes. None of them arrives in a claim, an event
//! or any other answer of the API.
//!
//! - [`request::check`] turns a request into this build's own engine options, or refuses it: a
//!   tool or an option the build doesn't know, a run over the node's limits, a target the node
//!   sends nothing to. A node never runs a command it was sent.
//! - [`address`] says which addresses a node sends packets to, and which ranges its owner
//!   allowed.
//! - [`gate::Gate`] counts runs at once and runs a minute.
//! - [`refusal::Refusal`] is the reason, and the report with the outcome `refused` that carries
//!   it back.
//! - [`setup::Setup`] says whether someone set the node up. Without a credential nothing calls
//!   the API.
//! - [`Heartbeat`] is all a node says about itself.
//!
//! `rvrs node` is built on top of this ([`Guard`]). Nothing here opens a socket.

pub mod address;
pub mod gate;
pub mod refusal;
pub mod request;
pub mod setup;

use std::net::IpAddr;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::report::Report;
use crate::resolve::ResolveError;
use crate::FamilyChoice;

use address::Allowed;
use gate::Gate;
use refusal::Refusal;
use request::{Admitted, Request};

/// The revision of the node protocol this build speaks.
pub const PROTOCOL: u32 = 1;

/// What a node allows itself, whatever it is asked. Its heartbeat says them by these names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// Runs the node makes at the same time.
    pub runs_at_once: u32,
    /// Runs the node starts in any 60 seconds.
    pub runs_per_minute: u32,
    /// Probes one run sends: the packets of a traceroute, a ping or an MTR toward the target and
    /// the routers on the way. The questions of a DNS lookup and the requests of a BGP lookup are
    /// no probes. Their number is the tool's own, a request can't raise it, and `seconds_per_run`
    /// ends those runs as it ends any.
    pub packets_per_run: u32,
    /// How long one run may take before the node ends it.
    pub seconds_per_run: u32,
    /// The largest report the node sends.
    pub report_bytes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            runs_at_once: 2,
            runs_per_minute: 12,
            packets_per_run: 1000,
            seconds_per_run: 300,
            report_bytes: 262_144,
        }
    }
}

impl Limits {
    /// The most an owner can raise each limit to. The build carries these, so no setting and no
    /// API turns a node into something that floods: at the ceiling a node sends 10,000 packets a
    /// run. The report's ceiling stays under the 2 MiB the API takes for a result.
    pub const CEILING: Limits = Limits {
        runs_at_once: 8,
        runs_per_minute: 60,
        packets_per_run: 10_000,
        seconds_per_run: 3600,
        report_bytes: 1_900_000,
    };

    /// The smallest `reportBytes` a node can be set to. A node has to be able to say how a run
    /// ended, a refusal included, and [`fit`] cuts any report down to less than this. So a
    /// limit at or above the floor is one the node always keeps. Every other limit can be 0,
    /// which allows nothing.
    pub const REPORT_BYTES_FLOOR: u32 = 4096;

    /// These limits, each no higher than its ceiling, and the report's no lower than its floor.
    pub fn within_bounds(self) -> Limits {
        Limits {
            runs_at_once: self.runs_at_once.min(Self::CEILING.runs_at_once),
            runs_per_minute: self.runs_per_minute.min(Self::CEILING.runs_per_minute),
            packets_per_run: self.packets_per_run.min(Self::CEILING.packets_per_run),
            seconds_per_run: self.seconds_per_run.min(Self::CEILING.seconds_per_run),
            report_bytes: self
                .report_bytes
                .clamp(Self::REPORT_BYTES_FLOOR, Self::CEILING.report_bytes),
        }
    }

    /// Sets one limit by the name the heartbeat says it with, as an owner writes it:
    /// `runsPerMinute=6`. A value over the ceiling or under the floor is refused, so nobody
    /// believes a node keeps a limit it doesn't.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        let (limit, floor, ceiling) = match name {
            "runsAtOnce" => (&mut self.runs_at_once, 0, Self::CEILING.runs_at_once),
            "runsPerMinute" => (
                &mut self.runs_per_minute,
                0,
                Self::CEILING.runs_per_minute,
            ),
            "packetsPerRun" => (
                &mut self.packets_per_run,
                0,
                Self::CEILING.packets_per_run,
            ),
            "secondsPerRun" => (
                &mut self.seconds_per_run,
                0,
                Self::CEILING.seconds_per_run,
            ),
            "reportBytes" => (
                &mut self.report_bytes,
                Self::REPORT_BYTES_FLOOR,
                Self::CEILING.report_bytes,
            ),
            _ => {
                return Err(format!(
                    "{name} is not a limit. The limits are runsAtOnce, runsPerMinute, packetsPerRun, secondsPerRun and reportBytes."
                ))
            }
        };
        match value.trim().parse::<u32>() {
            Ok(number) if (floor..=ceiling).contains(&number) => {
                *limit = number;
                Ok(())
            }
            _ => Err(format!(
                "{name} must be a whole number from {floor} to {ceiling}."
            )),
        }
    }
}

/// What a node says about itself, and all it says: no credential, no target and nothing from a
/// report has a field here to travel in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Heartbeat {
    /// The node's rvrs version.
    pub version: String,
    /// The revision of the protocol the node knows.
    pub protocol: u32,
    pub system: System,
    /// A random word the node makes each time it starts.
    pub boot: String,
    /// The build features the node has.
    pub features: Vec<String>,
    pub limits: Limits,
    pub outbox: Outbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct System {
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outbox {
    /// Finished runs the API hasn't answered for yet.
    pub waiting: u32,
    /// Runs given up since this start.
    pub dropped: u32,
    /// `disk` or `memory`.
    pub keeps: String,
}

/// A run the node admitted and then didn't make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stopped {
    /// The address the name resolved to is one the node sends nothing to.
    Refused(Refusal),
    /// The name didn't resolve. The run ends failed, with this sentence.
    Failed(String),
}

/// The node's own checks in one place: what it allows itself, what its build has, which ranges
/// its owner allowed and how many runs it is making.
#[derive(Debug, Clone)]
pub struct Guard {
    limits: Limits,
    features: Vec<String>,
    allowed: Allowed,
    gate: Gate,
}

impl Guard {
    /// `features` are the build features this build has. Limits over the ceiling are lowered to
    /// it, and a report limit under the floor is raised to it.
    pub fn new(limits: Limits, features: &[&str], allowed: Allowed) -> Guard {
        let limits = limits.within_bounds();
        Guard {
            limits,
            features: features.iter().map(|feature| feature.to_string()).collect(),
            allowed,
            gate: Gate::new(&limits),
        }
    }

    /// The limits as the node keeps them, which is what its heartbeat says.
    pub fn limits(&self) -> Limits {
        self.limits
    }

    pub fn features(&self) -> &[String] {
        &self.features
    }

    /// How many more runs the node starts now: the `room` of its next claim.
    pub fn room(&mut self, now: Instant) -> u32 {
        self.gate.room(now)
    }

    /// Checks a request and counts it as a run the node is making. A refused request counts as
    /// none. Every admitted run ends with [`Guard::finish`].
    pub fn admit(&mut self, request: &Request, now: Instant) -> Result<Admitted, Refusal> {
        let features: Vec<&str> = self.features.iter().map(String::as_str).collect();
        let admitted = request::check(request, &features, &self.limits, &self.allowed)?;
        self.gate.start(now)?;
        Ok(admitted)
    }

    /// The address an admitted run probes, or None for a tool that probes none. The name is
    /// resolved here, once, and the answer is checked before anything is sent. The engine is
    /// handed this address and never the name, so the address that was checked is the address
    /// that is probed, and a name that resolves to a private address the next time changes
    /// nothing about this run.
    pub fn destination<Resolve>(
        &self,
        admitted: &Admitted,
        resolve: Resolve,
    ) -> Result<Option<IpAddr>, Stopped>
    where
        Resolve: FnOnce(&str, FamilyChoice) -> Result<IpAddr, ResolveError>,
    {
        if !admitted.probes() {
            return Ok(None);
        }
        let address = resolve(&admitted.target, admitted.family())
            .map_err(|error| Stopped::Failed(error.to_string()))?;
        request::resolved(&admitted.target, address, &self.allowed)
            .map(Some)
            .map_err(Stopped::Refused)
    }

    /// An admitted run ended, however it ended.
    pub fn finish(&mut self) {
        self.gate.end();
    }

    /// Makes a report fit the node's limit. `Ok(true)` when something was left out. The guard's
    /// limit is never under the floor, so the error is one no report of this build reaches. A
    /// caller that gets it sends nothing and counts the run as dropped.
    pub fn fit(&self, report: &mut Report) -> Result<bool, Unfit> {
        fit(report, self.limits.report_bytes as usize)
    }
}

/// A report that is over the limit with everything left out that can be. It is never sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unfit {
    /// The report's size once cut down as far as it goes.
    pub bytes: usize,
    pub limit: usize,
}

impl std::fmt::Display for Unfit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "The report is {} bytes with everything left out that can be, and this node sends {} at most.",
            self.bytes, self.limit
        )
    }
}

/// The longest a text of a cut-down report may be: a tool's word, a node's name, an address.
const WORD: usize = 100;
/// The longest target a report names, which is the longest a request may carry.
const TARGET: usize = 255;
/// The longest summary of a cut-down report.
const SUMMARY: usize = 200;

/// Shortens a text to `most` bytes, at a character's edge.
fn clip(text: &mut String, most: usize) {
    if text.len() > most {
        let mut end = most;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}

fn clip_some(text: &mut Option<String>, most: usize) {
    if let Some(text) = text {
        clip(text, most);
    }
}

/// Cuts a report down to `limit` bytes of JSON, as [`Report::json`] writes it. What goes, in
/// this order, until it fits:
///
/// 1. each hop's samples,
/// 2. the run's samples,
/// 3. the path, the records, the sources and the interpretation, with a sentence in `error`
///    that says so. The figures stay, since a report states them apart from its samples,
/// 4. everything but how the run ended: the figures go, and every text is cut to a fixed
///    length. What is left is smaller than [`Limits::REPORT_BYTES_FLOOR`] whatever the report
///    held, which is why a limit can't be set under it.
///
/// `Ok(false)` is a report that fit as it was and `Ok(true)` one that fits now. `Err` is a
/// report that is over the limit even so, which only a limit under the floor can cause. It
/// must not be sent: the size is checked after the last cut, never assumed.
pub fn fit(report: &mut Report, limit: usize) -> Result<bool, Unfit> {
    let size = |report: &Report| report.json().len();
    if size(report) <= limit {
        return Ok(false);
    }
    if let Some(hops) = report.hops.as_mut() {
        for hop in hops {
            hop.samples = None;
        }
    }
    if size(report) <= limit {
        return Ok(true);
    }
    report.samples = None;
    if size(report) <= limit {
        return Ok(true);
    }
    report.hops = None;
    report.records = None;
    report.sources = None;
    report.interpretation.clear();
    if report.error.is_none() {
        report.error = Some(format!(
            "The report was over this node's limit of {limit} bytes, so its path and records were left out."
        ));
    }
    if size(report) <= limit {
        return Ok(true);
    }
    report.measured.clear();
    report.probe = None;
    report.mtu = None;
    report.sentence.clear();
    report.error = Some(format!(
        "The report was over this node's limit of {limit} bytes, so only how the run ended is left."
    ));
    clip(&mut report.id, WORD);
    clip(&mut report.tool, WORD);
    clip(&mut report.target, TARGET);
    clip(&mut report.started_at, WORD);
    clip(&mut report.outcome, WORD);
    clip(&mut report.summary, SUMMARY);
    clip_some(&mut report.address, WORD);
    clip_some(&mut report.family, WORD);
    clip(&mut report.origin.kind, WORD);
    clip_some(&mut report.origin.node, WORD);
    clip_some(&mut report.origin.node_id, WORD);
    clip_some(&mut report.origin.install, WORD);
    clip_some(&mut report.origin.source_address, WORD);
    clip_some(&mut report.origin.public_address, WORD);
    match size(report) {
        bytes if bytes <= limit => Ok(true),
        bytes => Err(Unfit { bytes, limit }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::address::Space;
    use crate::node::refusal::Identity;
    use serde_json::{json, Value};
    use std::path::Path;
    use std::time::Duration;

    const FULL: [&str; 7] = ["traceroute", "ping", "mtr", "dns", "bgp", "tcp", "mtu"];

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../schema/node-protocol/fixtures")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn request(tool: &str, target: &str, options: Value) -> Request {
        Request {
            run: "0e6d2f0a-8c0e-4f58-9a55-0b8f3f6f1c11".into(),
            attempt: 1,
            tool: tool.into(),
            target: target.into(),
            options: options.as_object().cloned().unwrap_or_default(),
        }
    }

    fn guard() -> Guard {
        Guard::new(Limits::default(), &FULL, Allowed::none())
    }

    #[test]
    fn the_limits_are_the_heartbeats() {
        let heartbeat = fixture("heartbeat.request.json");
        let limits: Limits = serde_json::from_value(heartbeat["limits"].clone()).unwrap();
        assert_eq!(limits, Limits::default());
        assert_eq!(serde_json::to_value(limits).unwrap(), heartbeat["limits"]);
    }

    #[test]
    fn a_heartbeat_has_the_fields_of_the_protocol_and_no_other() {
        let written = fixture("heartbeat.request.json");
        let heartbeat: Heartbeat = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(heartbeat.protocol, PROTOCOL);
        // Read and written back, it is the fixture: every field of the protocol is one of the
        // struct's, and the struct has no field the protocol lacks.
        assert_eq!(serde_json::to_value(&heartbeat).unwrap(), written);
    }

    #[test]
    fn an_owner_sets_a_limit_by_its_name_up_to_the_ceiling() {
        let mut limits = Limits::default();
        assert_eq!(limits.set("runsPerMinute", "6"), Ok(()));
        assert_eq!(limits.set("packetsPerRun", " 300 "), Ok(()));
        assert_eq!(limits.runs_per_minute, 6);
        assert_eq!(limits.packets_per_run, 300);
        // 0 turns the node's runs off without taking it offline.
        assert_eq!(limits.set("runsAtOnce", "0"), Ok(()));
        assert_eq!(
            limits.set("packetsPerRun", "10001"),
            Err("packetsPerRun must be a whole number from 0 to 10000.".to_string())
        );
        assert_eq!(
            limits.set("secondsPerRun", "-1"),
            Err("secondsPerRun must be a whole number from 0 to 3600.".to_string())
        );
        // A node always has to be able to say how a run ended, so its report has a floor.
        for under in ["0", "1", "4095"] {
            assert_eq!(
                limits.set("reportBytes", under),
                Err("reportBytes must be a whole number from 4096 to 1900000.".to_string()),
                "{under}"
            );
        }
        assert_eq!(limits.report_bytes, Limits::default().report_bytes);
        assert_eq!(limits.set("reportBytes", "4096"), Ok(()));
        assert!(limits.set("packetsPerSecond", "5").is_err());
        assert_eq!(limits.packets_per_run, 300);
    }

    #[test]
    fn limits_over_the_ceiling_are_lowered_to_it() {
        let asked = Limits {
            runs_at_once: 500,
            runs_per_minute: 6000,
            packets_per_run: u32::MAX,
            seconds_per_run: 86_400,
            report_bytes: u32::MAX,
        };
        assert_eq!(
            Guard::new(asked, &FULL, Allowed::none()).limits(),
            Limits::CEILING
        );
        assert_eq!(Limits::default().within_bounds(), Limits::default());
        // A report limit under the floor is raised to it. Nothing else has a floor.
        let nothing = Limits {
            runs_at_once: 0,
            runs_per_minute: 0,
            packets_per_run: 0,
            seconds_per_run: 0,
            report_bytes: 0,
        };
        assert_eq!(
            Guard::new(nothing, &FULL, Allowed::none()).limits(),
            Limits {
                report_bytes: Limits::REPORT_BYTES_FLOOR,
                ..nothing
            }
        );
    }

    #[test]
    fn a_refusal_for_a_tool_the_build_lacks_is_the_protocols_example() {
        // The fixture is the report a node without iperf3 sends. What the guard writes for the
        // same request is that report, key for key.
        let written = fixture("result.refused.request.json");
        let report = &written["report"];
        let asked = Request {
            run: written["claim"]["run"].as_str().unwrap().into(),
            attempt: 1,
            tool: "iperf3".into(),
            target: report["target"].as_str().unwrap().into(),
            options: serde_json::Map::new(),
        };
        let mut guard = guard();
        let refusal = guard.admit(&asked, Instant::now()).unwrap_err();
        assert_eq!(
            refusal,
            Refusal::Tool {
                tool: "iperf3".into()
            }
        );
        let node = Identity {
            name: "fra-1",
            id: report["origin"]["nodeId"].as_str(),
            version: "0.14.2",
        };
        let started = crate::format::seconds_of(report["startedAt"].as_str().unwrap()).unwrap();
        let made = refusal.report(&asked, &node, report["id"].as_str().unwrap(), started);
        let mut made = serde_json::to_value(&made).unwrap();
        // The fixture leaves out what a refused run has none of.
        fn without_nulls(value: &mut Value) {
            if let Some(object) = value.as_object_mut() {
                object.retain(|_, value| !value.is_null());
                object.values_mut().for_each(without_nulls);
            }
        }
        without_nulls(&mut made);
        assert_eq!(&made, report);
    }

    #[test]
    fn a_refused_request_counts_as_no_run() {
        let now = Instant::now();
        let mut guard = guard();
        for _ in 0..50 {
            assert!(guard
                .admit(&request("ping", "10.0.0.1", json!({})), now)
                .is_err());
        }
        assert_eq!(guard.room(now), 2);
        assert!(guard
            .admit(&request("ping", "1.1.1.1", json!({})), now)
            .is_ok());
        assert_eq!(guard.room(now), 1);
    }

    #[test]
    fn more_runs_than_the_node_allows_are_refused() {
        let now = Instant::now();
        let mut guard = guard();
        let ping = request("ping", "example.com", json!({}));
        assert!(guard.admit(&ping, now).is_ok());
        assert!(guard.admit(&ping, now).is_ok());
        assert_eq!(guard.room(now), 0);
        assert_eq!(guard.admit(&ping, now), Err(Refusal::AtOnce { limit: 2 }));
        guard.finish();
        guard.finish();
        // Twelve starts a minute, however quickly each run ends.
        for _ in 0..10 {
            assert!(guard.admit(&ping, now).is_ok());
            guard.finish();
        }
        assert_eq!(
            guard.admit(&ping, now),
            Err(Refusal::PerMinute { limit: 12 })
        );
        let later = now + Duration::from_secs(61);
        assert_eq!(guard.room(later), 2);
        assert!(guard.admit(&ping, later).is_ok());
    }

    #[test]
    fn a_name_that_rebinds_is_refused() {
        // The name answers with a public address the first time it is asked and with a private
        // one the second time, which is how DNS rebinding gets a checker to wave a name through.
        let answers = std::cell::RefCell::new(vec!["10.0.0.5", "93.184.216.34"]);
        let resolve = |_: &str, _: FamilyChoice| -> Result<IpAddr, ResolveError> {
            Ok(answers.borrow_mut().pop().unwrap().parse().unwrap())
        };
        let now = Instant::now();
        let mut guard = guard();
        let asked = request("mtr", "rebind.example.net", json!({}));

        // The first run gets the public address, and that address is what it probes: the engine
        // is given the address and resolves nothing itself.
        let first = guard.admit(&asked, now).unwrap();
        assert_eq!(
            guard.destination(&first, resolve),
            Ok(Some("93.184.216.34".parse().unwrap()))
        );
        guard.finish();

        // The second run resolves again, gets the private address and is refused.
        let second = guard.admit(&asked, now).unwrap();
        assert_eq!(
            guard.destination(&second, resolve),
            Err(Stopped::Refused(Refusal::Resolved {
                name: "rebind.example.net".into(),
                space: Space::Private,
            }))
        );
    }

    #[test]
    fn a_name_that_resolves_to_the_node_itself_is_refused() {
        let mut guard = Guard::new(
            Limits::default(),
            &FULL,
            Allowed::parse("0.0.0.0/0, ::/0").unwrap(),
        );
        let admitted = guard
            .admit(
                &request("traceroute", "metadata.example.net", json!({})),
                Instant::now(),
            )
            .unwrap();
        for (address, space) in [
            ("127.0.0.1", Space::Loopback),
            ("169.254.169.254", Space::LinkLocal),
            ("::1", Space::Loopback),
        ] {
            assert_eq!(
                guard.destination(&admitted, |_, _| Ok(address.parse().unwrap())),
                Err(Stopped::Refused(Refusal::Resolved {
                    name: "metadata.example.net".into(),
                    space,
                })),
                "{address}"
            );
        }
    }

    #[test]
    fn a_name_that_does_not_resolve_fails_and_is_no_refusal() {
        let mut guard = guard();
        let admitted = guard
            .admit(
                &request("ping", "nope.example.net", json!({})),
                Instant::now(),
            )
            .unwrap();
        assert_eq!(
            guard.destination(&admitted, |host, _| Err(ResolveError::NotFound(
                host.into()
            ))),
            Err(Stopped::Failed(
                "Couldn't find \u{201C}nope.example.net\u{201D}.".into()
            ))
        );
    }

    #[test]
    fn the_family_that_was_asked_for_is_the_family_resolved_in() {
        let mut guard = guard();
        let admitted = guard
            .admit(
                &request("ping", "example.com", json!({ "family": "IPv6" })),
                Instant::now(),
            )
            .unwrap();
        let destination = guard.destination(&admitted, |host, family| {
            assert_eq!(host, "example.com");
            assert_eq!(family, FamilyChoice::V6);
            Ok("2606:4700:4700::1111".parse().unwrap())
        });
        assert_eq!(
            destination,
            Ok(Some("2606:4700:4700::1111".parse().unwrap()))
        );
    }

    #[test]
    fn a_tool_that_probes_nothing_resolves_nothing() {
        let mut guard = guard();
        for (tool, target) in [("dns", "example.com"), ("bgp", "AS13335")] {
            let admitted = guard
                .admit(&request(tool, target, json!({})), Instant::now())
                .unwrap();
            let destination = guard.destination(&admitted, |_, _| {
                panic!("{tool} has no destination to resolve")
            });
            assert_eq!(destination, Ok(None));
            guard.finish();
        }
    }

    fn long_report() -> Report {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/fixtures/mtr.json");
        let mut report: Report =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let samples: Vec<Option<f64>> = (0..600).map(|n| Some(f64::from(n) / 10.0)).collect();
        for hop in report.hops.as_mut().unwrap() {
            hop.samples = Some(samples[..120].to_vec());
        }
        report.samples = Some(samples);
        report
    }

    #[test]
    fn a_report_over_the_limit_loses_its_samples_first() {
        let whole = long_report();
        let size = whole.json().len();

        let mut fits = whole.clone();
        assert_eq!(fit(&mut fits, size), Ok(false));
        assert_eq!(fits, whole);

        // One byte too many: the hops' samples go, and nothing else.
        let mut report = whole.clone();
        assert_eq!(fit(&mut report, size - 1), Ok(true));
        assert!(report
            .hops
            .as_ref()
            .unwrap()
            .iter()
            .all(|hop| hop.samples.is_none()));
        assert_eq!(report.samples, whole.samples);
        assert_eq!(report.measured, whole.measured);
        let without_hop_samples = report.json().len();

        let mut report = whole.clone();
        assert_eq!(fit(&mut report, without_hop_samples - 1), Ok(true));
        assert_eq!(report.samples, None);
        assert_eq!(
            report.hops.as_ref().map(Vec::len),
            whole.hops.as_ref().map(Vec::len)
        );
        let without_samples = report.json().len();

        // Smaller still: the path goes and the report says so. The figures stay.
        let mut report = whole.clone();
        assert_eq!(fit(&mut report, without_samples - 1), Ok(true));
        assert_eq!(report.hops, None);
        assert_eq!(report.measured, whole.measured);
        assert_eq!(report.outcome, whole.outcome);
        assert_eq!(
            report.error,
            Some(format!(
                "The report was over this node's limit of {} bytes, so its path and records were left out.",
                without_samples - 1
            ))
        );
        assert!(report.json().len() < without_samples);
    }

    #[test]
    fn the_guard_fits_a_report_to_its_own_limit() {
        let limits = Limits {
            report_bytes: 4096,
            ..Limits::default()
        };
        let guard = Guard::new(limits, &FULL, Allowed::none());
        let mut report = long_report();
        assert_eq!(guard.fit(&mut report), Ok(true));
        assert!(report.json().len() <= 4096);
    }

    /// A report with every text and every list far larger than any limit.
    fn enormous_report() -> Report {
        let long = |letter: &str| letter.repeat(20_000);
        let mut report = long_report();
        report.id = long("i");
        report.tool = long("t");
        report.target = long("\u{00FC}");
        report.started_at = long("2");
        report.outcome = long("o");
        report.summary = long("s");
        report.sentence = long("n");
        report.error = Some(long("e"));
        report.address = Some(long("a"));
        report.family = Some(long("f"));
        report.origin.kind = long("k");
        report.origin.node = Some(long("d"));
        report.origin.node_id = Some(long("u"));
        report.origin.install = Some(long("l"));
        report.origin.source_address = Some(long("1"));
        report.origin.public_address = Some(long("2"));
        report.interpretation = vec![long("p"); 20];
        report.measured = (0..500)
            .map(|index| crate::report::Measurement {
                name: format!("figure {index} {}", "m".repeat(1000)),
                value: f64::from(index),
                unit: Some("u".repeat(1000)),
            })
            .collect();
        report
    }

    #[test]
    fn a_report_over_the_limit_with_its_path_gone_keeps_only_how_the_run_ended() {
        // Enormous figures and interpretation: the third cut is not enough.
        let mut report = long_report();
        let outcome = report.outcome.clone();
        report.interpretation = vec!["p".repeat(5000); 4];
        report.measured = (0..2000)
            .map(|index| crate::report::Measurement {
                name: format!("figure {index}"),
                value: f64::from(index),
                unit: Some("ms".into()),
            })
            .collect();
        assert_eq!(fit(&mut report, 8192), Ok(true));
        assert!(report.json().len() <= 8192);
        assert!(report.measured.is_empty());
        assert_eq!(report.outcome, outcome);
        assert_eq!(
            report.error.as_deref(),
            Some("The report was over this node's limit of 8192 bytes, so only how the run ended is left.")
        );
    }

    #[test]
    fn whatever_a_report_holds_it_is_cut_to_less_than_the_floor() {
        let floor = Limits::REPORT_BYTES_FLOOR as usize;
        let mut report = enormous_report();
        assert!(report.json().len() > 1_000_000);
        assert_eq!(fit(&mut report, floor), Ok(true));
        let size = report.json().len();
        assert!(size <= floor, "{size}");
        // A text is cut at a character's edge, so what is left is still a report to read.
        assert!(report.target.chars().all(|letter| letter == '\u{00FC}'));
        assert!(report.target.len() <= 255);
        let read: Report = serde_json::from_str(&report.json()).unwrap();
        assert_eq!(read, report);
        // The guard's limit is never under the floor, so the guard always has a report to send.
        let nothing = Limits {
            report_bytes: 0,
            ..Limits::default()
        };
        let guard = Guard::new(nothing, &FULL, Allowed::none());
        let mut report = enormous_report();
        assert_eq!(guard.fit(&mut report), Ok(true));
        assert!(report.json().len() <= floor);
    }

    #[test]
    fn a_report_that_can_not_fit_is_an_error_and_never_a_report_to_send() {
        // No report is 0 bytes, so a limit of 0 can't be kept. The answer says so.
        let mut report = long_report();
        let unfit = fit(&mut report, 0).unwrap_err();
        assert_eq!(unfit.limit, 0);
        assert_eq!(unfit.bytes, report.json().len());
        assert!(unfit.bytes > 0);
        assert_eq!(
            unfit.to_string(),
            format!(
                "The report is {} bytes with everything left out that can be, and this node sends 0 at most.",
                unfit.bytes
            )
        );
        // The same for any limit under what is left of a report once everything is cut.
        let mut small = enormous_report();
        let unfit = fit(&mut small, 256).unwrap_err();
        assert!(unfit.bytes > 256 && unfit.bytes < Limits::REPORT_BYTES_FLOOR as usize);
        // A refusal is the smallest report a node sends, and it fits the floor as it is.
        let refusal = Refusal::Tool {
            tool: "iperf3".into(),
        };
        let node = Identity {
            name: "fra-1",
            id: Some("9c2f1a7e-5b3d-4e8f-8a6c-1d2e3f4a5b6c"),
            version: "0.14.2",
        };
        let asked = request("iperf3", &"a".repeat(255), json!({}));
        let mut report = refusal.report(&asked, &node, "r8w2kc", 0);
        assert_eq!(
            fit(&mut report, Limits::REPORT_BYTES_FLOOR as usize),
            Ok(false)
        );
    }
}
