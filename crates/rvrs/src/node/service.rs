//! The node's loop: what `rvrs node` does once it has a token.
//!
//! It follows the protocol's page (`schema/node-protocol/README.md`). A heartbeat says what the
//! node is. One event stream is held open, and `work` on it means claim now. A claim says every
//! run the node holds and how many more it takes, and gets the runs that wait. Each run is made
//! within the node's own limits and its result waits in the outbox until the API has answered
//! for it.
//!
//! A node also repeats runs by itself: its schedules (`rvrs_core::node::schedules`). It reads its
//! copy of them after every `hello`, after every `schedules` event and when a heartbeat's answer
//! names another version, keeps the copy beside the outbox with its age, and starts each run when
//! it is due, with the API in reach or not. A scheduled run passes the same check as a run that
//! was asked for, and its result waits in the same outbox. A schedule that leaves the copy ends
//! at once: a run of it that is going is stopped, and what it measured is sent.
//!
//! Nothing here trusts a wake-up to arrive. The node claims after every `hello` and every `work`,
//! every third of the lease while it holds a run, after a stream that failed to open, and at a
//! random time while it has nothing to do. Every number it waits by comes from the API, as a
//! length in seconds, and is counted on a clock that only goes forward.
//!
//! No line it prints carries a token, a target or anything from a report. A run is named by its
//! id and a schedule by its own.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rvrs_core::model::{Run, Tool};
use rvrs_core::node::refusal::{Identity, Refusal};
use rvrs_core::node::request::{Admitted, Request};
use rvrs_core::node::schedules::{Age, Copy, Due, LeftOut, Schedules, AGE_WRITTEN_EVERY_SECONDS};
use rvrs_core::node::{Guard, Heartbeat, Outbox as SaidOutbox, Stopped, System, PROTOCOL};
use rvrs_core::node_config::NodeDeclaration;
use rvrs_core::report::Report;
use rvrs_core::resolve::{self, ResolveError};
use rvrs_core::FamilyChoice;
use serde_json::{json, Value};

use super::api::{Answer, Api};
use super::kept::KeptCopy;
use super::outbox::{Entry, Outbox};
use super::runner;
use crate::help::VERSION;

/// The longest a reconnect or a retry waits, at random, once failures have doubled it this far.
const WINDOW_AT_MOST: u64 = 300;
/// How long the node waits after the API said it may not do this now.
const ASK_AGAIN: Duration = Duration::from_secs(3600);
/// A stream is cut after this long and opened again. A healthy one never notices more than a
/// reconnect an hour.
const LONGEST_STREAM: Duration = Duration::from_secs(3600);
/// How much longer than the loop the thread that reads a stream waits before it gives a silent
/// one up. The loop notices first and says so, and the thread ends a moment later.
const READER_WAITS_LONGER: u64 = 5;
/// A claim that got no answer is sent again with its key for this long. After that the node
/// claims anew.
const SAME_CLAIM: Duration = Duration::from_secs(120);

/// The numbers a node works by. `hello` sets them, and these stand in until the first one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Numbers {
    keep_alive: u64,
    claim: u64,
    heartbeat: u64,
    reconnect: u64,
}

impl Default for Numbers {
    fn default() -> Self {
        Numbers {
            keep_alive: 25,
            claim: 1800,
            heartbeat: 3600,
            reconnect: 10,
        }
    }
}

/// Makes an admitted run with the address its guard checked, until it ends or is told to stop.
pub type Make = Box<dyn Fn(&Admitted, Option<IpAddr>, &Arc<AtomicBool>) -> Run + Send + Sync>;
/// Resolves a name in a family.
pub type Resolve = Box<dyn Fn(&str, FamilyChoice) -> Result<IpAddr, ResolveError> + Send + Sync>;

/// How a run is made and how a name is resolved. The node uses the engines and the system's
/// resolver. A test stands in for both, so it sends no packet.
pub struct Hands {
    pub make: Make,
    pub resolve: Resolve,
}

impl Default for Hands {
    fn default() -> Self {
        Hands {
            make: Box::new(runner::make),
            resolve: Box::new(resolve::resolve),
        }
    }
}

/// What the node was set up with.
pub struct Settings {
    pub api: Api,
    pub guard: Guard,
    pub outbox: Outbox,
    /// Where the node keeps its copy of its schedules.
    pub schedules: KeptCopy,
    pub declaration: NodeDeclaration,
    /// What the node calls itself until the API has said its name.
    pub name: String,
    pub hands: Hands,
    /// Set to stop the node: by a signal, or by a test.
    pub stop: Arc<AtomicBool>,
    /// Where a line about what the node does goes.
    pub log: Box<dyn Fn(&str) + Send>,
    /// What the node's pulse counts: that it is in use, and the runs it made.
    pub count: Box<dyn Fn(Counted) + Send>,
}

/// One thing the pulse counts. Nothing of a run but its tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counted {
    /// The API took a heartbeat, so the node has a token that works.
    Heartbeat,
    Run(Tool),
}

/// Why the loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// It was told to stop.
    Stopped,
    /// The API doesn't know the node's token, or it was revoked. The node makes no further call.
    NoToken,
}

