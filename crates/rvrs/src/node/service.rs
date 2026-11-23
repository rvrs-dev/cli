//! The node's loop: what `rvrs node` does once it has a token.
//!
//! It follows the protocol's page (`schema/node-protocol/README.md`). A heartbeat says what the
//! node is. One event stream is held open, and `work` on it means claim now. A claim says every
//! run the node holds and how many more it takes, and gets the runs that wait. Each run is made
//! within the node's own limits and its result waits in the outbox until the API has answered
//! for it.
//!
//! Nothing here trusts a wake-up to arrive. The node claims after every `hello` and every `work`,
//! every third of the lease while it holds a run, after a stream that failed to open, and at a
//! random time while it has nothing to do. Every number it waits by comes from the API, as a
//! length in seconds, and is counted on a clock that only goes forward.
//!
//! No line it prints carries a token, a target or anything from a report.

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
use rvrs_core::node::{Guard, Heartbeat, Outbox as SaidOutbox, Stopped, System, PROTOCOL};
use rvrs_core::node_config::NodeDeclaration;
use rvrs_core::report::Report;
use rvrs_core::resolve::{self, ResolveError};
use rvrs_core::FamilyChoice;
use serde_json::{json, Value};

use super::api::{Answer, Api};
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
/// node. The six characters a run gets on a person's device repeat within some ten thousand
/// runs, and the API would refuse the second report.
fn report_id(request: &Request) -> String {
    format!("{}-{}", request.run, request.attempt)
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

    /// Ends every run the node is making. Their results still go to the outbox.
    fn stop_runs(&mut self) {
        for held in self.runs.values() {
            held.stop.store(true, Ordering::Relaxed);
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
                self.ended = Some(Ended::NoToken);
                self.say("The API doesn't know this node's token, or the token was revoked. The node sends nothing until it is given a new one.");
                true
            }
            (403, "FLEET_STOPPED") => {
                self.stop_runs();
                self.close_stream();
                self.paused_until = Some(now + ASK_AGAIN);
                self.say("An admin stopped this workspace's fleet. The node ended its runs, keeps its results and asks again in an hour.");
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
                self.claim_now = true;
                self.next_send = now;
                self.say("The event stream is open.");
            }
            StreamEvent::Work => self.claim_now = true,
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
        let holds = !self.runs.is_empty() || self.outbox.waiting() > 0;
        self.next_claim = now
            + if holds {
                Duration::from_secs((self.lease / 3).max(1))
            } else {
                Duration::from_secs(self.numbers.claim / 2) + within(self.numbers.claim / 2)
            };
    }

    fn keep(&mut self, report: &Report, request: &Request, started: Instant) {
        let body = json!({
            "report": report,
            "claim": { "run": request.run, "attempt": request.attempt },
        });
        if !self.outbox.put(body, Some(started)) {
            self.say("The outbox's folder can't be written. This result waits in memory and is lost if the node stops before the API has it.");
        }
        self.next_send = Instant::now();
    }

    fn refuse(&mut self, refusal: &Refusal, request: &Request, started: Instant) {
        self.say(&refusal.log_line(request));
        let id = report_id(request);
        let mut report = refusal.report(request, &self.identity(), id, unix_seconds());
        report.origin.metadata_revision = self.revision;
        self.keep(&report, request, started);
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
            Err(refusal) => return self.refuse(&refusal, &request, now),
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
        let guard = self.guard.clone();
        let hands = Arc::clone(&self.hands);
        let sender = self.sender.clone();
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
                started: now,
                what,
            })));
        });
    }

    /// A run thread ended. Its report goes to the outbox, unless the API said to drop the run.
    fn on_made(&mut self, made: Made) {
        self.guard.finish();
        let key = (made.request.run.clone(), made.request.attempt);
        let Some(held) = self.runs.remove(&key) else {
            return;
        };
        if held.dropped {
            return;
        }
        let run = match made.what {
            Ok(run) => run,
            Err(refusal) => return self.refuse(&refusal, &made.request, made.started),
        };
        (self.count)(Counted::Run(run.tool));
        let mut report = Report::of(&run);
        report.id = report_id(&made.request);
        // The report names the target as it was asked, whatever the tool made of it.
        report.target = made.request.target.clone();
        report.origin.kind = "node".into();
        report.origin.node = Some(self.name.clone());
        report.origin.node_id = self.id.clone();
        report.origin.metadata_revision = self.revision;
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
        self.keep(&report, &made.request, made.started);
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
        if let Some(until) = self.paused_until {
            if now < until {
                return;
            }
            self.paused_until = None;
            self.next_heartbeat = now;
            self.claim_now = true;
        }
        if now >= self.next_heartbeat || self.outbox.dropped() != self.said_dropped {
            self.heartbeat(now);
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
        }
        if self.ended.is_some() || self.paused_until.is_some() {
            return;
        }
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
        loop {
            if self.stop.load(Ordering::Relaxed) {
                self.stop_runs();
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
