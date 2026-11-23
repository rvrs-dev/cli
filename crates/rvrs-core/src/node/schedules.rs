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
//! - An answer that puts a start less than half the interval after the schedule's last one has
//!   that start left out.
//! - A schedule that fails the node's own check is refused once, until the copy says something
//!   else about it.
//! - A copy is good for `keepSeconds`. After that nothing starts until the copy was read again.
//!   A restart never makes a copy fresh: the node keeps the copy's age beside it ([`Age`]) and
//!   starts with what is left.
//! - A schedule that leaves the copy ends at once. A run of it that is going is stopped.

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

/// The most of a copy's use a crash can leave unwritten: the node writes the age at least this
/// often while it runs. A start from the kept copy counts it as used.
pub const AGE_WRITTEN_EVERY_SECONDS: u64 = 600;

/// How old a kept copy is, written beside it. A node with a disk starts from its kept copy, and
/// without this every start would make the copy good for its whole `keepSeconds` again, so a
/// node that restarts now and then without its uplink would run a deleted schedule for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Age {
    /// When the copy's answer arrived, in seconds since 1970 by the node's own clock, which may
    /// be wrong.
    pub read_at: u64,
    /// How long the node has run with this copy since its answer, by a clock that only counts
    /// forward.
    pub used_seconds: u64,
}

impl Age {
    /// The age of an answer that arrives now.
    pub fn new(wall: u64) -> Age {
        Age {
            read_at: wall,
            used_seconds: 0,
        }
    }

    /// Reads the age a kept copy carries. `None` is a file without one, which an older build
    /// wrote, and such a file is no copy.
    pub fn read(value: &Value) -> Option<Age> {
        Some(Age {
            read_at: value["readAt"].as_u64()?,
            used_seconds: value["usedSeconds"].as_u64()?,
        })
    }

    /// How old the copy is at a start from the kept file, in seconds. `wall` is the clock's
    /// time now, in seconds since 1970. It is the larger of two counts. The time the node ran
    /// with the copy, plus what a crash could have left unwritten. And the time since the answer
    /// by the clock, which also counts the time the node was off. A clock that reads earlier
    /// than the answer is not synchronized and counts nothing.
    pub fn at_start(&self, wall: u64) -> u64 {
        let ran = self.used_seconds.saturating_add(AGE_WRITTEN_EVERY_SECONDS);
        let by_the_clock = wall.saturating_sub(self.read_at);
        ran.max(by_the_clock)
    }
}

/// What an answer changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Replaced {
    /// The version is another than the one the node held.
    pub changed: bool,
    /// The schedules that left the copy while a run of theirs was going. The node stops those
    /// runs now and sends what they measured.
    pub stop: Vec<String>,
}

