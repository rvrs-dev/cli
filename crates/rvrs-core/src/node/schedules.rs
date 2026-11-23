//! The schedules a node keeps (`schema/node-protocol/README.md`, Schedules).
//!
//! A schedule is a run a node repeats at an interval. The API hands the node its whole copy:
//! what to run, how often, and how long after the answer the next run starts. From there the
//! node repeats each one by itself, on a clock that only counts forward, so it goes on measuring
//! while the API is out of reach.
//!
//! Everything here is decided from the copy and the time it is handed. Nothing reads a clock or
//! opens a socket, so the rules are tested without waiting:
//!
//! - An answer replaces the whole copy and sets every next start anew.
//! - A run is due once. One the node can't start, at a limit or with the schedule's last run
//!   still going, is left out and never made up.
//! - A start less than half the interval after the schedule's last one is left out.
//! - A schedule that fails the node's own check is refused once, until the copy says something
//!   else about it.
//! - A copy is good for `keepSeconds`. After that nothing starts until the copy was read again.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::refusal::Refusal;
use super::request::{Admitted, Request};
use super::Guard;

/// The most schedules a node keeps. An answer with more is read up to here.
pub const AT_MOST: usize = 50;

/// How long a copy is good for when its answer doesn't say: the outbox's 7 days.
const KEEP_SECONDS: u64 = 604_800;

/// The longest interval a node keeps a schedule at. An entry with a longer one is not read.
const EVERY_AT_MOST: u64 = 366 * 86_400;

/// One schedule as the node's copy has it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    /// The schedule's id.
    pub schedule: String,
    /// The word a build names the tool by in its build features.
    pub tool: String,
    /// What the person typed.
    pub target: String,
    /// What the person set. An option left out takes the tool's default.
    #[serde(default)]
    pub options: Map<String, Value>,
    /// The interval the node keeps.
    pub every_seconds: u64,
    /// How long after the answer the next run starts. It is different in every answer and is no
    /// part of the copy a node keeps on disk.
    #[serde(default, skip_serializing)]
    pub starts_in_milliseconds: u64,
}

impl Entry {
    /// True when both say the same thing to run: the tool, the target, the options and the
    /// interval. When the next run starts is no part of it.
    pub fn says_the_same(&self, other: &Entry) -> bool {
        self.tool == other.tool
            && self.target == other.target
            && self.options == other.options
            && self.every_seconds == other.every_seconds
    }

    /// The entry as the node's check reads a run. A scheduled run has no run id and no attempt,
    /// so the schedule's id stands where the run's would.
    pub fn request(&self) -> Request {
        Request {
            run: self.schedule.clone(),
            attempt: 0,
            tool: self.tool.clone(),
            target: self.target.clone(),
            options: self.options.clone(),
        }
    }

    fn every(&self) -> Duration {
        Duration::from_secs(self.every_seconds)
    }
}

/// A node's whole copy of its schedules, as `GET /v1/nodes/me/schedules` answers and as the node
/// keeps it beside its outbox.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Copy {
    /// A word that changes when an entry comes, goes or says something else to run.
    pub version: String,
    /// How long the copy is good for after its answer.
    pub keep_seconds: u64,
    pub schedules: Vec<Entry>,
}

impl Copy {
    /// Reads an answer, or a kept copy. A field the node doesn't know is ignored, an entry it
    /// can't read is left out, and at most [`AT_MOST`] entries are taken, each schedule once.
    /// `None` is something that is no copy at all.
    pub fn read(value: &Value) -> Option<Copy> {
        let version = value["version"].as_str()?.to_string();
        let listed = value["schedules"].as_array()?;
        let mut schedules: Vec<Entry> = Vec::new();
        for entry in listed {
            if schedules.len() == AT_MOST {
                break;
            }
            let Ok(entry) = serde_json::from_value::<Entry>(entry.clone()) else {
                continue;
            };
            let readable = !entry.schedule.is_empty()
                && (1..=EVERY_AT_MOST).contains(&entry.every_seconds)
                && !schedules.iter().any(|kept| kept.schedule == entry.schedule);
            if readable {
                schedules.push(entry);
            }
        }
        Some(Copy {
            version,
            keep_seconds: value["keepSeconds"]
                .as_u64()
                .filter(|seconds| *seconds > 0)
                .unwrap_or(KEEP_SECONDS)
                .min(EVERY_AT_MOST),
            schedules,
        })
    }
}

