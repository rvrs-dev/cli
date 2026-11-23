//! The node's loop against an API that lives inside the test: a listener on this machine that
//! answers with the protocol's own examples and with whatever a test tells it to. No packet is
//! sent: the hands that make a run and resolve a name are the test's.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rvrs_core::model::{Outcome, Run};
use rvrs_core::node::address::Allowed;
use rvrs_core::node::setup::Secret;
use rvrs_core::node::{Guard, Limits};
use rvrs_core::node_config::NodeDeclaration;
use rvrs_core::FamilyChoice;
use serde_json::{json, Value};

use super::super::api::Api;
use super::super::outbox::Outbox;
use super::{run, Ended, Hands, Settings};

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schema/node-protocol/fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

const RUN: &str = "0e6d2f0a-8c0e-4f58-9a55-0b8f3f6f1c11";

/// What the fake API answers with.
#[derive(Clone)]
enum Reply {
    Json(u16, Value),
    /// A page from something on the way, such as a load balancer with no server behind it.
    Page(u16),
    /// The connection closes without an answer.
    Nothing,
}

fn refused(status: u16, code: &str) -> Reply {
    Reply::Json(status, json!({ "error": { "code": code, "message": "x" } }))
}

#[derive(Clone)]
struct Call {
    path: String,
    body: Value,
    token: String,
}

/// What the fake answers next. A list that ran out answers with its usual.
struct Script {
    heartbeat: Reply,
    claims: VecDeque<Reply>,
    results: VecDeque<Reply>,
    /// What each new stream writes, in order, and then holds open in silence.
    streams: VecDeque<Vec<String>>,
    /// Lines to write to the stream that is open now.
    write: VecDeque<String>,
}

struct Fake {
    address: String,
    calls: Arc<Mutex<Vec<Call>>>,
    script: Arc<Mutex<Script>>,
    closed: Arc<AtomicBool>,
}

const HELLO: &str =
    "event: hello\ndata: {\"keepAliveSeconds\":1,\"claimSeconds\":2,\"heartbeatSeconds\":3600,\"reconnectSeconds\":1}\n\n";
const WORK: &str = "event: work\ndata: {}\n\n";

fn empty_claim() -> Value {
    json!({ "leaseSeconds": 3, "runs": [], "holding": [] })
}

/// A claim's answer that hands over one run, as the protocol's example does.
fn claim_with(tool: &str, target: &str) -> Reply {
    Reply::Json(
        200,
        json!({
            "leaseSeconds": 3,
            "runs": [{ "run": RUN, "attempt": 1, "tool": tool, "target": target, "options": {} }],
            "holding": [],
        }),
    )
}

fn verdict(word: &str) -> Reply {
    Reply::Json(
        200,
        json!({
            "leaseSeconds": 3,
            "runs": [],
            "holding": [{ "run": RUN, "attempt": 1, "verdict": word }],
        }),
    )
}

