//! The BGP tool's engine. It turns a hostname, an address, a prefix or an AS number into the
//! route, its origin validation, the paths collectors see, its history and the network behind
//! it, each part sent as its data call answers. Read-only: the facts come from RIPEstat (RIPE
//! RIS collectors, a Routinator validator, the registries' statistics) and from the user's
//! looking glasses, and every part reports its source and the time it describes. One failed
//! call leaves its part empty and is listed as failed. The lookup itself fails only when the
//! route cannot be found at all.

use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Mutex;
use std::thread;

use serde::de::DeserializeOwned;

use super::glass::{self, GlassKind, GlassResult, GlassSessions, LookingGlass};
use super::result::{
    Activity, Address, BgpNetwork, BgpResult, Neighbor, Origin, Path, RelatedPrefix, Roa,
    RpkiStatus, Share, Side, Source, Span, Target, TargetKind, Validation,
};
use super::ripestat::{self, Answer};
use crate::asn::clean_holder;
use crate::{format, is_private};

/// What every collector peer's route folds into, counted over all of them and not only the
/// paths that are kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folded {
    /// Distinct AS paths.
    pub paths: usize,
    pub peers: usize,
    pub collectors: usize,
    pub upstreams: Vec<Share>,
    pub shortest: usize,
    pub typical: usize,
    pub direct: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPart {
    Identity {
        holder: Option<String>,
        announced: Option<bool>,
        block: Option<String>,
    },
    Registry(String),
    Country(String),
    Neighbors {
        list: Vec<Neighbor>,
        upstream: Option<u64>,
        downstream: Option<u64>,
        uncertain: Option<u64>,
    },
    Prefixes {
        list: Vec<String>,
        v4: Option<u64>,
        v6: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum BgpEvent {
    /// How many more sources the lookup is about to ask. Negative when a part falls away.
    Plan(i32),
    Addresses(Vec<Address>, Option<String>),
    Unrouted(String),
    Route(String),
    Overview {
        announced: Option<bool>,
        origins: Vec<Origin>,
        registry: Option<String>,
    },
    Status {
        seeing: Option<u64>,
        total: Option<u64>,
        first_seen: Option<u64>,
        related: Vec<RelatedPrefix>,
        route_objects: BTreeMap<u32, Vec<String>>,
    },
    Validation(Validation),
    Paths(Vec<Path>, Folded),
    Names(BTreeMap<u32, String>),
    History(Vec<Span>),
    Activity(Vec<Activity>),
    Network(u32),
    NetworkPart(NetworkPart),
    PrefixValidation(Validation),
    Glass(GlassResult),
    /// The origin AS's sessions with one looking glass's route servers, by its host.
    GlassSessions(String, GlassSessions),
    Source(Source),
    Finished,
    Stopped,
    Failed(String),
}

struct Lookup<'a> {
    events: &'a Sender<BgpEvent>,
    stop: &'a AtomicBool,
    agent: ureq::Agent,
    glasses: &'a [LookingGlass],
}

impl Lookup<'_> {
    fn emit(&self, event: BgpEvent) {
        let _ = self.events.send(event);
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn get<Payload: DeserializeOwned>(
        &self,
        call: &str,
        query: &[(&str, &str)],
        timeout: u64,
    ) -> Option<Answer<Payload>> {
        if self.stopped() {
            return None;
        }
        ripestat::get(&self.agent, call, query, timeout)
    }

    fn source(
        &self,
        call: &str,
        title: &str,
        basis: &str,
        observed_at: Option<u64>,
        retrieved_at: Option<u64>,
        failed: bool,
    ) {
        self.emit(BgpEvent::Source(Source {
            call: call.to_string(),
            provider: None,
            title: title.to_string(),
            basis: basis.to_string(),
            observed_at,
            retrieved_at: retrieved_at.unwrap_or_else(format::now),
            failed,
        }));
    }
}

fn observed(time: &Option<String>) -> Option<u64> {
    time.as_deref().and_then(ripestat::date)
}

/// Runs one lookup to its end. `glasses` are the user's looking glasses. Each is asked for the
/// route's prefix. The last event is `Finished`, `Stopped` or `Failed`.
pub fn run(
    target: &Target,
    glasses: &[LookingGlass],
    events: &Sender<BgpEvent>,
    stop: &AtomicBool,
) {
    let lookup = Lookup {
        events,
        stop,
        agent: ripestat::agent(),
        glasses,
    };
    let outcome = lookup.run(target);
    lookup.emit(if lookup.stopped() {
        BgpEvent::Stopped
    } else {
        match outcome {
            Ok(()) => BgpEvent::Finished,
            Err(message) => BgpEvent::Failed(message),
        }
    });
}

const UNREACHABLE: &str = "RIPEstat didn't answer. Check the connection and run it again.";

impl Lookup<'_> {
    fn run(&self, target: &Target) -> Result<(), String> {
        match target.kind {
            TargetKind::Asn => {
                let Some(asn) = target.asn() else {
                    return Ok(());
                };
                self.emit(BgpEvent::Plan(4));
                self.emit(BgpEvent::Network(asn));
                self.network(asn, None);
                Ok(())
            }
            TargetKind::Prefix => self.route(&target.canonical, &[]),
            TargetKind::Address => self.address(&target.canonical),
            TargetKind::Host => {
                let addresses = resolve_all(&target.canonical);
                if addresses.is_empty() {
                    return Err(format!(
                        "Couldn't find \u{201C}{}\u{201D}.",
                        target.canonical
                    ));
                }
                // Every address is placed in its route, so a name served from two networks
                // shows both.
                let placed: Vec<Address> = thread::scope(|scope| {
                    let handles: Vec<_> = addresses
                        .iter()
                        .take(8)
                        .map(|address| {
                            scope.spawn(move || {
                                let text = address.to_string();
                                let mut entry = Address {
                                    address: text.clone(),
                                    prefix: None,
                                    asn: None,
                                };
                                if !is_private(*address) {
                                    if let Some(info) = self.get::<ripestat::NetworkInfo>(
                                        "network-info",
                                        &[("resource", &text)],
                                        15,
                                    ) {
                                        entry.prefix = info.data.prefix;
                                        entry.asn = info.data.asns.first().map(|asn| asn.0);
                                    }
                                }
                                entry
                            })
                        })
                        .collect();
                    handles
                        .into_iter()
                        .filter_map(|handle| handle.join().ok())
                        .collect()
                });
                let Some(selected) = placed
                    .iter()
                    .find(|entry| entry.prefix.is_some())
                    .or(placed.first())
                    .cloned()
                else {
                    return Err(UNREACHABLE.to_string());
                };
                self.emit(BgpEvent::Addresses(placed, Some(selected.address.clone())));
                match &selected.prefix {
                    Some(prefix) => {
                        let known: Vec<u32> = selected.asn.into_iter().collect();
                        self.route(prefix, &known)
                    }
                    None => self.address(&selected.address),
                }
            }
        }
    }

    fn address(&self, text: &str) -> Result<(), String> {
        if text.parse::<IpAddr>().is_ok_and(is_private) {
            self.emit(BgpEvent::Unrouted(format!(
                "{text} is a private or reserved address, so it has no public route."
            )));
            return Ok(());
        }
        let alone = |prefix: Option<String>, asn: Option<u32>| {
            BgpEvent::Addresses(
                vec![Address {
                    address: text.to_string(),
                    prefix,
                    asn,
                }],
                Some(text.to_string()),
            )
        };
        self.emit(alone(None, None));
        self.emit(BgpEvent::Plan(1));
        let Some(info) =
            self.get::<ripestat::NetworkInfo>("network-info", &[("resource", text)], 15)
        else {
            return Err(UNREACHABLE.to_string());
        };
        self.source(
            "network-info",
            "Covering route",
            "RIPE RIS route collectors, 8-hour snapshots",
            None,
            info.time,
            false,
        );
        let Some(prefix) = info.data.prefix else {
            self.emit(BgpEvent::Overview {
                announced: Some(false),
                origins: Vec::new(),
                registry: None,
            });
            return Ok(());
        };
        let origins: Vec<u32> = info.data.asns.iter().map(|asn| asn.0).collect();
        self.emit(alone(Some(prefix.clone()), origins.first().copied()));
        self.route(&prefix, &origins)
    }

    /// Everything about one prefix. The parts run side by side. The origin AS's network follows
    /// the overview, since it needs the origin AS.
    fn route(&self, prefix: &str, known_origins: &[u32]) -> Result<(), String> {
        self.emit(BgpEvent::Route(prefix.to_string()));
        self.emit(BgpEvent::Plan(10 + self.glasses.len() as i32));
        let reached = thread::scope(|scope| {
            let parts = [
                scope.spawn(|| self.overview(prefix, known_origins)),
                scope.spawn(|| self.status(prefix)),
                scope.spawn(|| self.paths(prefix)),
                scope.spawn(|| self.history(prefix)),
                scope.spawn(|| self.activity(prefix)),
            ];
            for glass in self.glasses {
                scope.spawn(move || {
                    if self.stopped() {
                        return;
                    }
                    let result = glass::routes(prefix, glass);
                    let basis = if glass.kind == GlassKind::AliceLg {
                        "Route servers of this looking glass".to_string()
                    } else {
                        format!("The operator's routers, {} asked", glass.routers.len())
                    };
                    let source = Source {
                        call: glass.host.clone(),
                        provider: Some(glass.kind.title().to_string()),
                        title: glass.name.clone(),
                        basis,
                        observed_at: result.observed_at,
                        retrieved_at: format::now(),
                        failed: result.failed,
                    };
                    // A looking glass adds a view. It does not decide whether the lookup worked.
                    self.emit(BgpEvent::Glass(result));
                    self.emit(BgpEvent::Source(source));
                });
            }
            // Every part is waited for, whichever answered first.
            let answered: Vec<bool> = parts
                .into_iter()
                .map(|part| part.join().unwrap_or(false))
                .collect();
            answered.contains(&true)
        });
        if !reached && !self.stopped() {
            return Err(UNREACHABLE.to_string());
        }
        Ok(())
    }

    fn overview(&self, prefix: &str, known_origins: &[u32]) -> bool {
        let answer =
            self.get::<ripestat::PrefixOverview>("prefix-overview", &[("resource", prefix)], 15);
        self.source(
            "prefix-overview",
            "Origin AS and holder",
            "RIPE RIS route collectors",
            answer
                .as_ref()
                .and_then(|answer| observed(&answer.data.query_time)),
            answer.as_ref().and_then(|answer| answer.time),
            answer.is_none(),
        );
        let mut origins: Vec<Origin> = answer
            .as_ref()
            .map(|answer| {
                answer
                    .data
                    .asns
                    .iter()
                    .map(|entry| Origin {
                        asn: entry.asn.0,
                        holder: entry.holder.as_deref().map(clean_holder),
                        route_objects: Vec::new(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if origins.is_empty() {
            origins = known_origins
                .iter()
                .map(|asn| Origin {
                    asn: *asn,
                    holder: None,
                    route_objects: Vec::new(),
                })
                .collect();
        }
        let registry = answer
            .as_ref()
            .and_then(|answer| answer.data.block.as_ref())
            .and_then(|block| block.desc.as_deref())
            .and_then(registry_name);
        let announced = answer
            .as_ref()
            .and_then(|answer| answer.data.announced)
            .or((!origins.is_empty()).then_some(true));
        self.emit(BgpEvent::Overview {
            announced,
            origins: origins.clone(),
            registry: registry.map(str::to_string),
        });

        // RPKI is a property of a prefix and one origin AS, so every origin AS gets its own
        // check. The looking glasses are asked about the origin AS meanwhile: whether it is on
        // their route servers at all says why a route is or isn't there.
        thread::scope(|scope| {
            for origin in origins.iter().take(4) {
                scope.spawn(move || {
                    if !self.stopped() {
                        self.emit(BgpEvent::Validation(validate(
                            &self.agent,
                            prefix,
                            origin.asn,
                        )));
                    }
                });
            }
            if let Some(origin) = origins.first() {
                let exchanges = self
                    .glasses
                    .iter()
                    .filter(|glass| glass.kind == GlassKind::AliceLg);
                for glass in exchanges {
                    scope.spawn(move || {
                        if self.stopped() {
                            return;
                        }
                        if let Some(sessions) = glass::sessions(origin.asn, glass) {
                            self.emit(BgpEvent::GlassSessions(glass.host.clone(), sessions));
                        }
                    });
                }
            }
        });
        self.source(
            "rpki-validation",
            "RPKI origin validation",
            "Routinator validator, all five trust anchors",
            None,
            None,
            origins.is_empty(),
        );

        match origins.first() {
            Some(first) => {
                self.emit(BgpEvent::Network(first.asn));
                self.network(first.asn, Some(prefix));
            }
            None => self.emit(BgpEvent::Plan(-4)),
        }
        answer.is_some()
    }

    fn status(&self, prefix: &str) -> bool {
        let answer =
            self.get::<ripestat::RoutingStatus>("routing-status", &[("resource", prefix)], 15);
        self.source(
            "routing-status",
            "Visibility and first seen",
            "RIPE RIS route collectors, 8-hour snapshots",
            answer
                .as_ref()
                .and_then(|answer| observed(&answer.data.query_time)),
            answer.as_ref().and_then(|answer| answer.time),
            answer.is_none(),
        );
        let Some(Answer { data, .. }) = answer else {
            return false;
        };
        let family = data.visibility.and_then(|visibility| {
            if prefix.contains(':') {
                visibility.v6
            } else {
                visibility.v4
            }
        });
        let related = |list: Option<Vec<ripestat::Specific>>, is_more_specific| {
            list.unwrap_or_default()
                .into_iter()
                .map(move |entry| RelatedPrefix {
                    prefix: entry.prefix,
                    origin: entry.origin.map(|origin| origin.0),
                    is_more_specific,
                })
        };
        let related: Vec<RelatedPrefix> = related(data.more_specifics, true)
            .chain(related(data.less_specifics, false))
            .take(40)
            .collect();
        let route_objects = data
            .origins
            .unwrap_or_default()
            .into_iter()
            .map(|origin| (origin.origin.0, origin.route_objects.unwrap_or_default()))
            .collect();
        self.emit(BgpEvent::Status {
            seeing: family.as_ref().map(|family| family.ris_peers_seeing),
            total: family.as_ref().map(|family| family.total_ris_peers),
            first_seen: data
                .first_seen
                .and_then(|seen| seen.time)
                .as_deref()
                .and_then(ripestat::date),
            related,
            route_objects,
        });
        true
    }

    fn paths(&self, prefix: &str) -> bool {
        let answer =
            self.get::<ripestat::LookingGlass>("looking-glass", &[("resource", prefix)], 20);
        self.source(
            "looking-glass",
            "AS paths",
            "RIPE RIS route collectors, their peers' current routes",
            answer
                .as_ref()
                .and_then(|answer| observed(&answer.data.latest_time)),
            answer.as_ref().and_then(|answer| answer.time),
            answer.is_none(),
        );
        let Some(answer) = answer else {
            return false;
        };
        let (paths, folded) = fold(&answer.data.rrcs, prefix);
        // Names for the networks in the paths people will read first.
        let mut wanted: Vec<u32> = Vec::new();
        let read_first = paths
            .iter()
            .take(12)
            .flat_map(|path| path.asns.iter().copied())
            .chain(folded.upstreams.iter().take(8).map(|share| share.asn));
        for asn in read_first {
            if !wanted.contains(&asn) {
                wanted.push(asn);
            }
        }
        wanted.truncate(40);
        self.emit(BgpEvent::Paths(
            paths.into_iter().take(BgpResult::PATH_LIMIT).collect(),
            folded,
        ));
        self.emit(BgpEvent::Names(self.names(&wanted)));
        true
    }

    fn history(&self, prefix: &str) -> bool {
        let start = ripestat::stamp(format::now().saturating_sub(super::result::Change::WINDOW));
        let answer = self.get::<ripestat::RoutingHistory>(
            "routing-history",
            &[
                ("resource", prefix),
                ("starttime", &start),
                ("min_peers", "10"),
            ],
            20,
        );
        self.source(
            "routing-history",
            "Origin AS history, 90 days",
            "RIPE RIS route collectors, full-table peers",
            answer
                .as_ref()
                .and_then(|answer| observed(&answer.data.query_endtime)),
            answer.as_ref().and_then(|answer| answer.time),
            answer.is_none(),
        );
        let Some(answer) = answer else {
            return false;
        };
        let mut spans = Vec::new();
        for origin in &answer.data.by_origin {
            for entry in origin
                .prefixes
                .iter()
                .filter(|entry| entry.prefix == prefix)
            {
                for timeline in &entry.timelines {
                    let (Some(start), Some(end)) = (
                        ripestat::date(&timeline.starttime),
                        ripestat::date(&timeline.endtime),
                    ) else {
                        continue;
                    };
                    spans.push(Span {
                        origin: origin.origin.0,
                        prefix: entry.prefix.clone(),
                        start,
                        end,
                        peers: timeline.full_peers_seeing.unwrap_or(0.0),
                    });
                }
            }
        }
        spans.sort_by_key(|span| span.start);
        self.emit(BgpEvent::History(spans));
        true
    }

    fn activity(&self, prefix: &str) -> bool {
        let start = ripestat::stamp(format::now().saturating_sub(7 * 86_400));
        let answer = self.get::<ripestat::UpdateActivity>(
            "bgp-update-activity",
            &[("resource", prefix), ("starttime", &start)],
            15,
        );
        self.source(
            "bgp-update-activity",
            "Update activity, 7 days",
            "RIPE RIS route collectors, announcements and withdrawals",
            answer
                .as_ref()
                .and_then(|answer| observed(&answer.data.query_endtime)),
            answer.as_ref().and_then(|answer| answer.time),
            answer.is_none(),
        );
        let Some(answer) = answer else {
            return false;
        };
        self.emit(BgpEvent::Activity(
            answer
                .data
                .updates
                .iter()
                .filter_map(|bucket| {
                    Some(Activity {
                        start: ripestat::date(&bucket.starttime)?,
                        announcements: bucket.announcements.unwrap_or(0),
                        withdrawals: bucket.withdrawals.unwrap_or(0),
                    })
                })
                .collect(),
        ));
        true
    }

    /// The network behind an AS number: who holds it, which registry, what it originates, who
    /// it is seen next to, and origin validation for the first of its prefixes.
    fn network(&self, asn: u32, leading: Option<&str>) {
        let resource = format!("AS{asn}");
        let resource = resource.as_str();
        thread::scope(|scope| {
            scope.spawn(move || {
                let answer =
                    self.get::<ripestat::AsOverview>("as-overview", &[("resource", resource)], 15);
                self.source(
                    "as-overview",
                    "Network holder",
                    "Registry records and RIPE RIS",
                    answer
                        .as_ref()
                        .and_then(|answer| observed(&answer.data.query_endtime)),
                    answer.as_ref().and_then(|answer| answer.time),
                    answer.is_none(),
                );
                if let Some(Answer { data, .. }) = answer {
                    self.emit(BgpEvent::NetworkPart(NetworkPart::Identity {
                        holder: data.holder.as_deref().map(clean_holder),
                        announced: data.announced,
                        block: data.block.and_then(|block| block.desc),
                    }));
                }
            });
            scope.spawn(move || {
                let registry = self.get::<ripestat::Registry>("rir", &[("resource", resource)], 15);
                let country = self.get::<ripestat::Country>(
                    "rir-stats-country",
                    &[("resource", resource)],
                    15,
                );
                self.source(
                    "rir",
                    "Registry and country",
                    "Regional registries' published statistics",
                    None,
                    registry.as_ref().and_then(|answer| answer.time),
                    registry.is_none() && country.is_none(),
                );
                if let Some(entry) = registry.and_then(|answer| answer.data.rirs.into_iter().next())
                {
                    self.emit(BgpEvent::NetworkPart(NetworkPart::Registry(entry.rir)));
                }
                let code = country
                    .and_then(|answer| answer.data.located_resources.into_iter().next())
                    .and_then(|located| located.location)
                    .filter(|code| !code.is_empty());
                if let Some(code) = code {
                    self.emit(BgpEvent::NetworkPart(NetworkPart::Country(code)));
                }
            });
            scope.spawn(move || {
                let answer = self.get::<ripestat::Neighbours>(
                    "asn-neighbours",
                    &[("resource", resource)],
                    15,
                );
                self.source(
                    "asn-neighbours",
                    "Observed neighbors",
                    "RIPE RIS route collectors, adjacency in AS paths",
                    answer
                        .as_ref()
                        .and_then(|answer| observed(&answer.data.latest_time)),
                    answer.as_ref().and_then(|answer| answer.time),
                    answer.is_none(),
                );
                let Some(Answer { data, .. }) = answer else {
                    return;
                };
                let all: Vec<Neighbor> = data
                    .neighbours
                    .iter()
                    .map(|neighbour| Neighbor {
                        asn: neighbour.asn.0,
                        side: Side::from_provider(&neighbour.side),
                        power: neighbour.power.unwrap_or(0),
                    })
                    .collect();
                // The strongest of each side, so no side is crowded out by another.
                let mut kept: Vec<Neighbor> = Vec::new();
                for side in Side::ALL {
                    let mut of_side: Vec<Neighbor> = all
                        .iter()
                        .filter(|neighbor| neighbor.side == side)
                        .cloned()
                        .collect();
                    of_side.sort_by_key(|neighbor| std::cmp::Reverse(neighbor.power));
                    kept.extend(of_side.into_iter().take(BgpNetwork::NEIGHBOR_LIMIT));
                }
                let mut strongest = kept.clone();
                strongest.sort_by_key(|neighbor| std::cmp::Reverse(neighbor.power));
                let wanted: Vec<u32> = strongest
                    .iter()
                    .take(36)
                    .map(|neighbor| neighbor.asn)
                    .collect();
                let counts = data.neighbour_counts;
                self.emit(BgpEvent::NetworkPart(NetworkPart::Neighbors {
                    list: kept,
                    upstream: counts.as_ref().and_then(|counts| counts.left),
                    downstream: counts.as_ref().and_then(|counts| counts.right),
                    uncertain: counts.as_ref().and_then(|counts| counts.uncertain),
                }));
                self.emit(BgpEvent::Names(self.names(&wanted)));
            });
            scope.spawn(move || {
                let answer = self.get::<ripestat::RisPrefixes>(
                    "ris-prefixes",
                    &[("resource", resource), ("list_prefixes", "true")],
                    20,
                );
                self.source(
                    "ris-prefixes",
                    "Originated prefixes",
                    "RIPE RIS route collectors, 8-hour snapshots",
                    answer
                        .as_ref()
                        .and_then(|answer| observed(&answer.data.query_time)),
                    answer.as_ref().and_then(|answer| answer.time),
                    answer.is_none(),
                );
                let Some(Answer { data, .. }) = answer else {
                    return;
                };
                let (v4, v6) = match data.prefixes {
                    Some(lists) => (
                        lists.v4.and_then(|family| family.originating),
                        lists.v6.and_then(|family| family.originating),
                    ),
                    None => (None, None),
                };
                let v4 = sorted(v4.unwrap_or_default());
                let v6 = sorted(v6.unwrap_or_default());
                let (list, sample) = shown_and_sampled(&v4, &v6, leading);
                let counted = |family: Option<ripestat::Originating<u64>>, listed: usize| {
                    family
                        .and_then(|family| family.originating)
                        .unwrap_or(listed as u64)
                };
                let (v4_count, v6_count) = match data.counts {
                    Some(counts) => (counted(counts.v4, v4.len()), counted(counts.v6, v6.len())),
                    None => (v4.len() as u64, v6.len() as u64),
                };
                self.emit(BgpEvent::NetworkPart(NetworkPart::Prefixes {
                    list,
                    v4: Some(v4_count),
                    v6: Some(v6_count),
                }));
                // Validate a sample from both families, six at a time.
                pool(sample, 6, |prefix| {
                    if !self.stopped() {
                        self.emit(BgpEvent::PrefixValidation(validate(
                            &self.agent,
                            &prefix,
                            asn,
                        )));
                    }
                });
            });
        });
    }

    /// Holder names by AS number, eight asked at a time. A network named once in this process
    /// is not asked for again.
    fn names(&self, asns: &[u32]) -> BTreeMap<u32, String> {
        static KNOWN: Mutex<BTreeMap<u32, String>> = Mutex::new(BTreeMap::new());
        let missing: Vec<u32> = match KNOWN.lock() {
            Ok(known) => asns
                .iter()
                .copied()
                .filter(|asn| !known.contains_key(asn))
                .collect(),
            Err(_) => asns.to_vec(),
        };
        pool(missing, 8, |asn| {
            let answer = self.get::<ripestat::AsOverview>(
                "as-overview",
                &[("resource", &format!("AS{asn}"))],
                8,
            );
            let name = answer
                .and_then(|answer| answer.data.holder)
                .map(|holder| clean_holder(&holder))
                .filter(|name| !name.is_empty());
            if let (Some(name), Ok(mut known)) = (name, KNOWN.lock()) {
                known.insert(asn, name);
            }
        });
        match KNOWN.lock() {
            Ok(known) => asns
                .iter()
                .filter_map(|asn| Some((*asn, known.get(asn)?.clone())))
                .collect(),
            Err(_) => BTreeMap::new(),
        }
    }
}

/// Runs `work` over `items` on up to `width` threads and returns when all are done.
fn pool<Item: Send>(items: Vec<Item>, width: usize, work: impl Fn(Item) + Sync) {
    let queue = Mutex::new(VecDeque::from(items));
    let workers = width.min(queue.lock().map_or(0, |queue| queue.len()));
    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let next = match queue.lock() {
                    Ok(mut queue) => queue.pop_front(),
                    Err(_) => None,
                };
                match next {
                    Some(item) => work(item),
                    None => return,
                }
            });
        }
    });
}