/// Why a due run was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftOut {
    /// The node is at one of its limits: runs at once, or runs a minute.
    AtALimit,
    /// The schedule's last run is still going.
    StillGoing,
    /// The schedule's last run started less than half an interval ago.
    TooSoon,
}

/// What the node does about a run that is due.
#[derive(Debug, Clone, PartialEq)]
pub enum Due {
    /// The run passed the node's check and counts as a run it is making.
    Start {
        schedule: String,
        request: Request,
        admitted: Admitted,
    },
    /// The schedule failed the node's check. The node sends one report that says why and starts
    /// no run of it until the copy says something else about it.
    Refuse {
        schedule: String,
        request: Request,
        refusal: Refusal,
    },
    /// The run is left out. It is never made up.
    LeftOut { schedule: String, why: LeftOut },
}

#[derive(Debug, Clone)]
struct Kept {
    entry: Entry,
    next: Instant,
    last_start: Option<Instant>,
    refused: bool,
}

/// The schedules a node holds and when each is due.
#[derive(Debug, Clone, Default)]
pub struct Schedules {
    version: Option<String>,
    kept: Vec<Kept>,
    /// The copy is good until then. None while the node holds no copy.
    good_until: Option<Instant>,
    /// Nothing starts until the copy was read again.
    held: bool,
    /// The schedules with a run that is still going, also one that left the copy meanwhile.
    going: BTreeSet<String>,
}

impl Schedules {
    /// A node that holds no schedules.
    pub fn none() -> Schedules {
        Schedules::default()
    }

    /// Starts from a copy the node kept, without an answer: each schedule at a random time
    /// within its interval. `random` gives a random number each time it is called.
    pub fn from_kept(copy: &Copy, now: Instant, mut random: impl FnMut() -> u64) -> Schedules {
        let kept = copy
            .schedules
            .iter()
            .take(AT_MOST)
            .map(|entry| Kept {
                next: now + Duration::from_millis(random() % (entry.every_seconds.max(1) * 1000)),
                last_start: None,
                refused: false,
                entry: entry.clone(),
            })
            .collect();
        Schedules {
            version: Some(copy.version.clone()),
            kept,
            good_until: Some(now + Duration::from_secs(copy.keep_seconds)),
            held: false,
            going: BTreeSet::new(),
        }
    }

    /// The version of the copy the node holds. A heartbeat's answer that names another one
    /// means the copy is behind.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// How many schedules the node holds.
    pub fn len(&self) -> usize {
        self.kept.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }

    /// An answer arrived. It replaces the copy the node held, whole: a schedule that is no
    /// longer in it ends at once, and every other one's next start is `startsInMilliseconds`
    /// from now. A schedule the node refused stays refused while the copy says the same about
    /// it. True when the version is another than the one the node held.
    pub fn replace(&mut self, copy: &Copy, now: Instant) -> bool {
        let before = std::mem::take(&mut self.kept);
        self.kept = copy
            .schedules
            .iter()
            .take(AT_MOST)
            .map(|entry| {
                let was = before
                    .iter()
                    .find(|kept| kept.entry.schedule == entry.schedule);
                // A place in the interval is never further away than the interval is long.
                let starts_in = entry.starts_in_milliseconds.min(entry.every_seconds * 1000);
                Kept {
                    next: now + Duration::from_millis(starts_in),
                    last_start: was.and_then(|kept| kept.last_start),
                    refused: was
                        .is_some_and(|kept| kept.refused && kept.entry.says_the_same(entry)),
                    entry: entry.clone(),
                }
            })
            .collect();
        let changed = self.version.as_deref() != Some(copy.version.as_str());
        self.version = Some(copy.version.clone());
        self.good_until = Some(now + Duration::from_secs(copy.keep_seconds));
        self.held = false;
        changed
    }