impl Fake {
    fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut heartbeat = fixture("heartbeat.response.json");
        // The stream is here, on the API's own address.
        heartbeat.as_object_mut().unwrap().remove("events");
        let script = Arc::new(Mutex::new(Script {
            heartbeat: Reply::Json(200, heartbeat),
            claims: VecDeque::new(),
            results: VecDeque::new(),
            streams: VecDeque::new(),
            write: VecDeque::new(),
        }));
        let closed = Arc::new(AtomicBool::new(false));
        let fake = Fake {
            address,
            calls: Arc::clone(&calls),
            script: Arc::clone(&script),
            closed: Arc::clone(&closed),
        };
        std::thread::spawn(move || {
            for connection in listener.incoming().flatten() {
                if closed.load(Ordering::Relaxed) {
                    return;
                }
                let (calls, script, closed) =
                    (Arc::clone(&calls), Arc::clone(&script), Arc::clone(&closed));
                std::thread::spawn(move || serve(connection, &calls, &script, &closed));
            }
        });
        fake
    }

    fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        self.script.lock().unwrap()
    }

    fn calls_to(&self, path: &str) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.path == path)
            .cloned()
            .collect()
    }

    /// Waits until `met`, for a few seconds at most.
    fn until(&self, met: impl Fn(&Fake) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if met(self) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        met(self)
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

fn answer(connection: &mut TcpStream, status: u16, kind: &str, body: &str) {
    let _ = write!(
        connection,
        "HTTP/1.1 {status} X\r\ncontent-type: {kind}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn serve(
    mut connection: TcpStream,
    calls: &Arc<Mutex<Vec<Call>>>,
    script: &Arc<Mutex<Script>>,
    closed: &Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(connection.try_clone().unwrap());
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let mut parts = first.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or_default().to_string(),
        parts.next().unwrap_or_default().to_string(),
    );
    let (mut length, mut token) = (0usize, String::new());
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        let lower = line.to_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("authorization:") {
            token = line[14..].trim().trim_start_matches("Bearer ").to_string();
        }
    }
    let mut body = vec![0u8; length];
    let _ = reader.read_exact(&mut body);
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    calls.lock().unwrap().push(Call {
        path: path.clone(),
        body,
        token,
    });

    if method == "GET" {
        let Some(lines) = script.lock().unwrap().streams.pop_front() else {
            return answer(&mut connection, 404, "text/html", "<html>Not Found</html>");
        };
        let _ = write!(
            connection,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n"
        );
        for line in lines {
            let _ = connection.write_all(line.as_bytes());
        }
        let _ = connection.flush();
        // The stream stays open, and the test writes to it when it wants to.
        while !closed.load(Ordering::Relaxed) {
            let next = script.lock().unwrap().write.pop_front();
            if let Some(line) = next {
                if connection.write_all(line.as_bytes()).is_err() || connection.flush().is_err() {
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        return;
    }
    let reply = {
        let mut script = script.lock().unwrap();
        match path.as_str() {
            "/v1/nodes/me/heartbeats" => script.heartbeat.clone(),
            "/v1/nodes/me/claims" => script
                .claims
                .pop_front()
                .unwrap_or(Reply::Json(200, empty_claim())),
            "/v1/runs" => script
                .results
                .pop_front()
                .unwrap_or(Reply::Json(201, fixture("result.response.json"))),
            _ => Reply::Page(404),
        }
    };
    match reply {
        Reply::Json(status, body) => answer(
            &mut connection,
            status,
            "application/json",
            &body.to_string(),
        ),
        Reply::Page(status) => answer(&mut connection, status, "text/html", "<html>x</html>"),
        Reply::Nothing => {}
    }
}

/// A node on the fake, with hands that send no packet: a run goes on until it is told to stop,
/// and a name resolves to a public address.
struct Started {
    stop: Arc<AtomicBool>,
    ended: Option<std::thread::JoinHandle<Ended>>,
    log: Arc<Mutex<Vec<String>>>,
    state: PathBuf,
}

impl Started {
    fn stop(mut self) -> Ended {
        self.stop.store(true, Ordering::Relaxed);
        let ended = self.ended.take().unwrap().join().unwrap();
        let _ = std::fs::remove_dir_all(&self.state);
        ended
    }

    fn said(&self, part: &str) -> bool {
        self.log
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(part))
    }
}

fn node(fake: &Fake, name: &str) -> Started {
    let stop = Arc::new(AtomicBool::new(false));
    let log = Arc::new(Mutex::new(Vec::new()));
    let state = std::env::temp_dir().join(format!("rvrs-node-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&state);
    let hands = Hands {
        make: Box::new(|admitted, _destination, stop| {
            let started = Instant::now();
            while !stop.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(6) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let mut run = Run::new(admitted.tool, &admitted.target, FamilyChoice::Auto, 0, 0);
            run.outcome = Some(if stop.load(Ordering::Relaxed) {
                Outcome::Stopped
            } else {
                Outcome::Reached
            });
            run
        }),
        resolve: Box::new(|_, _| Ok("1.1.1.1".parse().unwrap())),
    };
    let lines = Arc::clone(&log);
    let settings = Settings {
        api: Api::new(&fake.address, Secret::new("rvrs_test-token")),
        guard: Guard::new(
            Limits::default(),
            &["traceroute", "ping", "mtr", "dns", "bgp"],
            Allowed::none(),
        ),
        outbox: Outbox::open(&state.join("outbox")),
        declaration: NodeDeclaration::default(),
        name: "this-machine".into(),
        hands,
        stop: Arc::clone(&stop),
        log: Box::new(move |line| lines.lock().unwrap().push(line.to_string())),
        count: Box::new(|_| {}),
    };
    Started {
        stop,
        ended: Some(std::thread::spawn(move || run(settings))),
        log,
        state,
    }
}

#[test]
fn it_says_what_it_is_then_holds_its_stream_and_claims_on_hello() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    let started = node(&fake, "hello");
    assert!(fake.until(|fake| !fake.calls_to("/v1/nodes/me/claims").is_empty()));
    let beat = &fake.calls_to("/v1/nodes/me/heartbeats")[0];
    assert_eq!(beat.token, "rvrs_test-token");
    assert_eq!(beat.body["protocol"], 1);
    assert_eq!(
        beat.body["limits"],
        fixture("heartbeat.request.json")["limits"]
    );
    assert_eq!(beat.body["outbox"]["keeps"], "disk");
    assert_eq!(beat.body["tags"], json!({}));
    // The heartbeat came before the stream, and the claim after hello, with a key of its own.
    let claim = &fake.calls_to("/v1/nodes/me/claims")[0];
    assert_eq!(claim.body["holding"], json!([]));
    assert_eq!(claim.body["room"], 2);
    assert_eq!(claim.body["key"].as_str().unwrap().len(), 16);
    assert!(started.said("The API knows this node as fra-1."));
    assert!(started.said("The event stream is open."));
    // No line it printed has the token.
    assert!(!started.said("test-token"));
    assert_eq!(started.stop(), Ended::Stopped);
}

#[test]
fn a_run_it_refuses_is_answered_with_a_report_that_says_why() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script()
        .claims
        .push_back(claim_with("ping", "192.168.1.1"));
    let started = node(&fake, "refused");
    assert!(fake.until(|fake| !fake.calls_to("/v1/runs").is_empty()));
    let result = &fake.calls_to("/v1/runs")[0].body;
    assert_eq!(result["claim"], json!({ "run": RUN, "attempt": 1 }));
    assert_eq!(result["report"]["outcome"], "refused");
    assert_eq!(result["report"]["tool"], "Ping");
    assert_eq!(result["report"]["target"], "192.168.1.1");
    assert_eq!(result["report"]["origin"]["kind"], "node");
    assert_eq!(result["report"]["origin"]["node"], "fra-1");
    // The revision the API last said travels with the report.
    assert_eq!(result["report"]["origin"]["metadataRevision"], 3);
    assert!(result["startedAgoMilliseconds"].is_u64());
    // The log names the run and the kind of rule, never the target.
    assert!(started.said(&format!("refused run {RUN} attempt 1: target")));
    assert!(!started.said("192.168"));
    started.stop();
}

#[test]
fn a_wake_up_that_never_arrives_is_claimed_anyway() {
    let fake = Fake::start();
    // The stream says hello and then nothing more, no keep-alive either, so it is opened again.
    for _ in 0..6 {
        fake.script().streams.push_back(vec![HELLO.into()]);
    }
    fake.script()
        .claims
        .push_back(Reply::Json(200, empty_claim()));
    fake.script()
        .claims
        .push_back(claim_with("ping", "192.168.1.1"));
    let started = node(&fake, "lost");
    // No `work` is ever written. The node claims by itself and gets the run.
    assert!(fake.until(|fake| !fake.calls_to("/v1/runs").is_empty()));
    assert!(fake.calls_to("/v1/nodes/me/claims").len() >= 2);
    started.stop();
}

#[test]
fn work_on_the_stream_means_claim_now() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    let started = node(&fake, "work");
    assert!(fake.until(|fake| fake.calls_to("/v1/nodes/me/claims").len() == 1));
    fake.script()
        .claims
        .push_back(claim_with("dns", "localhost"));
    let asked = Instant::now();
    fake.script().write.push_back(WORK.into());
    assert!(fake.until(|fake| !fake.calls_to("/v1/runs").is_empty()));
    assert!(
        asked.elapsed() < Duration::from_millis(900),
        "{:?}",
        asked.elapsed()
    );
    started.stop();
}

#[test]
fn a_held_run_is_listed_in_every_claim_and_ends_when_the_api_says_stop() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script()
        .claims
        .push_back(claim_with("ping", "example.com"));
    for _ in 0..2 {
        fake.script().claims.push_back(verdict("keep"));
    }
    fake.script().claims.push_back(verdict("stop"));
    let started = node(&fake, "held");
    assert!(fake.until(|fake| !fake.calls_to("/v1/runs").is_empty()));
    let claims = fake.calls_to("/v1/nodes/me/claims");
    // Every claim after the first lists the run, a third of the lease apart, each with a new key.
    assert!(claims.len() >= 4, "{}", claims.len());
    for claim in &claims[1..4] {
        assert_eq!(claim.body["holding"], json!([{ "run": RUN, "attempt": 1 }]));
        assert_eq!(claim.body["room"], 1);
    }
    assert_ne!(claims[1].body["key"], claims[2].body["key"]);
    let result = &fake.calls_to("/v1/runs")[0].body;
    assert_eq!(result["report"]["outcome"], "stopped");
    assert_eq!(result["report"]["target"], "example.com");
    assert!(started.said(&format!("run {RUN} attempt 1: started")));
    assert!(started.said(&format!("run {RUN} attempt 1: stopped")));
    started.stop();
}