/// Validates one route. A failed request is `Unavailable`, never an old or assumed result.
pub fn validate(agent: &ureq::Agent, prefix: &str, origin: u32) -> Validation {
    let resource = format!("AS{origin}");
    let answer = ripestat::get::<ripestat::Validation>(
        agent,
        "rpki-validation",
        &[("resource", &resource), ("prefix", prefix)],
        15,
    );
    let Some(answer) = answer else {
        return Validation {
            prefix: prefix.to_string(),
            origin,
            status: RpkiStatus::Unavailable,
            roas: Vec::new(),
            checked_at: format::now(),
        };
    };
    Validation {
        prefix: prefix.to_string(),
        origin,
        status: RpkiStatus::from_provider(&answer.data.status),
        roas: answer
            .data
            .validating_roas
            .unwrap_or_default()
            .into_iter()
            .map(|roa| Roa {
                origin: roa.origin.0,
                prefix: roa.prefix,
                max_length: roa.max_length,
                validity: roa.validity,
            })
            .collect(),
        checked_at: answer.time.unwrap_or_else(format::now),
    }
}

/// Validates one route with an agent of its own, for Check a Route.
pub fn check(prefix: &str, origin: u32) -> Validation {
    validate(&ripestat::agent(), prefix, origin)
}

/// Groups every peer's route into distinct AS paths, most reported first. Prepending (an AS
/// repeated to lengthen the path) is collapsed and remembered.
pub fn fold(collectors: &[ripestat::Collector], prefix: &str) -> (Vec<Path>, Folded) {
    let mut by_path: BTreeMap<Vec<u32>, Path> = BTreeMap::new();
    let mut peers = 0;
    let mut seen_collectors = 0;
    for collector in collectors {
        let label = match &collector.location {
            Some(location) => format!("{} \u{00B7} {location}", collector.rrc),
            None => collector.rrc.clone(),
        };
        let mut counted = false;
        for peer in &collector.peers {
            if peer.prefix.as_deref().is_some_and(|other| other != prefix) {
                continue;
            }
            let raw: Vec<u32> = peer
                .as_path
                .split_whitespace()
                .filter_map(|asn| asn.parse().ok())
                .collect();
            if raw.is_empty() {
                continue;
            }
            let mut asns = raw.clone();
            asns.dedup();
            peers += 1;
            counted = true;
            let prepended = asns.len() != raw.len();
            let path = by_path.entry(asns.clone()).or_insert_with(|| Path {
                asns,
                peers: 0,
                collectors: Vec::new(),
                last_changed: None,
                prepended: false,
                communities: Vec::new(),
            });
            path.peers += 1;
            path.prepended |= prepended;
            if !path.collectors.contains(&label) {
                path.collectors.push(label.clone());
            }
            if let Some(changed) = peer.last_updated.as_deref().and_then(ripestat::date) {
                if path.last_changed.is_none_or(|known| changed > known) {
                    path.last_changed = Some(changed);
                }
            }
            if path.communities.is_empty() {
                if let Some(community) = peer.community.as_deref().filter(|text| !text.is_empty()) {
                    path.communities = community
                        .split_whitespace()
                        .take(12)
                        .map(str::to_string)
                        .collect();
                }
            }
        }
        if counted {
            seen_collectors += 1;
        }
    }
    let mut paths: Vec<Path> = by_path.into_values().collect();
    paths.sort_by(|a, b| {
        b.peers
            .cmp(&a.peers)
            .then(a.asns.len().cmp(&b.asns.len()))
            .then(a.asns.cmp(&b.asns))
    });
    let mut upstream: BTreeMap<u32, usize> = BTreeMap::new();
    for path in &paths {
        if let Some(asn) = path.upstream() {
            *upstream.entry(asn).or_default() += path.peers;
        }
    }
    let mut upstreams: Vec<Share> = upstream
        .into_iter()
        .map(|(asn, peers)| Share { asn, peers })
        .collect();
    upstreams.sort_by(|a, b| b.peers.cmp(&a.peers).then(a.asn.cmp(&b.asn)));
    upstreams.truncate(20);
    // Lengths weighted by peers: the median is what a typical collector peer sees.
    let mut lengths: Vec<usize> = paths
        .iter()
        .flat_map(|path| std::iter::repeat_n(path.asns.len(), path.peers))
        .collect();
    lengths.sort_unstable();
    let folded = Folded {
        paths: paths.len(),
        peers,
        collectors: seen_collectors,
        upstreams,
        shortest: lengths.first().copied().unwrap_or(0),
        typical: lengths.get(lengths.len() / 2).copied().unwrap_or(0),
        direct: paths
            .iter()
            .filter(|path| path.asns.len() <= 2)
            .map(|path| path.peers)
            .sum(),
    };
    (paths, folded)
}