    /// The fleet was stopped. The node keeps its copy and starts no run of it until it has read
    /// the copy again.
    pub fn hold(&mut self) {
        self.held = true;
    }

    /// The node holds no schedules any more: its token is gone, or the API has none for it.
    pub fn clear(&mut self) {
        self.version = None;
        self.kept.clear();
        self.good_until = None;
        self.held = false;
    }

    /// True when the node holds schedules and its copy is older than it is good for.
    pub fn expired(&self, now: Instant) -> bool {
        !self.kept.is_empty() && self.good_until.is_some_and(|until| now >= until)
    }

    /// A run of this schedule ended, however it ended.
    pub fn finished(&mut self, schedule: &str) {
        self.going.remove(schedule);
    }

    /// The address a run of this schedule resolved to is one the node sends nothing to, which
    /// only shows once the run was started. It counts as a failed check.
    pub fn refuse(&mut self, schedule: &str) {
        if let Some(kept) = self
            .kept
            .iter_mut()
            .find(|kept| kept.entry.schedule == schedule)
        {
            kept.refused = true;
        }
    }

    /// Everything that is due now, each schedule once. A run that starts is counted by `guard`
    /// as a run the node is making, and ends with [`Guard::finish`] and
    /// [`Schedules::finished`].
    pub fn due(&mut self, guard: &mut Guard, now: Instant) -> Vec<Due> {
        if self.held || self.good_until.is_none_or(|until| now >= until) {
            return Vec::new();
        }
        let mut due = Vec::new();
        for kept in &mut self.kept {
            if now < kept.next {
                continue;
            }
            // The next start keeps the schedule's place in the interval. The starts a node
            // missed while it did something else are passed over, never made up.
            let every = kept.entry.every();
            let behind = now.duration_since(kept.next).as_nanos() / every.as_nanos().max(1);
            let steps = u32::try_from(behind).unwrap_or(u32::MAX).saturating_add(1);
            kept.next = kept
                .next
                .checked_add(every.saturating_mul(steps))
                .unwrap_or(now + every);
            if kept.refused {
                continue;
            }
            let schedule = kept.entry.schedule.clone();
            let too_soon = kept
                .last_start
                .is_some_and(|last| now.duration_since(last) < every / 2);
            if too_soon {
                let why = LeftOut::TooSoon;
                due.push(Due::LeftOut { schedule, why });
                continue;
            }
            if self.going.contains(&schedule) {
                let why = LeftOut::StillGoing;
                due.push(Due::LeftOut { schedule, why });
                continue;
            }
            let request = kept.entry.request();
            match guard.admit(&request, now) {
                Ok(admitted) => {
                    kept.last_start = Some(now);
                    self.going.insert(schedule.clone());
                    due.push(Due::Start {
                        schedule,
                        request,
                        admitted,
                    });
                }
                Err(Refusal::AtOnce { .. } | Refusal::PerMinute { .. }) => {
                    let why = LeftOut::AtALimit;
                    due.push(Due::LeftOut { schedule, why });
                }
                Err(refusal) => {
                    kept.refused = true;
                    due.push(Due::Refuse {
                        schedule,
                        request,
                        refusal,
                    });
                }
            }
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::address::Allowed;
    use crate::node::Limits;
    use serde_json::json;
    use std::path::Path;

    const FULL: [&str; 7] = ["traceroute", "ping", "mtr", "dns", "bgp", "tcp", "mtu"];
    const PING: &str = "7a1c9e52-3b4d-4f6a-8c0e-2d5f7b9a1c3e";
    const MTR: &str = "c4d8e1f2-6a7b-4c9d-8e0f-1a2b3c4d5e6f";

    fn fixture(name: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../schema/node-protocol/fixtures")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn guard() -> Guard {
        Guard::new(Limits::default(), &FULL, Allowed::none())
    }

    fn copy(name: &str) -> Copy {
        Copy::read(&fixture(name)).unwrap()
    }

    /// The second node's copy, with a target a node probes in place of the MTR's documentation
    /// address.
    fn two() -> Copy {
        let mut copy = copy("node-schedules.other.response.json");
        copy.schedules[1].target = "example.net".into();
        copy
    }

    /// A copy with one ping every `every` seconds, first in `starts_in` milliseconds.
    fn one(target: &str, every: u64, starts_in: u64) -> Copy {
        Copy::read(&json!({
            "version": format!("{target}-{every}"),
            "keepSeconds": 604_800,
            "schedules": [{
                "schedule": PING, "tool": "ping", "target": target, "options": {},
                "everySeconds": every, "startsInMilliseconds": starts_in,
            }],
        }))
        .unwrap()
    }

    fn seconds(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    /// The schedules that started at `now`, with their runs ended at once.
    fn started(schedules: &mut Schedules, guard: &mut Guard, now: Instant) -> Vec<String> {
        let mut ids = Vec::new();
        for due in schedules.due(guard, now) {
            if let Due::Start { schedule, .. } = due {
                guard.finish();
                schedules.finished(&schedule);
                ids.push(schedule);
            }
        }
        ids
    }

    #[test]
    fn the_protocols_copies_are_read() {
        let first = copy("node-schedules.response.json");
        assert_eq!(first.version, "5d0c2a9e41b7f630");
        assert_eq!(first.keep_seconds, 604_800);
        assert_eq!(first.schedules.len(), 1);
        let entry = &first.schedules[0];
        assert_eq!(entry.schedule, PING);
        assert_eq!(
            (entry.tool.as_str(), entry.target.as_str()),
            ("ping", "example.com")
        );
        assert_eq!(entry.options["packets"], 10);
        assert_eq!(
            (entry.every_seconds, entry.starts_in_milliseconds),
            (60, 41_250)
        );
        let other = copy("node-schedules.other.response.json");
        assert_eq!(other.schedules.len(), 2);
        assert_eq!(other.schedules[1].every_seconds, 1200);
        // The ping passes the node's own check as it stands. The MTR's target is a
        // documentation address, which a node probes as little as any reserved one.
        let mut guard = guard();
        let now = Instant::now();
        assert!(guard.admit(&other.schedules[0].request(), now).is_ok());
        let refusal = guard.admit(&other.schedules[1].request(), now).unwrap_err();
        assert_eq!(refusal.kind().word(), "target");
    }

    #[test]
    fn a_copy_is_read_with_what_this_version_knows() {
        let entry = |id: usize| {
            json!({
                "schedule": format!("s{id}"), "tool": "ping", "target": "example.com",
                "options": {}, "everySeconds": 60, "startsInMilliseconds": 0, "priority": "high",
            })
        };
        let mut listed: Vec<Value> = vec![
            // No target, an interval of nothing, and one the node can't count to.
            json!({ "schedule": "a", "tool": "ping", "everySeconds": 60 }),
            json!({ "schedule": "b", "tool": "ping", "target": "example.com", "everySeconds": 0 }),
            json!({ "schedule": "c", "tool": "ping", "target": "example.com", "everySeconds": u64::MAX }),
            json!("nothing"),
        ];
        listed.extend((0..60).map(entry));
        // The same schedule twice is one schedule.
        listed.insert(5, entry(0));
        let read = Copy::read(&json!({
            "version": "v", "schedules": listed, "spread": "even",
        }))
        .unwrap();
        assert_eq!(read.schedules.len(), AT_MOST);
        assert_eq!(read.schedules[0].schedule, "s0");
        assert_eq!(read.schedules[1].schedule, "s1");
        // An answer that doesn't say how long it is good for is good for the outbox's 7 days.
        assert_eq!(read.keep_seconds, 604_800);
        // Something that is no copy is none.
        assert_eq!(Copy::read(&json!({ "schedules": [] })), None);
        assert_eq!(Copy::read(&json!({ "version": "v" })), None);
        assert_eq!(
            Copy::read(&json!({ "error": { "code": "NOT_FOUND" } })),
            None
        );
    }

    #[test]
    fn the_kept_copy_leaves_out_when_the_next_run_starts() {
        let read = copy("node-schedules.other.response.json");
        let kept = serde_json::to_value(&read).unwrap();
        assert!(kept["schedules"][0].get("startsInMilliseconds").is_none());
        assert_eq!(kept["version"], "b81e07c93a2f4d16");
        assert_eq!(kept["keepSeconds"], 604_800);
        let back = Copy::read(&kept).unwrap();
        assert_eq!(back.schedules.len(), 2);
        assert!(back.schedules[1].says_the_same(&read.schedules[1]));
        assert_eq!(back.schedules[1].starts_in_milliseconds, 0);
    }

    #[test]
    fn an_answer_sets_the_next_start_and_the_node_repeats_from_there() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        assert!(schedules.replace(&copy("node-schedules.response.json"), now));
        assert_eq!(schedules.version(), Some("5d0c2a9e41b7f630"));
        assert_eq!(schedules.len(), 1);
        // Nothing before 41,250 ms, one run at it, and then one every 60 seconds.
        let at = |milliseconds: u64| now + Duration::from_millis(milliseconds);
        assert!(started(&mut schedules, &mut guard, at(41_249)).is_empty());
        assert_eq!(started(&mut schedules, &mut guard, at(41_250)), [PING]);
        assert!(started(&mut schedules, &mut guard, at(41_251)).is_empty());
        assert!(started(&mut schedules, &mut guard, at(101_249)).is_empty());
        assert_eq!(started(&mut schedules, &mut guard, at(101_250)), [PING]);
        assert_eq!(started(&mut schedules, &mut guard, at(161_250)), [PING]);
    }

    #[test]
    fn the_run_it_starts_is_the_one_the_copy_names() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&copy("node-schedules.response.json"), now);
        let due = schedules.due(&mut guard, now + seconds(42));
        let [Due::Start {
            schedule,
            request,
            admitted,
        }] = due.as_slice()
        else {
            panic!("{due:?}")
        };
        assert_eq!(schedule, PING);
        assert_eq!(
            (request.tool.as_str(), request.target.as_str()),
            ("ping", "example.com")
        );
        assert_eq!(admitted.packets, 10);
        // It counts against the node's limits like any other run.
        assert_eq!(guard.room(now + seconds(42)), 1);
    }