#[test]
fn a_verdict_it_does_not_know_stops_the_run_and_is_named() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script()
        .claims
        .push_back(claim_with("mtr", "example.com"));
    fake.script().claims.push_back(verdict("pause"));
    let started = node(&fake, "unknown");
    assert!(fake.until(|fake| !fake.calls_to("/v1/runs").is_empty()));
    let report = &fake.calls_to("/v1/runs")[0].body["report"];
    assert_eq!(report["outcome"], "stopped");
    assert_eq!(
        report["error"],
        "fra-1 doesn't know the verdict \u{201C}pause\u{201D}, so it stopped the run."
    );
    started.stop();
}

#[test]
fn a_run_the_api_says_to_drop_sends_nothing() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script()
        .claims
        .push_back(claim_with("ping", "example.com"));
    fake.script().claims.push_back(verdict("drop"));
    let started = node(&fake, "drop");
    // The run is gone from what the node holds, and no result was sent for it.
    assert!(fake.until(|fake| {
        fake.calls_to("/v1/nodes/me/claims")
            .iter()
            .skip(2)
            .any(|claim| claim.body["holding"] == json!([]))
    }));
    assert!(fake.calls_to("/v1/runs").is_empty());
    started.stop();
}

#[test]
fn a_result_waits_while_the_api_is_away_and_goes_out_once_it_is_back() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script()
        .claims
        .push_back(claim_with("ping", "192.168.1.1"));
    // The API asks the node to wait, then a load balancer's 404, then no answer at all, then
    // the API again.
    fake.script()
        .results
        .push_back(refused(429, "RATE_LIMITED"));
    fake.script().results.push_back(Reply::Page(404));
    fake.script().results.push_back(Reply::Nothing);
    let started = node(&fake, "away");
    assert!(fake.until(|fake| fake.calls_to("/v1/runs").len() == 4));
    let sent = fake.calls_to("/v1/runs");
    // The same result every time, and still listed as held while it waited.
    assert_eq!(sent[0].body["report"]["id"], sent[3].body["report"]["id"]);
    assert!(fake
        .calls_to("/v1/nodes/me/claims")
        .iter()
        .any(|claim| claim.body["holding"] == json!([{ "run": RUN, "attempt": 1 }])));
    // Once the API had it, it is sent no more.
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(fake.calls_to("/v1/runs").len(), 4);
    started.stop();
}