/// Why a due run was left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeftOut {
    /// The node is at one of its limits: runs at once, or runs a minute.
    AtALimit,
    /// The schedule's last run is still going.
    StillGoing,
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
    /// How long the copy is good for after its answer.
    keep: Duration,
    /// When the copy's answer arrived, by the node's own clock, in seconds since 1970.
    read_at: u64,
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
    ///
    /// The copy is as old as its `age` says at this start ([`Age::at_start`]), with `wall` the
    /// clock's time in seconds since 1970, and is good for what is left of `keepSeconds`. A copy
    /// with nothing left is held as one that ran out while the node ran: no scheduled run
    /// starts until the node has read the copy again.
    pub fn from_kept(
        copy: &Copy,
        age: Age,
        now: Instant,
        wall: u64,
        mut random: impl FnMut() -> u64,
    ) -> Schedules {
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
        let left = copy.keep_seconds.saturating_sub(age.at_start(wall));
        Schedules {
            version: Some(copy.version.clone()),
            kept,
            good_until: Some(now + Duration::from_secs(left)),
            keep: Duration::from_secs(copy.keep_seconds),
            read_at: age.read_at,
            held: false,
            going: BTreeSet::new(),
        }
    }

    /// How old the copy the node holds is now, to write beside the kept copy. `None` while the
    /// node holds no copy.
    pub fn age(&self, now: Instant) -> Option<Age> {
        let left = self.good_until?.saturating_duration_since(now);
        Some(Age {
            read_at: self.read_at,
            used_seconds: self.keep.saturating_sub(left).as_secs(),
        })
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

    /// An answer arrived, at `now` and at `wall` seconds since 1970 by the node's clock. It
    /// replaces the copy the node held, whole, and the copy's age starts over.
    ///
    /// A schedule that is no longer in the copy ends at once: it was paused or deleted, or the
    /// node no longer matches it. No further run of it starts, and a run of it that is going is
    /// named in [`Replaced::stop`], for the node to stop now. A schedule that stays in the copy
    /// keeps its going run, also when the copy says another interval or other options.
    ///
    /// Every schedule's next start is `startsInMilliseconds` from now. A start that would come
    /// less than half an interval after the schedule's last one is left out, so an answer that
    /// moves a schedule's place in the interval never has two of its runs close together. A
    /// schedule the node refused stays refused while the copy says the same about it.
    pub fn replace(&mut self, copy: &Copy, now: Instant, wall: u64) -> Replaced {
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
                let last_start = was.and_then(|kept| kept.last_start);
                let mut next = now + Duration::from_millis(starts_in);
                let too_soon = last_start
                    .is_some_and(|last| next.saturating_duration_since(last) < entry.every() / 2);
                if too_soon {
                    next += entry.every();
                }
                Kept {
                    next,
                    last_start,
                    refused: was
                        .is_some_and(|kept| kept.refused && kept.entry.says_the_same(entry)),
                    entry: entry.clone(),
                }
            })
            .collect();
        let changed = self.version.as_deref() != Some(copy.version.as_str());
        self.version = Some(copy.version.clone());
        self.keep = Duration::from_secs(copy.keep_seconds);
        self.good_until = Some(now + self.keep);
        self.read_at = wall;
        self.held = false;
        Replaced {
            changed,
            stop: self.gone(),
        }
    }

    /// The schedules with a run that is going and no place in the copy.
    fn gone(&self) -> Vec<String> {
        self.going
            .iter()
            .filter(|schedule| {
                !self
                    .kept
                    .iter()
                    .any(|kept| kept.entry.schedule == **schedule)
            })
            .cloned()
            .collect()
    }

    /// The fleet was stopped. The node keeps its copy and starts no run of it until it has read
    /// the copy again.
    pub fn hold(&mut self) {
        self.held = true;
    }

    /// The node holds no schedules any more: its token is gone, or the API has none for it. The
    /// answer names the schedules with a run that is going, for the node to stop now.
    pub fn clear(&mut self) -> Vec<String> {
        self.version = None;
        self.kept.clear();
        self.good_until = None;
        self.held = false;
        self.gone()
    }

    /// True when the node holds schedules and its copy is older than it is good for. Nothing
    /// starts then. A run that is going goes on to its end.
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
    /// The node's clock when an answer arrives, in seconds since 1970.
    const WALL: u64 = 1_795_000_000;
    const DAY: u64 = 86_400;

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
        assert!(
            schedules
                .replace(&copy("node-schedules.response.json"), now, WALL)
                .changed
        );
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
        schedules.replace(&copy("node-schedules.response.json"), now, WALL);
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
        schedules.replace(&one("example.com", 60, 10_000), now, WALL);
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
        schedules.replace(&one("example.com", 60, 10_000), now, WALL);
        assert_eq!(
            started(&mut schedules, &mut guard, now + seconds(10)),
            [PING]
        );
        // A new answer 5 s later puts the next start 20 s after the last one. That is less than
        // half of 60 s, so it is left out, and the one after it runs.
        let answered = now + seconds(15);
        assert!(
            !schedules
                .replace(&one("example.com", 60, 15_000), answered, WALL)
                .changed
        );
        assert!(schedules.due(&mut guard, answered + seconds(15)).is_empty());
        assert!(started(&mut schedules, &mut guard, answered + seconds(74)).is_empty());
        assert_eq!(
            started(&mut schedules, &mut guard, answered + seconds(75)),
            [PING]
        );
        // An answer that puts it 31 s after the last start changes nothing about that run.
        let again = answered + seconds(75);
        schedules.replace(&one("example.com", 60, 31_000), again, WALL);
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
        schedules.replace(&one("example.com", 60, 0), now, WALL);
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
        schedules.replace(&two(), now, WALL);
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
        schedules.replace(&one("192.168.1.1", 60, 0), now, WALL);
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
        schedules.replace(&one("192.168.1.1", 60, 0), later, WALL);
        assert!(schedules.due(&mut guard, later).is_empty());
        // A copy that says something else is checked again: once more refused, then a target
        // the node probes.
        schedules.replace(&one("192.168.1.2", 60, 0), later, WALL);
        assert!(matches!(
            schedules.due(&mut guard, later)[..],
            [Due::Refuse { .. }]
        ));
        schedules.replace(&one("example.com", 60, 0), later, WALL);
        assert_eq!(started(&mut schedules, &mut guard, later), [PING]);
    }

    #[test]
    fn a_name_that_resolved_to_a_private_address_refuses_the_schedule_too() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&one("example.com", 60, 0), now, WALL);
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
        schedules.replace(&two(), now, WALL);
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
        let replaced = schedules.replace(&without, now + seconds(11), WALL);
        assert!(replaced.changed);
        assert_eq!(schedules.len(), 1);
        // The schedule was paused or deleted, so its run is to be stopped now. What it measured
        // is sent like any result, and no further run of it starts.
        assert_eq!(replaced.stop, [PING]);
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
        let mut schedules =
            Schedules::from_kept(&kept, Age::new(WALL), now, WALL, || numbers.next().unwrap());
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
        schedules.replace(&kept, answered, WALL);
        assert!(started(&mut schedules, &mut guard, answered + seconds(9)).is_empty());
    }

    #[test]
    fn a_copy_is_good_for_as_long_as_it_says() {
        let now = Instant::now();
        let mut guard = guard();
        let mut short = one("example.com", 60, 0);
        short.keep_seconds = 150;
        let mut schedules = Schedules::none();
        schedules.replace(&short, now, WALL);
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
        schedules.replace(&short, now + seconds(250), WALL);
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
        schedules.replace(&one("example.com", 60, 0), now, WALL);
        schedules.hold();
        // The copy is kept, and no run of it starts.
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules.version(), Some("example.com-60"));
        assert!(schedules.due(&mut guard, now + seconds(60)).is_empty());
        schedules.replace(&one("example.com", 60, 0), now + seconds(3600), WALL);
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
        schedules.replace(&two(), now, WALL);
        // A run of one of them is going. It is to be stopped, since the node has no schedules.
        assert!(matches!(
            schedules.due(&mut guard, now + seconds(10))[..],
            [Due::Start { .. }]
        ));
        assert_eq!(schedules.clear(), [PING]);
        assert!(schedules.is_empty());
        assert_eq!(schedules.version(), None);
        assert!(schedules.due(&mut guard, now + seconds(3600)).is_empty());
    }

    #[test]
    fn a_schedule_that_stays_in_the_copy_keeps_its_going_run() {
        let now = Instant::now();
        let mut guard = guard();
        let mut schedules = Schedules::none();
        schedules.replace(&one("example.com", 60, 0), now, WALL);
        assert!(matches!(
            schedules.due(&mut guard, now)[..],
            [Due::Start { .. }]
        ));
        // The same copy again, then one with another interval and one with other options: the
        // schedule is still there, so its run goes on.
        assert!(schedules
            .replace(&one("example.com", 60, 0), now, WALL)
            .stop
            .is_empty());
        let other_interval = schedules.replace(&one("example.com", 300, 0), now + seconds(1), WALL);
        assert!(other_interval.changed && other_interval.stop.is_empty());
        let mut other_options = one("example.com", 300, 0);
        other_options.schedules[0]
            .options
            .insert("packets".into(), 5.into());
        assert!(schedules
            .replace(&other_options, now + seconds(2), WALL)
            .stop
            .is_empty());
        // A copy that ran out starts no new run. It stops none either: nothing names the run.
        let mut short = one("example.com", 60, 0);
        short.keep_seconds = 10;
        assert!(schedules
            .replace(&short, now + seconds(3), WALL)
            .stop
            .is_empty());
        assert!(schedules.expired(now + seconds(13)));
        assert!(schedules.due(&mut guard, now + seconds(60)).is_empty());
        // A copy without the schedule stops it, once, and a later one has nothing left to stop.
        let empty = Copy {
            version: "none".into(),
            keep_seconds: 604_800,
            schedules: Vec::new(),
        };
        assert_eq!(
            schedules.replace(&empty, now + seconds(70), WALL).stop,
            [PING]
        );
        guard.finish();
        schedules.finished(PING);
        assert!(schedules
            .replace(&empty, now + seconds(80), WALL)
            .stop
            .is_empty());
    }

    #[test]
    fn the_age_of_a_copy_counts_while_the_node_runs_and_an_answer_starts_it_over() {
        let now = Instant::now();
        let mut schedules = Schedules::none();
        assert_eq!(schedules.age(now), None);
        schedules.replace(&one("example.com", 60, 0), now, WALL);
        assert_eq!(schedules.age(now), Some(Age::new(WALL)));
        assert_eq!(
            schedules.age(now + seconds(3 * DAY + 5)),
            Some(Age {
                read_at: WALL,
                used_seconds: 3 * DAY + 5
            })
        );
        // A copy is never older than it is good for.
        assert_eq!(
            schedules.age(now + seconds(30 * DAY)).unwrap().used_seconds,
            7 * DAY
        );
        // The next answer is a new copy with no age.
        let again = now + seconds(3 * DAY);
        schedules.replace(&one("example.com", 60, 0), again, WALL + 3 * DAY);
        assert_eq!(schedules.age(again), Some(Age::new(WALL + 3 * DAY)));
        // The age travels as two fields beside the copy, and a file without them has none.
        let written = serde_json::to_value(Age {
            read_at: WALL,
            used_seconds: 77,
        })
        .unwrap();
        assert_eq!(written, json!({ "readAt": WALL, "usedSeconds": 77 }));
        assert_eq!(Age::read(&written).unwrap().used_seconds, 77);
        assert_eq!(Age::read(&json!({ "version": "v", "schedules": [] })), None);
        assert_eq!(Age::read(&json!({ "readAt": WALL })), None);
        assert_eq!(
            Age::read(&json!({ "readAt": "now", "usedSeconds": 0 })),
            None
        );
    }

    #[test]
    fn a_copy_used_for_six_days_expires_a_day_after_a_restart() {
        let read = Instant::now();
        let kept = one("example.com", 60, 0);
        let mut schedules = Schedules::none();
        schedules.replace(&kept, read, WALL);
        // Six days later the node stops, with the copy's age written, and starts again.
        let age = schedules.age(read + seconds(6 * DAY)).unwrap();
        assert_eq!(age.used_seconds, 6 * DAY);
        let start = Instant::now();
        let mut schedules = Schedules::from_kept(&kept, age, start, WALL + 6 * DAY, || 0);
        let mut guard = guard();
        assert_eq!(started(&mut schedules, &mut guard, start), [PING]);
        // One day is left of the seven, less the ten minutes a crash could have left unwritten.
        // The restart did not make the copy good for seven days again.
        let left = DAY - AGE_WRITTEN_EVERY_SECONDS;
        assert!(!schedules.expired(start + seconds(left - 1)));
        assert_eq!(
            started(&mut schedules, &mut guard, start + seconds(left - 60)),
            [PING]
        );
        assert!(schedules.expired(start + seconds(left)));
        assert!(schedules.due(&mut guard, start + seconds(left)).is_empty());
        assert!(schedules
            .due(&mut guard, start + seconds(2 * DAY))
            .is_empty());
    }

    #[test]
    fn restarts_without_the_api_never_use_a_copy_for_more_than_it_is_good_for() {
        let kept = one("example.com", 60, 0);
        let mut age = Age::new(WALL);
        let mut good_hours = 0;
        // A node without its uplink that restarts every 40 hours, for a month. Its clock is not
        // synchronized and reads a time before the answer, so only the time it ran counts.
        for _ in 0..20 {
            let start = Instant::now();
            let schedules = Schedules::from_kept(&kept, age, start, WALL - 5 * DAY, || 0);
            for hour in 0..40 {
                if !schedules.expired(start + seconds(hour * 3600)) {
                    good_hours += 1;
                }
            }
            age = schedules.age(start + seconds(40 * 3600)).unwrap();
        }
        assert!(good_hours <= 7 * 24, "{good_hours}");
        // Most of the seven days were used: each restart costs the ten minutes it may have lost.
        assert!(good_hours >= 6 * 24, "{good_hours}");
        assert_eq!(age.used_seconds, 7 * DAY);
        // From then on every start finds the copy run out.
        let start = Instant::now();
        let mut schedules = Schedules::from_kept(&kept, age, start, WALL - 5 * DAY, || 0);
        assert!(schedules.expired(start));
        assert!(schedules.due(&mut guard(), start + seconds(60)).is_empty());
    }

    #[test]
    fn a_clock_that_moved_on_past_the_copys_time_finds_it_expired_at_the_start() {
        let kept = one("example.com", 60, 0);
        // The node ran an hour with the copy and was then off for eight days.
        let age = Age {
            read_at: WALL,
            used_seconds: 3600,
        };
        assert_eq!(age.at_start(WALL + 8 * DAY), 8 * DAY);
        let start = Instant::now();
        let mut schedules = Schedules::from_kept(&kept, age, start, WALL + 8 * DAY, || 0);
        let mut guard = guard();
        // It holds the copy, and no scheduled run starts until it has read the copy again.
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules.version(), Some("example.com-60"));
        assert!(schedules.expired(start));
        assert!(schedules.due(&mut guard, start + seconds(120)).is_empty());
        schedules.replace(&kept, start + seconds(130), WALL + 8 * DAY + 130);
        assert!(!schedules.expired(start + seconds(130)));
        assert_eq!(
            started(&mut schedules, &mut guard, start + seconds(130)),
            [PING]
        );
        // Off for two days only, the clock still counts more than the hour it ran.
        let start = Instant::now();
        let schedules = Schedules::from_kept(&kept, age, start, WALL + 2 * DAY, || 0);
        assert!(!schedules.expired(start + seconds(5 * DAY - 1)));
        assert!(schedules.expired(start + seconds(5 * DAY)));
    }

    #[test]
    fn a_clock_behind_the_copys_answer_counts_nothing() {
        let kept = one("example.com", 60, 0);
        let age = Age {
            read_at: WALL,
            used_seconds: DAY,
        };
        // A router that booted without a synchronized clock reads 1970. Only the day the node
        // ran with the copy counts, and the ten minutes a crash could have lost.
        for wall in [250, WALL - 400 * DAY, WALL - 1, WALL] {
            assert_eq!(
                age.at_start(wall),
                DAY + AGE_WRITTEN_EVERY_SECONDS,
                "{wall}"
            );
            let start = Instant::now();
            let schedules = Schedules::from_kept(&kept, age, start, wall, || 0);
            let left = 6 * DAY - AGE_WRITTEN_EVERY_SECONDS;
            assert!(!schedules.expired(start + seconds(left - 1)), "{wall}");
            assert!(schedules.expired(start + seconds(left)), "{wall}");
            // The answer's own time stays with the copy, for the next start.
            assert_eq!(schedules.age(start).unwrap().read_at, WALL);
        }
    }
}