enum StreamEvent {
    Hello(Value),
    Work,
    /// The node's schedules changed. The copy says how.
    Schedules,
    /// A keep-alive, or an event this version doesn't know. Either shows the stream is alive.
    Line,
    /// The stream is over. With an answer, it never opened.
    Ended(Option<Answer>),
}

enum Event {
    Stream(u64, StreamEvent),
    Made(Box<Made>),
}

/// A run thread's result.
struct Made {
    request: Request,
    /// The schedule the node made the run by. None for a run that was asked for.
    schedule: Option<String>,
    /// The revision of the node's tags and location when the run started. The report carries
    /// this one, whatever the API said while the run was going.
    revision: Option<u32>,
    started: Instant,
    what: Result<Run, Refusal>,
}

/// A run the node is making.
struct Held {
    stop: Arc<AtomicBool>,
    /// The API said to drop it: the run ends and nothing is sent.
    dropped: bool,
    /// A verdict this version doesn't know, which stopped the run.
    unknown_verdict: Option<String>,
}

struct Stream {
    generation: u64,
    last_line: Instant,
}

/// A random number, from the system where it gives one.
fn random() -> u64 {
    let mut bytes = [0u8; 8];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut file| file.read_exact(&mut bytes));
    if read.is_ok() {
        return u64::from_le_bytes(bytes);
    }
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

/// A random wait of up to `seconds`.
fn within(seconds: u64) -> Duration {
    Duration::from_millis(random() % (seconds.max(1) * 1000))
}

/// A random word, for a claim's key and for the boot.
pub fn word(length: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    (0..length)
        .map(|_| ALPHABET[(random() % ALPHABET.len() as u64) as usize] as char)
        .collect()
}

/// The id of the report for a run that was asked for: the run and the attempt. The API tells a
/// node's runs apart by this id for as long as it keeps them, so it may never repeat on one
/// node. Made from the request, it is also the same every time the node sends the result.
fn report_id(request: &Request) -> String {
    format!("{}-{}", request.run, request.attempt)
}

/// The id of the report for a scheduled run. No run id and no attempt name such a run, so the
/// id is 26 random characters, 128 bits, and repeats on no node. It is kept with the result, so it
/// is the same every time the node sends it.
fn scheduled_report_id() -> String {
    word(26)
}

/// "1 schedule", "3 schedules".
fn counted(schedules: usize) -> String {
    match schedules {
        1 => "1 schedule".to_string(),
        many => format!("{many} schedules"),
    }
}

/// True when a 2xx answer to a result is the API's receipt for it: it names a run, and for a run
/// that was asked for it names that run.
fn is_receipt(answer: &Value, entry: &Entry) -> bool {
    match (answer["id"].as_str(), entry.claim()) {
        (Some(id), Some((run, _))) => id == run,
        (Some(id), None) => !id.is_empty(),
        (None, _) => false,
    }
}