#[test]
fn a_result_the_api_will_never_take_is_dropped_and_the_next_heartbeat_says_so() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script()
        .claims
        .push_back(claim_with("ping", "192.168.1.1"));
    fake.script()
        .results
        .push_back(refused(422, "VALIDATION_ERROR"));
    let started = node(&fake, "never");
    assert!(fake.until(|fake| {
        fake.calls_to("/v1/nodes/me/heartbeats")
            .iter()
            .any(|beat| beat.body["outbox"]["dropped"] == 1)
    }));
    assert_eq!(fake.calls_to("/v1/runs").len(), 1);
    started.stop();
}

#[test]
fn a_claim_that_got_no_answer_is_sent_again_with_its_key() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script().claims.push_back(Reply::Nothing);
    fake.script().claims.push_back(Reply::Page(502));
    let started = node(&fake, "again");
    assert!(fake.until(|fake| fake.calls_to("/v1/nodes/me/claims").len() >= 3));
    let claims = fake.calls_to("/v1/nodes/me/claims");
    assert_eq!(claims[0].body, claims[1].body);
    assert_eq!(claims[1].body, claims[2].body);
    started.stop();
}

#[test]
fn a_token_the_api_does_not_know_ends_the_runs_and_every_call() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script()
        .claims
        .push_back(claim_with("ping", "example.com"));
    fake.script().claims.push_back(refused(401, "UNAUTHORIZED"));
    let mut started = node(&fake, "revoked");
    let ended = started.ended.take().unwrap().join().unwrap();
    assert_eq!(ended, Ended::NoToken);
    assert!(started.said("The node sends nothing until it is given a new one."));
    // Nothing is called after that: no result for the run it ended, no claim, no heartbeat.
    let calls = fake.calls.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(fake.calls.lock().unwrap().len(), calls);
    assert!(fake.calls_to("/v1/runs").is_empty());
    let _ = std::fs::remove_dir_all(&started.state);
}