    #[test]
    fn a_run_is_due_once_and_a_missed_one_is_never_made_up() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&one("example.com", 60, 10_000), now);
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(10)),
            [PING]
        );
        // The node did something else for five minutes. One run starts, and the next one is at
        // the schedule's own place in the interval: 10 s after each full minute.
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(325)),
            [PING]
        );
        assert!(started(&mut schedules, &mut guard, now + seconds(326)).is_empty());
        assert!(started(&mut schedules, &mut guard, now + seconds(369)).is_empty());
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(370)),
            [PING]
        );
    }

    #[test]
    fn a_start_less_than_half_an_interval_after_the_last_one_is_left_out() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&one("example.com", 60, 10_000), now);
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(10)),
            [PING]
        );
        // A new answer 5 s later puts the next start 20 s after the last one. That is less than
        // half of 60 s, so it is left out, and the one after it runs.
        let answered = now + seconds(15);
        assert!(!schedules.replace(&one("example.com", 60, 15_000), answered));
        assert_eq!(
            schedules.due(&mut guard, answered + seconds(15)),
            [Due::LeftOut {
                schedule: PING.into(),
                why: LeftOut::TooSoon
            }]
        );
        assert_eq!(
            started(&mut schedules, &mut guard, answered + seconds(75)),
            [PING]
        );
        // An answer that puts it 31 s after the last start changes nothing about that run.
        let again = answered + seconds(75);
        schedules.replace(&one("example.com", 60, 31_000), again);
        assert_eq!(
            started(&mut schedules, &mut guard, again + seconds(31)),
            [PING]
        );
    }

    #[test]
    fn a_run_is_left_out_while_the_schedules_last_one_is_still_going() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&one("example.com", 60, 0), now);
        assert!(matches!(
            schedules.due(&mut guard, now)[..],
            [Due::Start { .. }]
        ));
        assert_eq!(
            schedules.due(&mut guard, now + seconds(60)),
            [Due::LeftOut {
                schedule: PING.into(),
                why: LeftOut::StillGoing
            }]
        );
        // It ends. The next due run starts, and the one that was left out is not made up.
        guard.finish();
        schedules.finished(PING);
        assert!(schedules.due(&mut guard, now + seconds(61)).is_empty());
        assert!(matches!(
            schedules.due(&mut guard, now + seconds(120))[..],
            [Due::Start { .. }]
        ));
    }

    #[test]
    fn a_run_is_left_out_while_the_node_is_at_a_limit() {
        let now = Instant::now();
        let limits = Limits {
            runs_at_once: 1,
            runs_per_minute: 2,
            ..Limits::default()
        };
        let mut guard = Guard::new(limits, &FULL, Allowed::none());
        let mut schedules = Schedules::none();
        schedules.replace(&two(), now);
        // A run someone asked for takes the node's one place.
        let asked = one("example.net", 60, 0).schedules[0].request();
        assert!(guard.admit(&asked, now).is_ok());
        let at = now + Duration::from_millis(9_800);
        assert_eq!(
            schedules.due(&mut guard, at),
            [Due::LeftOut {
                schedule: PING.into(),
                why: LeftOut::AtALimit
            }]
        );
        // The schedule is not refused for it: its next run starts once there is room.
        guard.finish();
        assert!(schedules.due(&mut guard, at + seconds(1)).is_empty());
        assert_eq!(
            started(&mut schedules, &mut guard, at + seconds(60)),
            [PING]
        );
        // Two starts in the last minute are all this node allows, so the MTR is left out too.
        let mtr = now + Duration::from_millis(733_412);
        assert!(guard.admit(&asked, mtr - seconds(1)).is_ok());
        guard.finish();
        assert!(guard.admit(&asked, mtr - seconds(1)).is_ok());
        guard.finish();
        let due = schedules.due(&mut guard, mtr);
        assert!(due.contains(&Due::LeftOut {
            schedule: MTR.into(),
            why: LeftOut::AtALimit
        }));
    }

    #[test]
    fn a_schedule_that_fails_the_check_is_refused_once() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&one("192.168.1.1", 60, 0), now);
        let due = schedules.due(&mut guard, now);
        let [Due::Refuse {
            schedule, refusal, ..
        }] = due.as_slice()
        else {
            panic!("{due:?}")
        };
        assert_eq!(schedule, PING);
        assert_eq!(refusal.kind().word(), "target");
        // A refused schedule counts as no run, and nothing more comes of it.
        assert_eq!(guard.room(now), 2);
        for minute in 1..=5 {
            assert!(schedules
                .due(&mut guard, now + seconds(60 * minute))
                .is_empty());
        }
        // A copy that says the same about it changes nothing.
        let later = now + seconds(400);
        schedules.replace(&one("192.168.1.1", 60, 0), later);
        assert!(schedules.due(&mut guard, later).is_empty());
        // A copy that says something else is checked again: once more refused, then a target
        // the node probes.
        schedules.replace(&one("192.168.1.2", 60, 0), later);
        assert!(matches!(
            schedules.due(&mut guard, later)[..],
            [Due::Refuse { .. }]
        ));
        schedules.replace(&one("example.com", 60, 0), later);
        assert_eq!(started(&mut schedules, &mut guard, later), [PING]);
    }

    #[test]
    fn a_name_that_resolved_to_a_private_address_refuses_the_schedule_too() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&one("example.com", 60, 0), now);
        assert_eq!(started(&mut schedules, &mut guard, now), [PING]);
        schedules.refuse(PING);
        assert!(schedules.due(&mut guard, now + seconds(60)).is_empty());
        // A schedule that left the copy is refused nowhere.
        schedules.refuse("gone");
    }

    #[test]
    fn a_schedule_that_leaves_the_copy_ends_at_once() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&two(), now);
        assert_eq!(schedules.len(), 2);
        // The ping's run is going when a copy without the ping arrives.
        assert!(matches!(
            schedules.due(&mut guard, now + seconds(10))[..],
            [Due::Start { .. }]
        ));
        let without = Copy {
            version: "w".into(),
            keep_seconds: 604_800,
            schedules: vec![two().schedules[1].clone()],
        };
        assert!(schedules.replace(&without, now + seconds(11)));
        assert_eq!(schedules.len(), 1);
        // Its run ends and is sent like any. No further run of it starts.
        guard.finish();
        schedules.finished(PING);
        assert!(schedules.due(&mut guard, now + seconds(70)).is_empty());
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(11 + 734)),
            [MTR]
        );
    }

    #[test]
    fn a_kept_copy_starts_each_schedule_at_a_random_time_within_its_interval() {
        let now = Instant::now();
        let kept = two();
        // The random numbers put the ping 7 s in and the MTR 1,000 s in.
        let mut numbers = [60_000 * 5 + 7_000, 1_200_000 * 3 + 1_000_000].into_iter();
        let mut schedules = Schedules::from_kept(&kept, now, || numbers.next().unwrap());
        assert_eq!(schedules.version(), Some("b81e07c93a2f4d16"));
        let mut guard = guard();
        assert!(started(&mut schedules, &mut guard, now + seconds(6)).is_empty());
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(7)),
            [PING]
        );
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(67)),
            [PING]
        );
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(1000)),
            [PING, MTR]
        );
        // The answer that arrives later puts each one at the API's place in the interval.
        let answered = now + seconds(1001);
        schedules.replace(&kept, answered);
        assert!(started(&mut schedules, &mut guard, answered + seconds(9)).is_empty());
    }

    #[test]
    fn a_copy_is_good_for_as_long_as_it_says() {
        let now = Instant::now();
        let mut guard = guard();
        let mut short = one("example.com", 60, 0);
        short.keep_seconds = 150;
        let mut schedules = Schedules::none();
        schedules.replace(&short, now);
        assert_eq!(started(&mut schedules, &mut guard, now), [PING]);
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(120)),
            [PING]
        );
        assert!(!schedules.expired(now + seconds(149)));
        // After 150 s nothing starts until the node has read the copy again.
        assert!(schedules.expired(now + seconds(150)));
        assert!(schedules.due(&mut guard, now + seconds(180)).is_empty());
        assert!(schedules.due(&mut guard, now + seconds(240)).is_empty());
        schedules.replace(&short, now + seconds(250));
        assert!(!schedules.expired(now + seconds(250)));
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(250)),
            [PING]
        );
        // A node without schedules has nothing that could be too old.
        assert!(!Schedules::none().expired(now));
    }

    #[test]
    fn a_stopped_fleet_holds_the_schedules_until_the_copy_is_read_again() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&one("example.com", 60, 0), now);
        schedules.hold();
        // The copy is kept, and no run of it starts.
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules.version(), Some("example.com-60"));
        assert!(schedules.due(&mut guard, now + seconds(60)).is_empty());
        schedules.replace(&one("example.com", 60, 0), now + seconds(3600));
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(3600)),
            [PING]
        );
    }

    #[test]
    fn a_node_without_a_token_holds_no_schedules() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&copy("node-schedules.other.response.json"), now);
        schedules.clear();
        assert!(schedules.is_empty());
        assert_eq!(schedules.version(), None);
        assert!(schedules.due(&mut guard, now + seconds(3600)).is_empty());
    }
}