/// The body of a result for a run that was asked for: the report and the claim it was made
/// under.
fn claimed(report: &Report, request: &Request) -> Value {
    json!({
        "report": report,
        "claim": { "run": request.run, "attempt": request.attempt },
    })
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Reads a stream's events until it ends. `send` answers false when nobody listens any more.
fn read_events(body: Box<dyn Read + Send>, send: impl Fn(StreamEvent) -> bool) {
    let mut name = String::new();
    let mut data = String::new();
    for line in BufReader::new(body).lines() {
        let Ok(line) = line else { return };
        let listening = if line.is_empty() {
            let event = match name.as_str() {
                "hello" => Some(StreamEvent::Hello(
                    serde_json::from_str(&data).unwrap_or(Value::Null),
                )),
                "work" => Some(StreamEvent::Work),
                "schedules" => Some(StreamEvent::Schedules),
                // An event with a name this version doesn't know is ignored.
                "" => None,
                _ => Some(StreamEvent::Line),
            };
            name.clear();
            data.clear();
            event.is_none_or(&send)
        } else if line.starts_with(':') {
            send(StreamEvent::Line)
        } else {
            if let Some(value) = line.strip_prefix("event:") {
                name = value.trim().to_string();
            } else if let Some(value) = line.strip_prefix("data:") {
                data.push_str(value.trim());
            }
            true
        };
        if !listening {
            return;
        }
    }
}

struct Node {
    api: Arc<Api>,
    guard: Guard,
    outbox: Outbox,
    declaration: NodeDeclaration,
    hands: Arc<Hands>,
    stop: Arc<AtomicBool>,
    log: Box<dyn Fn(&str) + Send>,
    count: Box<dyn Fn(Counted) + Send>,
    boot: String,
    name: String,
    id: Option<String>,
    /// The revision of the node's tags and location the API last said.
    revision: Option<u32>,
    /// Where the API said the stream is.
    events: Option<String>,
    numbers: Numbers,
    lease: u64,
    runs: HashMap<(String, u32), Held>,
    /// The schedules the node holds and when each is due.
    schedules: Schedules,
    kept: KeptCopy,
    /// The scheduled runs the node is making, by their schedule, each with what ends it.
    scheduled: HashMap<String, Arc<AtomicBool>>,
    /// The copy is to be read now: after `hello`, after a `schedules` event, and when a
    /// heartbeat's answer named another version.
    read_now: bool,
    /// A read that got no answer is made again then.
    next_read: Option<Instant>,
    read_window: u64,
    /// The log said that the copy is older than it is good for.
    said_expired: bool,
    /// The copy's age is written to the kept copy then.
    next_age: Instant,
    /// The log said that the kept copy can't be written.
    said_unkept: bool,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
    stream: Option<Stream>,
    generation: u64,
    /// The window a reconnect waits in, at random. Failures double it and `hello` sets it back.
    window: u64,
    next_stream: Option<Instant>,
    next_heartbeat: Instant,
    /// The dropped count the API was last told.
    said_dropped: u32,
    claim_now: bool,
    next_claim: Instant,
    /// A claim that got no answer, to send again as it was, with its key.
    pending_claim: Option<(Value, Instant)>,
    claim_window: u64,
    next_send: Instant,
    send_window: u64,
    /// The API said the node may not do this now. Nothing is called until then.
    paused_until: Option<Instant>,
    ended: Option<Ended>,
}

impl Node {
    fn say(&self, line: &str) {
        (self.log)(line);
    }

    fn identity(&self) -> Identity<'_> {
        Identity {
            name: &self.name,
            id: self.id.as_deref(),
            version: VERSION,
        }
    }

    /// Ends every run the node is making, the scheduled ones too. Their results still go to the
    /// outbox.
    fn stop_runs(&mut self) {
        for held in self.runs.values() {
            held.stop.store(true, Ordering::Relaxed);
        }
        for stop in self.scheduled.values() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    fn close_stream(&mut self) {
        // The thread that reads it ends with its connection, or once nothing arrived on it for
        // a little over twice the keep-alive. Its events are ignored.
        self.stream = None;
        self.next_stream = None;
    }

    /// What the node does with a refusal that isn't about one call: a token the API doesn't
    /// know, and a call it may not make now. True when the caller has nothing more to do.
    fn refused(&mut self, status: u16, code: &str, now: Instant) -> bool {
        match (status, code) {
            (401, _) => {
                self.stop_runs();
                self.close_stream();
                // Without a token the node has no schedules, and keeps no copy of any.
                self.schedules.clear();
                self.kept.remove();
                self.ended = Some(Ended::NoToken);
                self.say("The API doesn't know this node's token, or the token was revoked. The node sends nothing until it is given a new one.");
                true
            }
            (403, "FLEET_STOPPED") => {
                self.stop_runs();
                self.close_stream();
                // The copy stays. No run of it starts until the node has read it again.
                self.schedules.hold();
                self.paused_until = Some(now + ASK_AGAIN);
                self.say("An admin stopped every node of this workspace. This node ended its runs, keeps its results and asks again in an hour.");
                true
            }
            (403, _) => {
                self.close_stream();
                self.paused_until = Some(now + ASK_AGAIN);
                self.say(&format!(
                    "The API refused this node ({code}). It asks again in an hour."
                ));
                true
            }
            _ => false,
        }
    }

    fn beat(&self) -> Value {
        let beat = Heartbeat {
            version: VERSION.to_string(),
            protocol: PROTOCOL,
            system: System {
                os: std::env::consts::OS.to_string(),
                arch: std::env::consts::ARCH.to_string(),
            },
            boot: self.boot.clone(),
            features: self.guard.features().to_vec(),
            limits: self.guard.limits(),
            outbox: SaidOutbox {
                waiting: self.outbox.waiting(),
                dropped: self.outbox.dropped(),
                keeps: self.outbox.keeps().to_string(),
            },
            tags: Some(self.declaration.tags.clone()),
            location: self.declaration.location.clone(),
        };
        serde_json::to_value(beat).unwrap_or(Value::Null)
    }

    fn heartbeat(&mut self, now: Instant) {
        self.said_dropped = self.outbox.dropped();
        match self.api.heartbeat(&self.beat()) {
            Answer::Taken(answer) => {
                (self.count)(Counted::Heartbeat);
                self.next_heartbeat = now + Duration::from_secs(self.numbers.heartbeat);
                let name = answer["node"]["name"].as_str().unwrap_or(&self.name);
                if self.id.is_none() || name != self.name {
                    self.say(&format!("The API knows this node as {name}."));
                }
                self.name = name.to_string();
                self.id = answer["node"]["id"].as_str().map(str::to_string);
                self.revision = answer["node"]["metadataRevision"]
                    .as_u64()
                    .and_then(|revision| u32::try_from(revision).ok());
                if let Some(events) = answer["events"].as_str() {
                    self.events = Some(events.to_string());
                }
                // The answer names the version of the copy the node should hold. Another one
                // than the node holds means its copy is behind.
                if let Some(version) = answer["schedules"].as_str() {
                    if self.schedules.version() != Some(version) {
                        self.read_now = true;
                    }
                }
            }
            Answer::Refused {
                status,
                code,
                retry_after,
            } => {
                if !self.refused(status, &code, now) {
                    let wait = retry_after.unwrap_or(self.numbers.heartbeat);
                    self.next_heartbeat = now + Duration::from_secs(wait);
                }
            }
            Answer::None => self.next_heartbeat = now + within(self.window.max(10)),
        }
    }

    fn open_stream(&mut self, now: Instant) {
        self.generation += 1;
        let generation = self.generation;
        self.stream = Some(Stream {
            generation,
            last_line: now,
        });
        self.next_stream = None;
        let api = Arc::clone(&self.api);
        let url = api.events_at(self.events.as_deref());
        let sender = self.sender.clone();
        let silent = Duration::from_secs(self.numbers.keep_alive * 2 + READER_WAITS_LONGER);
        std::thread::spawn(move || {
            let send = |event| sender.send(Event::Stream(generation, event)).is_ok();
            match api.stream(&url, LONGEST_STREAM, silent) {
                Ok(body) => {
                    read_events(body, send);
                    send(StreamEvent::Ended(None));
                }
                Err(answer) => {
                    send(StreamEvent::Ended(Some(answer)));
                }
            }
        });
    }

    /// The stream is gone or never opened. Another is opened after a random wait in a window
    /// that doubles with every failure in a row, and the node claims meanwhile, so one whose
    /// network lets no stream through still gets its runs.
    fn stream_failed(&mut self, now: Instant) {
        self.stream = None;
        self.next_stream = Some(now + within(self.window));
        self.window = (self.window * 2).min(WINDOW_AT_MOST);
        self.claim_now = true;
    }

    fn on_stream(&mut self, generation: u64, event: StreamEvent, now: Instant) {
        // An event of a stream the node gave up on.
        if self.stream.as_ref().map(|stream| stream.generation) != Some(generation) {
            return;
        }
        if let Some(stream) = self.stream.as_mut() {
            stream.last_line = now;
        }
        match event {
            StreamEvent::Hello(numbers) => {
                let read =
                    |name: &str, or: u64| numbers[name].as_u64().filter(|n| *n > 0).unwrap_or(or);
                self.numbers = Numbers {
                    keep_alive: read("keepAliveSeconds", self.numbers.keep_alive),
                    claim: read("claimSeconds", self.numbers.claim),
                    heartbeat: read("heartbeatSeconds", self.numbers.heartbeat),
                    reconnect: read("reconnectSeconds", self.numbers.reconnect),
                };
                // The API is there. Every wait that failures had doubled starts over.
                self.window = self.numbers.reconnect;
                self.claim_window = self.numbers.reconnect;
                self.send_window = self.numbers.reconnect;
                self.read_window = self.numbers.reconnect;
                self.claim_now = true;
                self.read_now = true;
                self.next_send = now;
                self.say("The event stream is open.");
            }
            StreamEvent::Work => self.claim_now = true,
            StreamEvent::Schedules => self.read_now = true,
            StreamEvent::Line => {}
            StreamEvent::Ended(None) => self.stream_failed(now),
            StreamEvent::Ended(Some(Answer::Refused { status, code, .. })) => {
                if !self.refused(status, &code, now) {
                    self.stream_failed(now);
                }
            }
            StreamEvent::Ended(Some(_)) => self.stream_failed(now),
        }
    }

    /// Everything the node holds: the runs it is making and the results the API hasn't answered
    /// for.
    fn holding(&self) -> Vec<Value> {
        let mut held: Vec<(String, u64)> = self
            .runs
            .iter()
            .filter(|(_, held)| !held.dropped)
            .map(|((run, attempt), _)| (run.clone(), u64::from(*attempt)))
            .collect();
        held.extend(self.outbox.claims());
        held.sort();
        held.dedup();
        held.into_iter()
            .map(|(run, attempt)| json!({ "run": run, "attempt": attempt }))
            .collect()
    }

    fn claim(&mut self, now: Instant) {
        self.claim_now = false;
        // A claim that got no answer is sent again as it was, so the API knows it for the same.
        let body = match self.pending_claim.take() {
            Some((body, since)) if now.saturating_duration_since(since) < SAME_CLAIM => body,
            _ => json!({
                "key": word(16),
                "room": self.guard.room(now),
                "holding": self.holding(),
            }),
        };
        match self.api.claim(&body) {
            Answer::Taken(answer) => {
                self.claim_window = self.numbers.reconnect;
                self.take(&answer, now);
            }
            Answer::Refused {
                status,
                code,
                retry_after,
            } => {
                if !self.refused(status, &code, now) {
                    let wait = retry_after.unwrap_or(self.numbers.reconnect);
                    self.next_claim = now + Duration::from_secs(wait);
                }
            }
            Answer::None => {
                self.pending_claim = Some((body, now));
                self.next_claim = now + within(self.claim_window);
                self.claim_window = (self.claim_window * 2).min(WINDOW_AT_MOST);
            }
        }
    }

    /// Reads a claim's answer: a verdict for each run the node listed, and the runs it holds
    /// from now.
    fn take(&mut self, answer: &Value, now: Instant) {
        if let Some(lease) = answer["leaseSeconds"].as_u64().filter(|lease| *lease > 0) {
            self.lease = lease;
        }
        for verdict in answer["holding"].as_array().into_iter().flatten() {
            let (Some(run), Some(attempt)) = (verdict["run"].as_str(), verdict["attempt"].as_u64())
            else {
                continue;
            };
            let key = (run.to_string(), u32::try_from(attempt).unwrap_or(u32::MAX));
            match verdict["verdict"].as_str().unwrap_or_default() {
                "keep" => {}
                "stop" => {
                    if let Some(held) = self.runs.get(&key) {
                        held.stop.store(true, Ordering::Relaxed);
                    }
                }
                "drop" => {
                    if let Some(held) = self.runs.get_mut(&key) {
                        held.dropped = true;
                        held.stop.store(true, Ordering::Relaxed);
                    }
                    self.outbox.forget(run, attempt);
                }
                // A verdict this version doesn't know is never read as keep. The run stops,
                // and what it measured is sent with the word.
                unknown => {
                    if let Some(held) = self.runs.get_mut(&key) {
                        held.unknown_verdict = Some(unknown.to_string());
                        held.stop.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
        for run in answer["runs"].as_array().into_iter().flatten() {
            if let Ok(request) = serde_json::from_value::<Request>(run.clone()) {
                self.start(request, now);
            }
        }
        // While it holds a run the node claims every third of the lease, which keeps the lease.
        // With nothing to do it claims at a random time between half of claimSeconds and all.
        // A scheduled run's result is held under no lease, so it keeps no claim coming.
        let holds = !self.runs.is_empty() || !self.outbox.claims().is_empty();
        self.next_claim = now
            + if holds {
                Duration::from_secs((self.lease / 3).max(1))
            } else {
                Duration::from_secs(self.numbers.claim / 2) + within(self.numbers.claim / 2)
            };
    }

    /// Keeps a result until the API has answered for it: the report, with the claim it was made
    /// under or the schedule it belongs to.
    fn keep(&mut self, body: Value, started: Instant) {
        if !self.outbox.put(body, Some(started)) {
            self.say("The outbox's folder can't be written. This result waits in memory and is lost if the node stops before the API has it.");
        }
        self.next_send = Instant::now();
    }

    /// Answers a run the node won't make with a report that says why. `revision` is the
    /// revision the node held when it checked the run.
    fn refuse(
        &mut self,
        refusal: &Refusal,
        request: &Request,
        started: Instant,
        revision: Option<u32>,
    ) {
        self.say(&refusal.log_line(request));
        let id = report_id(request);
        let mut report = refusal.report(request, &self.identity(), id, unix_seconds());
        report.origin.metadata_revision = revision;
        self.keep(claimed(&report, request), started);
    }

    /// A schedule failed the node's check. One report says why, with the schedule's id beside
    /// it, and no run of the schedule starts until a copy says something else about it.
    fn refuse_schedule(
        &mut self,
        refusal: &Refusal,
        schedule: &str,
        request: &Request,
        started: Instant,
        revision: Option<u32>,
    ) {
        self.say(&refusal.schedule_log_line(schedule));
        let id = scheduled_report_id();
        let mut report = refusal.report(request, &self.identity(), id, unix_seconds());
        report.origin.metadata_revision = revision;
        self.keep(json!({ "report": report, "schedule": schedule }), started);
    }

    /// Starts a run the API handed over, or refuses it. A run the node holds already, as this
    /// attempt or a newer one, is left as it is.
    fn start(&mut self, request: Request, now: Instant) {
        let newer_or_same = self
            .runs
            .keys()
            .any(|(run, attempt)| *run == request.run && *attempt >= request.attempt);
        if newer_or_same {
            return;
        }
        // Of two attempts of one run, the higher one is the run. The lower one is dropped.
        for ((run, _), held) in self.runs.iter_mut() {
            if *run == request.run {
                held.dropped = true;
                held.stop.store(true, Ordering::Relaxed);
            }
        }
        let admitted = match self.guard.admit(&request, now) {
            Ok(admitted) => admitted,
            Err(refusal) => return self.refuse(&refusal, &request, now, self.revision),
        };
        let stop = Arc::new(AtomicBool::new(false));
        self.runs.insert(
            (request.run.clone(), request.attempt),
            Held {
                stop: Arc::clone(&stop),
                dropped: false,
                unknown_verdict: None,
            },
        );
        self.say(&format!(
            "run {} attempt {}: started",
            request.run, request.attempt
        ));
        self.make(request, None, admitted, stop, now);
    }

    /// Makes an admitted run on a thread of its own, which hands the run back to the loop. The
    /// run keeps the revision of the node's tags and location that holds now, at its start.
    fn make(
        &self,
        request: Request,
        schedule: Option<String>,
        admitted: Admitted,
        stop: Arc<AtomicBool>,
        now: Instant,
    ) {
        let guard = self.guard.clone();
        let hands = Arc::clone(&self.hands);
        let sender = self.sender.clone();
        let revision = self.revision;
        std::thread::spawn(move || {
            // The name is resolved once, here, and the address is checked before anything is
            // sent. The engine gets the address.
            let what =
                match guard.destination(&admitted, |name, family| (hands.resolve)(name, family)) {
                    Ok(destination) => Ok((hands.make)(&admitted, destination, &stop)),
                    Err(Stopped::Refused(refusal)) => Err(refusal),
                    Err(Stopped::Failed(why)) => {
                        Ok(runner::failed(admitted.tool, &admitted.target, &why))
                    }
                };
            let _ = sender.send(Event::Made(Box::new(Made {
                request,
                schedule,
                revision,
                started: now,
                what,
            })));
        });
    }

    /// The report of a run the node made, as the node sends it: under the id given, with the
    /// target as it was asked and the node as its origin. `revision` is the revision of the
    /// node's tags and location when the run started. A tag edited while a long run was going
    /// belongs to the runs that start after it.
    fn report_of(&self, run: &Run, id: String, target: &str, revision: Option<u32>) -> Report {
        let mut report = Report::of(run);
        report.id = id;
        // The report names the target as it was asked, whatever the tool made of it.
        report.target = target.to_string();
        report.origin.kind = "node".into();
        report.origin.node = Some(self.name.clone());
        report.origin.node_id = self.id.clone();
        report.origin.metadata_revision = revision;
        report
    }

    /// Stops the runs of schedules that left the node's copy: paused, deleted, or no longer the
    /// node's. Each run ends now, and what it measured is sent with the outcome `stopped` and
    /// the schedule's id, as a run the API says to stop is.
    fn stop_scheduled(&mut self, schedules: &[String]) {
        for schedule in schedules {
            if let Some(stop) = self.scheduled.get(schedule) {
                stop.store(true, Ordering::Relaxed);
                self.say(&format!(
                    "schedule {schedule}: no longer in the node's copy, so its run is stopped"
                ));
            }
        }
    }

    /// Says once that the kept copy can't be written, and again only after a write that worked.
    fn kept_on_disk(&mut self, on_disk: bool) {
        if on_disk {
            self.said_unkept = false;
        } else if !self.said_unkept {
            self.said_unkept = true;
            self.say("The node's state folder can't be written. Its copy of its schedules is kept in memory and is gone when the node stops.");
        }
    }

    /// Writes how old the node's copy is beside the kept copy: when the node starts from it,
    /// every ten minutes while it runs, and when it stops. A restart then finds the copy as old
    /// as it is and never good for its whole time again.
    fn keep_age(&mut self) {
        let now = Instant::now();
        self.next_age = now + Duration::from_secs(AGE_WRITTEN_EVERY_SECONDS);
        if let Some(age) = self.schedules.age(now) {
            let on_disk = self.kept.save_age(age);
            self.kept_on_disk(on_disk);
        }
    }

    /// Reads the node's copy of its schedules. The loop makes the call and waits for its
    /// answer, so there is one read at a time.
    fn read_schedules(&mut self, now: Instant) {
        self.read_now = false;
        self.next_read = None;
        let answer = match self.api.schedules() {
            Answer::Taken(answer) => match Copy::read(&answer) {
                Some(copy) => {
                    // Every next start counts from the moment of the answer, and so does the
                    // copy's age.
                    let (answered, wall) = (Instant::now(), unix_seconds());
                    let replaced = self.schedules.replace(&copy, answered, wall);
                    self.stop_scheduled(&replaced.stop);
                    self.read_window = self.numbers.reconnect;
                    self.said_expired = false;
                    self.next_age = answered + Duration::from_secs(AGE_WRITTEN_EVERY_SECONDS);
                    if replaced.changed {
                        self.say(&format!(
                            "The node keeps {}.",
                            counted(self.schedules.len())
                        ));
                    }
                    let on_disk = self.kept.save(&copy, Age::new(wall));
                    self.kept_on_disk(on_disk);
                    return;
                }
                // A good status without a copy in it is no answer of the API's.
                None => Answer::None,
            },
            answer => answer,
        };
        match answer {
            Answer::Taken(_) => {}
            Answer::Refused {
                status,
                code,
                retry_after,
            } => {
                if self.refused(status, &code, now) {
                    return;
                }
                match status {
                    // An API from before schedules. The node holds none, and asks again after
                    // its next hello.
                    404 => {
                        if self.schedules.version().is_some() {
                            self.say("The API has no schedules for this node. The node holds none and asks again when it next connects.");
                        }
                        let going = self.schedules.clear();
                        self.stop_scheduled(&going);
                        self.kept.remove();
                    }
                    429 => {
                        let wait = retry_after.unwrap_or(self.numbers.reconnect);
                        self.next_read = Some(now + Duration::from_secs(wait));
                    }
                    // The call is wrong and stays wrong. It is not repeated.
                    _ => {}
                }
            }
            Answer::None => {
                self.next_read = Some(now + within(self.read_window));
                self.read_window = (self.read_window * 2).min(WINDOW_AT_MOST);
            }
        }
    }

    /// Starts the scheduled runs that are due. A run is due once: one the node can't start now
    /// is left out and never made up.
    fn start_due(&mut self) {
        if self.ended.is_some() || self.paused_until.is_some() {
            return;
        }
        let now = Instant::now();
        if self.schedules.expired(now) && !self.said_expired {
            self.said_expired = true;
            self.read_now = true;
            self.say("The node's copy of its schedules is older than it is good for. No scheduled run starts until the node has read it again.");
        }
        for due in self.schedules.due(&mut self.guard, now) {
            match due {
                Due::Start {
                    schedule,
                    request,
                    admitted,
                } => {
                    let stop = Arc::new(AtomicBool::new(false));
                    self.scheduled.insert(schedule.clone(), Arc::clone(&stop));
                    self.make(request, Some(schedule), admitted, stop, now);
                }
                Due::Refuse {
                    schedule,
                    request,
                    refusal,
                } => self.refuse_schedule(&refusal, &schedule, &request, now, self.revision),
                Due::LeftOut { schedule, why } => match why {
                    LeftOut::AtALimit => self.say(&format!(
                        "schedule {schedule}: left out, the node is at one of its limits"
                    )),
                    LeftOut::StillGoing => self.say(&format!(
                        "schedule {schedule}: left out, its last run is still going"
                    )),
                },
            }
        }
    }

    /// A scheduled run ended. Its report goes to the outbox with the schedule's id beside it,
    /// also when the schedule left the copy while the run was going and the run was stopped.
    fn on_scheduled(&mut self, schedule: &str, made: Made) {
        self.schedules.finished(schedule);
        self.scheduled.remove(schedule);
        let run = match made.what {
            Ok(run) => run,
            // The name resolved to an address the node sends nothing to.
            Err(refusal) => {
                self.schedules.refuse(schedule);
                return self.refuse_schedule(
                    &refusal,
                    schedule,
                    &made.request,
                    made.started,
                    made.revision,
                );
            }
        };
        (self.count)(Counted::Run(run.tool));
        let mut report = self.report_of(
            &run,
            scheduled_report_id(),
            &made.request.target,
            made.revision,
        );
        if let Err(unfit) = self.guard.fit(&mut report) {
            self.outbox.lost();
            self.say(&format!("schedule {schedule}: not sent. {unfit}"));
            return;
        }
        self.say(&format!("schedule {schedule}: {}", report.outcome));
        self.keep(
            json!({ "report": report, "schedule": schedule }),
            made.started,
        );
    }

    /// A run thread ended. Its report goes to the outbox, unless the API said to drop the run.
    fn on_made(&mut self, made: Made) {
        self.guard.finish();
        if let Some(schedule) = made.schedule.clone() {
            return self.on_scheduled(&schedule, made);
        }
        let key = (made.request.run.clone(), made.request.attempt);
        let Some(held) = self.runs.remove(&key) else {
            return;
        };
        if held.dropped {
            return;
        }
        let run = match made.what {
            Ok(run) => run,
            Err(refusal) => {
                return self.refuse(&refusal, &made.request, made.started, made.revision)
            }
        };
        (self.count)(Counted::Run(run.tool));
        let mut report = self.report_of(
            &run,
            report_id(&made.request),
            &made.request.target,
            made.revision,
        );
        if let Some(verdict) = &held.unknown_verdict {
            report.outcome = "stopped".into();
            report.error = Some(format!(
                "{} doesn't know the verdict \u{201C}{verdict}\u{201D}, so it stopped the run.",
                self.name
            ));
        }
        if let Err(unfit) = self.guard.fit(&mut report) {
            self.outbox.lost();
            self.say(&format!(
                "run {} attempt {}: not sent. {unfit}",
                made.request.run, made.request.attempt
            ));
            return;
        }
        self.say(&format!(
            "run {} attempt {}: {}",
            made.request.run, made.request.attempt, report.outcome
        ));
        self.keep(claimed(&report, &made.request), made.started);
    }

    /// Sends the oldest result that waits. One a turn of the loop: a node that comes back with
    /// a full outbox still claims when a claim is due, which is what keeps the leases of the
    /// runs it is making, and still reads its stream.
    fn send(&mut self, now: Instant) {
        let Some(entry) = self.outbox.oldest().cloned() else {
            return;
        };
        let answer = match self.api.result(&entry.to_send(Instant::now())) {
            // A 2xx that doesn't name the run is no receipt. The result stays until one comes.
            Answer::Taken(answer) if !is_receipt(&answer, &entry) => {
                self.say("An answer to a result didn't name its run. The result waits and is sent again.");
                Answer::None
            }
            answer => answer,
        };
        match answer {
            Answer::Taken(_) => {
                self.outbox.answered(&entry.name);
                self.send_window = self.numbers.reconnect;
            }
            Answer::Refused {
                status,
                code,
                retry_after,
            } => {
                if self.refused(status, &code, now) {
                    return;
                }
                match status {
                    // The claim is lost: a newer attempt has the run, or it has its result.
                    409 => self.outbox.answered(&entry.name),
                    429 => {
                        let wait = retry_after.unwrap_or(self.numbers.reconnect);
                        self.next_send = now + Duration::from_secs(wait);
                    }
                    // The call is wrong and stays wrong. It is not repeated.
                    _ => self.outbox.give_up(&entry.name),
                }
            }
            Answer::None => {
                self.next_send = now + within(self.send_window);
                self.send_window = (self.send_window * 2).min(WINDOW_AT_MOST);
            }
        }
    }

    /// True when a result waits that may go now. The loop then turns again at once.
    fn sends_next(&self, now: Instant) -> bool {
        self.ended.is_none()
            && self.paused_until.is_none()
            && self.outbox.waiting() > 0
            && now >= self.next_send
    }

    /// Everything that is due now.
    fn tick(&mut self, now: Instant) {
        // The copy's age counts while the node runs, also while it may not call the API.
        if now >= self.next_age {
            self.keep_age();
        }
        if let Some(until) = self.paused_until {
            if now < until {
                return;
            }
            self.paused_until = None;
            self.next_heartbeat = now;
            self.claim_now = true;
            self.read_now = true;
        }
        // A call that gets no answer takes its whole wait. The scheduled runs are looked at
        // before the first call and between the calls, so none starts that much late.
        self.start_due();
        if now >= self.next_heartbeat || self.outbox.dropped() != self.said_dropped {
            self.heartbeat(now);
            self.start_due();
        }
        if self.ended.is_some() || self.paused_until.is_some() {
            return;
        }
        match &self.stream {
            None if self.next_stream.is_none_or(|at| now >= at) => self.open_stream(now),
            // A stream silent for twice the keep-alive is dead. The node opens a new one.
            Some(stream)
                if now.saturating_duration_since(stream.last_line)
                    > Duration::from_secs(self.numbers.keep_alive * 2) =>
            {
                self.say("The event stream went silent. Opening another.");
                self.stream_failed(now);
            }
            _ => {}
        }
        if self.claim_now || now >= self.next_claim {
            self.claim(now);
            self.start_due();
        }
        if self.ended.is_some() || self.paused_until.is_some() {
            return;
        }
        if self.read_now || self.next_read.is_some_and(|at| now >= at) {
            self.read_schedules(now);
        }
        if self.ended.is_some() || self.paused_until.is_some() {
            return;
        }
        // The scheduled runs go on with the API in reach or out of it.
        self.start_due();
        if self.outbox.waiting() > 0 && now >= self.next_send {
            self.send(now);
        }
    }

    fn run(mut self) -> Ended {
        self.say(&format!(
            "rvrs node {VERSION}, at {}, with the token {}",
            self.api.base(),
            self.api.token_prefix()
        ));
        if self.outbox.unreadable() > 0 {
            self.say(&format!(
                "{} files in the outbox held no result that could be read. They were removed and count as dropped.",
                self.outbox.unreadable()
            ));
        }
        if !self.schedules.is_empty() && !self.schedules.expired(Instant::now()) {
            self.say(&format!(
                "The node starts with the {} it kept.",
                counted(self.schedules.len())
            ));
        }
        // What this start counts as used is written at once, so a node that restarts every few
        // minutes still uses its copy up.
        self.keep_age();
        loop {
            if self.stop.load(Ordering::Relaxed) {
                self.stop_runs();
                self.keep_age();
                return Ended::Stopped;
            }
            if let Some(ended) = self.ended {
                return ended;
            }
            let now = Instant::now();
            self.tick(now);
            // With more to send, the loop looks for an event and turns again without waiting.
            let wait = if self.sends_next(Instant::now()) {
                Duration::ZERO
            } else {
                Duration::from_millis(200)
            };
            match self.receiver.recv_timeout(wait) {
                Ok(Event::Stream(generation, event)) => {
                    self.on_stream(generation, event, Instant::now())
                }
                Ok(Event::Made(made)) => self.on_made(*made),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
        }
    }
}

/// Runs the node until it is told to stop, or until the API no longer knows its token.
pub fn run(settings: Settings) -> Ended {
    let (sender, receiver) = mpsc::channel();
    let now = Instant::now();
    let numbers = Numbers::default();
    // A node with a disk starts from the copy it kept, each schedule at a random time within
    // its interval, so it measures also when the API is out of reach at the start. The copy is
    // as old as its age says, and a file without an age is no copy.
    let mut kept = settings.schedules;
    let schedules = match kept.load() {
        Some((copy, age)) => Schedules::from_kept(&copy, age, now, unix_seconds(), random),
        None => Schedules::none(),
    };
    Node {
        api: Arc::new(settings.api),
        guard: settings.guard,
        outbox: settings.outbox,
        declaration: settings.declaration,
        hands: Arc::new(settings.hands),
        stop: settings.stop,
        log: settings.log,
        count: settings.count,
        boot: word(8),
        name: settings.name,
        id: None,
        revision: None,
        events: None,
        numbers,
        lease: 90,
        runs: HashMap::new(),
        schedules,
        kept,
        scheduled: HashMap::new(),
        read_now: false,
        next_read: None,
        read_window: numbers.reconnect,
        said_expired: false,
        next_age: now + Duration::from_secs(AGE_WRITTEN_EVERY_SECONDS),
        said_unkept: false,
        sender,
        receiver,
        stream: None,
        generation: 0,
        window: numbers.reconnect,
        next_stream: None,
        next_heartbeat: now,
        said_dropped: 0,
        claim_now: false,
        next_claim: now + Duration::from_secs(numbers.claim),
        pending_claim: None,
        claim_window: numbers.reconnect,
        next_send: now,
        send_window: numbers.reconnect,
        paused_until: None,
        ended: None,
    }
    .run()
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