#[test]
fn a_stopped_fleet_ends_the_runs_and_keeps_their_results_for_later() {
    let fake = Fake::start();
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script()
        .claims
        .push_back(claim_with("ping", "example.com"));
    fake.script()
        .claims
        .push_back(Reply::Json(403, fixture("claim.stopped.json")));
    let started = node(&fake, "fleet");
    assert!(fake.until(|_| started.said(&format!("run {RUN} attempt 1: stopped"))));
    assert!(started.said("An admin stopped this workspace's fleet."));
    // The node asks again in an hour. Until then it calls nothing, and the result waits.
    let calls = fake.calls.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(fake.calls.lock().unwrap().len(), calls);
    assert!(fake.calls_to("/v1/runs").is_empty());
    let waiting = std::fs::read_dir(started.state.join("outbox"))
        .unwrap()
        .count();
    assert_eq!(waiting, 1);
    started.stop();
}

#[test]
fn the_stream_is_opened_where_the_heartbeats_answer_says() {
    let fake = Fake::start();
    let mut answer = fixture("heartbeat.response.json");
    answer["events"] = format!("{}/streams/held-elsewhere", fake.address).into();
    fake.script().heartbeat = Reply::Json(200, answer);
    fake.script().streams.push_back(vec![HELLO.into()]);
    let started = node(&fake, "events");
    assert!(fake.until(|fake| !fake.calls_to("/streams/held-elsewhere").is_empty()));
    assert_eq!(
        fake.calls_to("/streams/held-elsewhere")[0].token,
        "rvrs_test-token"
    );
    assert!(fake.calls_to("/v1/nodes/me/events").is_empty());
    started.stop();
}

#[test]
fn a_stream_that_goes_silent_is_given_up_and_opened_again() {
    let fake = Fake::start();
    // Two streams that say hello and then nothing. The keep-alive is one second.
    fake.script().streams.push_back(vec![HELLO.into()]);
    fake.script().streams.push_back(vec![HELLO.into()]);
    let started = node(&fake, "silent");
    assert!(fake.until(|fake| fake.calls_to("/v1/nodes/me/events").len() >= 2));
    assert!(started.said("The event stream went silent. Opening another."));
    started.stop();
}

#[test]
fn a_network_that_lets_no_stream_through_still_gets_its_runs() {
    let fake = Fake::start();
    // No stream is ever served: every attempt gets a page.
    fake.script()
        .claims
        .push_back(claim_with("ping", "192.168.1.1"));
    let started = node(&fake, "nostream");
    assert!(fake.until(|fake| !fake.calls_to("/v1/runs").is_empty()));
    assert!(!fake.calls_to("/v1/nodes/me/events").is_empty());
    started.stop();
}