/// The prefixes a network's list shows and the ones that get an RPKI check. The prefix that
/// was looked up leads both. Then the two families share the room.
fn shown_and_sampled(
    v4: &[String],
    v6: &[String],
    leading: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let take = |limit: usize| {
        let other = |name: &&String| Some(name.as_str()) != leading;
        let mut list: Vec<String> = leading.map(str::to_string).into_iter().collect();
        // The room left after the leading prefix, halved, and a family that needs less than
        // its half leaves the rest to the other.
        let room = limit - list.len();
        let half = room / 2;
        let v4_others = v4.iter().filter(other).count();
        let v6_others = v6.iter().filter(other).count();
        list.extend(
            v4.iter()
                .filter(other)
                .take((room - half).max(room.saturating_sub(v6_others)))
                .cloned(),
        );
        list.extend(
            v6.iter()
                .filter(other)
                .take(half.max(room.saturating_sub(v4_others)))
                .cloned(),
        );
        list.truncate(limit);
        list
    };
    (
        take(BgpNetwork::PREFIX_LIMIT),
        take(BgpNetwork::VALIDATION_LIMIT),
    )
}

/// Prefixes in address order, short prefixes before the more specific ones inside them.
pub fn sorted(mut prefixes: Vec<String>) -> Vec<String> {
    fn key(prefix: &str) -> (u128, u8) {
        let (base, length) = prefix.split_once('/').unwrap_or((prefix, "0"));
        let bits = match base.parse::<IpAddr>() {
            Ok(IpAddr::V4(address)) => u128::from(u32::from(address)),
            Ok(IpAddr::V6(address)) => u128::from(address),
            Err(_) => 0,
        };
        (bits, length.parse().unwrap_or(0))
    }
    prefixes.sort_by_key(|prefix| key(prefix));
    prefixes
}

/// "ARIN (Status: ALLOCATED)" and "Assigned by ARIN" both name the registry.
pub fn registry_name(description: &str) -> Option<&'static str> {
    let upper = description.to_uppercase();
    ["ARIN", "RIPE NCC", "APNIC", "LACNIC", "AFRINIC"]
        .into_iter()
        .find(|name| upper.contains(name))
}

/// Every address a name resolves to, both families, in the system's order. Empty when the name
/// does not resolve.
pub fn resolve_all(host: &str) -> Vec<IpAddr> {
    if let Ok(literal) = host.parse::<IpAddr>() {
        return vec![literal];
    }
    let mut addresses: Vec<IpAddr> = Vec::new();
    if let Ok(found) = (host, 0).to_socket_addrs() {
        for address in found.map(|socket| socket.ip()) {
            if !addresses.contains(&address) {
                addresses.push(address);
            }
        }
    }
    addresses
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bgp::ripestat::{Collector, Peer};

    fn peer(path: &str) -> Peer {
        Peer {
            as_path: path.to_string(),
            community: Some("64501:100 64501:200".to_string()),
            last_updated: Some("2027-01-15T08:00:00".to_string()),
            prefix: Some("192.0.2.0/24".to_string()),
        }
    }

    #[test]
    fn peers_fold_into_distinct_paths_with_prepending_collapsed() {
        let collectors = [
            Collector {
                rrc: "RRC01".to_string(),
                location: Some("London, GB".to_string()),
                peers: vec![
                    peer("64510 64501 64500"),
                    peer("64511 64501 64500 64500 64500"),
                    peer("64501 64500"),
                ],
            },
            Collector {
                rrc: "RRC03".to_string(),
                location: None,
                peers: vec![
                    peer("64510 64501 64500"),
                    peer("64512 64502 64500"),
                    Peer {
                        prefix: Some("192.0.2.0/23".to_string()),
                        ..peer("64512 64496")
                    },
                    peer(""),
                ],
            },
            Collector {
                rrc: "RRC05".to_string(),
                location: None,
                peers: Vec::new(),
            },
        ];
        let (paths, folded) = fold(&collectors, "192.0.2.0/24");
        assert_eq!(folded.paths, 4);
        assert_eq!((folded.peers, folded.collectors), (5, 2));
        assert_eq!(paths[0].asns, [64510, 64501, 64500]);
        assert_eq!(paths[0].peers, 2);
        assert_eq!(paths[0].collectors, ["RRC01 \u{00B7} London, GB", "RRC03"]);
        assert_eq!(paths[0].communities, ["64501:100", "64501:200"]);
        assert_eq!(paths[0].last_changed, Some(1_800_000_000));
        // Ties go to the shorter path.
        assert_eq!(paths[1].asns, [64501, 64500]);
        let prepended = paths
            .iter()
            .find(|path| path.asns == [64511, 64501, 64500])
            .unwrap();
        assert!(prepended.prepended);
        assert_eq!(
            folded.upstreams,
            [
                Share {
                    asn: 64501,
                    peers: 4
                },
                Share {
                    asn: 64502,
                    peers: 1
                }
            ]
        );
        assert_eq!((folded.shortest, folded.typical, folded.direct), (2, 3, 1));
    }

    #[test]
    fn prefixes_sort_in_address_order() {
        let list = |items: &[&str]| {
            items
                .iter()
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            sorted(list(&[
                "198.51.100.0/24",
                "192.0.2.128/25",
                "192.0.2.0/24",
                "192.0.2.0/23"
            ])),
            [
                "192.0.2.0/23",
                "192.0.2.0/24",
                "192.0.2.128/25",
                "198.51.100.0/24"
            ]
        );
        assert_eq!(
            sorted(list(&["2001:db8:1::/48", "2001:db8::/32"])),
            ["2001:db8::/32", "2001:db8:1::/48"]
        );
    }

    #[test]
    fn the_looked_up_prefix_leads_and_both_families_share_the_room() {
        let v4: Vec<String> = (0..400)
            .map(|index| format!("10.{}.{}.0/24", index / 256, index % 256))
            .collect();
        let v6: Vec<String> = (0..10)
            .map(|index| format!("2001:db8:{index}::/48"))
            .collect();
        let (shown, sample) = shown_and_sampled(&v4, &v6, Some("10.0.5.0/24"));
        assert_eq!(shown.len(), BgpNetwork::PREFIX_LIMIT);
        assert_eq!(shown[0], "10.0.5.0/24");
        assert_eq!(
            shown
                .iter()
                .filter(|prefix| *prefix == "10.0.5.0/24")
                .count(),
            1
        );
        assert_eq!(
            shown.iter().filter(|prefix| prefix.contains(':')).count(),
            10
        );
        assert_eq!(sample.len(), BgpNetwork::VALIDATION_LIMIT);
        assert_eq!(
            sample.iter().filter(|prefix| prefix.contains(':')).count(),
            10
        );
    }

    #[test]
    fn a_registry_is_named_however_the_block_describes_it() {
        assert_eq!(registry_name("ARIN (Status: ALLOCATED)"), Some("ARIN"));
        assert_eq!(registry_name("Assigned by ripe ncc"), Some("RIPE NCC"));
        assert_eq!(registry_name("IANA reserved"), None);
    }

    #[test]
    fn a_private_address_has_no_public_route_and_asks_nobody() {
        let (events, received) = std::sync::mpsc::channel();
        let stop = AtomicBool::new(false);
        run(&Target::parse("192.168.1.1").unwrap(), &[], &events, &stop);
        let events: Vec<BgpEvent> = received.try_iter().collect();
        assert_eq!(
            events,
            [
                BgpEvent::Unrouted(
                    "192.168.1.1 is a private or reserved address, so it has no public route."
                        .to_string()
                ),
                BgpEvent::Finished
            ]
        );
    }

    #[test]
    fn a_pool_runs_every_item_once() {
        let done = Mutex::new(Vec::new());
        pool((0..50).collect(), 6, |item: u32| {
            done.lock().unwrap().push(item);
        });
        let mut done = done.into_inner().unwrap();
        done.sort_unstable();
        assert_eq!(done, (0..50).collect::<Vec<u32>>());
        pool(Vec::<u32>::new(), 6, |_| unreachable!());
    }
}
