//! A run asked of a node: `--node <name>` on a tool, and `rvrs nodes`.
//!
//! The tool's own flags are read as always and travel as the protocol's options
//! (`schema/node-protocol/README.md`, What a run is). A flag the protocol has no option for is
//! refused, since a run that left it out would measure something else than was asked. The API
//! answers with the run's id at once. The command then reads the run until it has ended, says
//! where it stands, and shows the result from its report, as it shows any run another device
//! made.

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use rvrs_core::dns;
use rvrs_core::engine::{MtrOptions, Options, PingOptions};
use rvrs_core::format;
use rvrs_core::model::Run;
use rvrs_core::report::Report;
use rvrs_core::FamilyChoice;
use serde_json::{json, Map, Value};

use crate::account;
use crate::args::{Common, TraceArgs};
use crate::dig::DigArgs;
use crate::mtr::MtrArgs;
use crate::ping::PingArgs;
use crate::style::{ColorChoice, Style};
use crate::{fail, term, trace, usage};

/// How long one read of a run waits for its state to change.
const WAIT_SECONDS: u64 = 25;

/// The flags of a run from a node that belong to no tool: `--no-wait` and `--expires-in`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Waiting {
    /// Print the run's id and leave.
    pub no_wait: bool,
    /// How long the request may wait for its node, in seconds.
    pub expires_in: Option<u64>,
}

static WAITING: OnceLock<Waiting> = OnceLock::new();

/// Takes `--no-wait` and `--expires-in` out of a tool's arguments, so the tool's own parser
/// never meets them. They mean something only with `--node`.
pub fn take_flags(arguments: &mut Vec<String>) -> Result<(), String> {
    let mut waiting = Waiting::default();
    let mut kept = Vec::with_capacity(arguments.len());
    let mut at = 0;
    while at < arguments.len() {
        let argument = arguments[at].clone();
        at += 1;
        let value = if argument == "--no-wait" {
            waiting.no_wait = true;
            continue;
        } else if argument == "--expires-in" {
            let value = arguments.get(at).cloned();
            at += 1;
            value
        } else if let Some(value) = argument.strip_prefix("--expires-in=") {
            Some(value.to_string())
        } else {
            kept.push(argument);
            continue;
        };
        let seconds = value
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| (30..=86_400).contains(seconds));
        let Some(seconds) = seconds else {
            return Err("--expires-in takes a number of seconds from 30 to 86400.".into());
        };
        waiting.expires_in = Some(seconds);
    }
    let with_node = kept
        .iter()
        .any(|argument| argument == "--node" || argument.starts_with("--node="));
    if (waiting.no_wait || waiting.expires_in.is_some()) && !with_node {
        return Err("--no-wait and --expires-in go with --node.".into());
    }
    *arguments = kept;
    let _ = WAITING.set(waiting);
    Ok(())
}

/// The flags of waiting that were typed. A command that asks no node for a run refuses them.
pub fn waiting() -> Waiting {
    WAITING.get().copied().unwrap_or_default()
}

fn milliseconds(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn family(options: &mut Map<String, Value>, family: FamilyChoice) {
    match family {
        FamilyChoice::V4 => options.insert("family".into(), "IPv4".into()),
        FamilyChoice::V6 => options.insert("family".into(), "IPv6".into()),
        FamilyChoice::Auto => None,
    };
}

/// What a tool's flags say when one of them can't travel to a node.
fn stays(tool: &str, travel: &str) -> String {
    format!("A {tool} from a node takes {travel}. The other flags don't travel to a node yet.")
}

/// The options of a traceroute as the protocol names them. Only what the person set travels:
/// an option left out takes the tool's default on the node.
pub fn traceroute(args: &TraceArgs) -> Result<Value, String> {
    let usual = Options::default();
    let (set, kept) = (&args.options, &usual);
    if set.spacing != kept.spacing || set.payload != kept.payload || args.as_lookups {
        return Err(stays(
            "traceroute",
            "-4, -6, -q, -w, -m, -f, -T, -p, --mtu, -n and --no-lookups",
        ));
    }
    let mut options = Map::new();
    family(&mut options, args.family);
    if set.queries != kept.queries {
        options.insert("rounds".into(), set.queries.into());
    }
    if set.wait != kept.wait {
        options.insert("waitMilliseconds".into(), milliseconds(set.wait).into());
    }
    if set.max_ttl != kept.max_ttl {
        options.insert("maxHops".into(), set.max_ttl.into());
    }
    if set.first_ttl != kept.first_ttl {
        options.insert("firstHop".into(), set.first_ttl.into());
    }
    if let Some(port) = args.tcp {
        options.insert("probe".into(), json!({ "protocol": "tcp", "port": port }));
    }
    if args.mtu {
        options.insert("mtu".into(), true.into());
    }
    if args.numeric || args.no_lookups {
        options.insert("lookups".into(), false.into());
    }
    Ok(Value::Object(options))
}

pub fn ping(args: &PingArgs) -> Result<Value, String> {
    let usual = PingOptions::default();
    let (set, kept) = (&args.options, &usual);
    if set.payload != kept.payload || set.ttl != kept.ttl || set.deadline.is_some() {
        return Err(stays("ping", "-4, -6, -c, -i, -W, -n and --no-lookups"));
    }
    let mut options = Map::new();
    family(&mut options, args.family);
    if let Some(count) = set.count {
        options.insert("packets".into(), count.into());
    }
    if set.interval != kept.interval {
        options.insert(
            "intervalMilliseconds".into(),
            milliseconds(set.interval).into(),
        );
    }
    if set.wait != kept.wait {
        options.insert("waitMilliseconds".into(), milliseconds(set.wait).into());
    }
    if args.numeric || args.no_lookups {
        options.insert("lookups".into(), false.into());
    }
    Ok(Value::Object(options))
}

pub fn mtr(args: &MtrArgs) -> Result<Value, String> {
    let usual = MtrOptions::default();
    let (set, kept) = (&args.options, &usual);
    if set.spacing != kept.spacing
        || set.payload != kept.payload
        || set.max_unknown != kept.max_unknown
    {
        return Err(stays(
            "MTR",
            "-4, -6, -c, -i, -Z, -m, -f, -n and --no-lookups",
        ));
    }
    let mut options = Map::new();
    family(&mut options, args.family);
    if let Some(count) = set.count {
        options.insert("rounds".into(), count.into());
    }
    if set.interval != kept.interval {
        options.insert(
            "intervalMilliseconds".into(),
            milliseconds(set.interval).into(),
        );
    }
    if set.wait != kept.wait {
        options.insert("waitMilliseconds".into(), milliseconds(set.wait).into());
    }
    if set.max_ttl != kept.max_ttl {
        options.insert("maxHops".into(), set.max_ttl.into());
    }
    if set.first_ttl != kept.first_ttl {
        options.insert("firstHop".into(), set.first_ttl.into());
    }
    if args.numeric || args.no_lookups {
        options.insert("lookups".into(), false.into());
    }
    Ok(Value::Object(options))
}

pub fn dig(args: &DigArgs) -> Result<Value, String> {
    // A record type alone travels. A server or a +option asks one question in dig's own way,
    // which a node doesn't do.
    let own_way = args.one_question && args.type_code.is_none();
    if args.server.is_some() || args.trace || args.tcp || args.short || own_way {
        return Err(stays("DNS lookup", "the record type and --resolver"));
    }
    let mut options = Map::new();
    if let Some(code) = args.type_code {
        options.insert("type".into(), dns::type_name(code).into());
    }
    if let Some(resolver) = args.resolvers.first() {
        options.insert("resolver".into(), resolver.to_string().into());
    }
    Ok(Value::Object(options))
}

/// A BGP lookup takes no options.
pub fn bgp() -> Value {
    json!({})
}

/// The sentence of the API's error, or one that names the status when there is none.
pub(crate) fn refusal(status: u16, body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("The rvrs API answered {status}."))
}

/// How long ago the API last heard from a node, for a sentence.
fn last_seen(node: &Value) -> String {
    match node["lastSeenAt"].as_str().and_then(format::seconds_of) {
        Some(then) => {
            let ago = format::relative(then, format::now());
            format!("It was last seen {}.", ago.to_lowercase())
        }
        None => "It has never connected.".to_string(),
    }
}

/// What an expired request says: the node's name and how long the request waited.
fn expired(run: &Value) -> String {
    let name = run["node"]["name"].as_str().unwrap_or("The node");
    let waited = match (
        run["requestedAt"].as_str().and_then(format::seconds_of),
        run["expiresAt"].as_str().and_then(format::seconds_of),
    ) {
        (Some(from), Some(to)) => to.saturating_sub(from),
        _ => 300,
    };
    if run["reason"].as_str() == Some("abandoned") {
        return format!(
            "{name} claimed the run and sent no result, so the request expired. {}",
            last_seen(&run["node"])
        );
    }
    format!(
        "{name} didn't claim the run within {waited} s, so the request expired. {}",
        last_seen(&run["node"])
    )
}

/// The line that says where a run stands while the command waits.
fn standing(run: &Value) -> String {
    let name = run["node"]["name"].as_str().unwrap_or("the node");
    match run["state"].as_str().unwrap_or_default() {
        "requested" if run["node"]["online"].as_bool() == Some(false) => {
            format!("Requested \u{00B7} {name} is offline")
        }
        "requested" => format!("Requested \u{00B7} {name}"),
        "running" => format!("Running on {name}"),
        // A state this version doesn't know is shown as the word it is.
        other => format!("{other} \u{00B7} {name}"),
    }
}

/// Asks for one answer on a thread of its own, so Ctrl-C ends the waiting at once and never
/// the run.
fn asked(
    method: &'static str,
    path: String,
    body: Option<Value>,
    seconds: u64,
) -> Option<Result<(u16, Value), String>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(account::call(method, &path, body.as_ref(), seconds));
    });
    loop {
        if term::interrupts() > 0 {
            return None;
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(answer) => return Some(answer),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Some(Err(
                    "The request to the rvrs API ended without an answer.".into()
                ))
            }
        }
    }
}

/// Asks `node` for a run and shows it once it has ended.
pub fn run(node: &str, tool: &str, target: &str, options: Value, common: &Common) -> ExitCode {
    if account::token().is_none() {
        return fail("Asking a node for a run takes an account. Sign in with rvrs login.");
    }
    let waiting = waiting();
    let mut request = json!({
        "node": node,
        "tool": tool,
        "target": target,
        "options": options,
        // The key makes asking again safe: the same request gets the run it made the first time.
        "key": crate::node::word(16),
    });
    if let Some(seconds) = waiting.expires_in {
        request["expiresIn"] = seconds.into();
    }
    term::catch_interrupts();
    let mut sent = None;
    for _ in 0..3 {
        match asked("POST", "/v1/run-requests".into(), Some(request.clone()), 20) {
            None => return ExitCode::from(130),
            Some(Ok(answer)) => {
                sent = Some(answer);
                break;
            }
            Some(Err(message)) => sent = Some((0, json!({ "error": { "message": message } }))),
        }
    }
    let Some((status, mut run)) = sent else {
        return fail("Couldn't reach the rvrs API.");
    };
    match status {
        200 | 201 => {}
        404 => {
            return fail(&format!(
            "This workspace has no node called \u{201C}{node}\u{201D}. List them with rvrs nodes."
        ))
        }
        401 => return fail("Asking a node for a run takes an account. Sign in with rvrs login."),
        403 => {
            return fail(
                "A token is a node's credential and can't ask for a run. Sign in with rvrs login.",
            )
        }
        _ => return fail(&refusal(status, &run)),
    }
    let Some(id) = run["id"].as_str().map(str::to_string) else {
        return fail("The rvrs API answered without the run's id.");
    };
    if waiting.no_wait {
        println!("{id}");
        return ExitCode::SUCCESS;
    }

    let style = Style::detect(common.color);
    let says = std::io::stderr().is_terminal() && !common.json;
    let mut said = String::new();
    loop {
        let state = run["state"].as_str().unwrap_or_default().to_string();
        if state == "ended" {
            break;
        }
        if state == "expired" {
            return fail(&expired(&run));
        }
        let line = standing(&run);
        if says && line != said {
            eprintln!("{}", Style::detect_stderr().dim(&line));
            said = line;
        }
        // A state this version doesn't know is read until the request's time is over.
        let over = run["expiresAt"]
            .as_str()
            .and_then(format::seconds_of)
            .is_some_and(|until| format::now() > until + 3600);
        if !matches!(state.as_str(), "requested" | "running") && over {
            return fail(&format!(
                "The run is \u{201C}{state}\u{201D}, which this rvrs doesn't know. Its id is {id}."
            ));
        }
        let path = format!("/v1/run-requests/{id}?wait={WAIT_SECONDS}");
        match asked("GET", path, None, WAIT_SECONDS + 15) {
            // Ctrl-C ends the waiting. The run goes on, and its id finds it later.
            None => {
                eprintln!("The run goes on. Its id is {id}.");
                return ExitCode::from(130);
            }
            Some(Ok((200, read))) => run = read,
            Some(Ok((status, body))) if status == 404 || status == 401 || status == 403 => {
                return fail(&refusal(status, &body))
            }
            // The API is out of reach for a moment. The run is read again.
            Some(_) => std::thread::sleep(Duration::from_secs(2)),
        }
    }

    // The account has the run now. A sync brings it into history on this device.
    crate::sync::start();
    let report_json = run["report"].clone();
    if common.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report_json).unwrap_or_default()
        );
    }
    let Ok(report) = serde_json::from_value::<Report>(report_json) else {
        return fail(
            "The run ended with a report this rvrs can't read. rvrs update gets a newer one.",
        );
    };
    if matches!(report.outcome.as_str(), "refused" | "failed") {
        let why = report.error.clone().unwrap_or(report.summary.clone());
        return fail(&why);
    }
    if common.json {
        return ExitCode::SUCCESS;
    }
    let from = run["node"]["name"].as_str().map(str::to_string);
    match Run::from_report(&report, &report.id, from) {
        Some(shown) => trace::show(&shown, false, common.classic, style),
        // A tool this version doesn't know still has its line of facts.
        None => println!("{}", report.summary),
    }
    ExitCode::SUCCESS
}

/// `rvrs nodes`: the workspace's nodes, with whether each is online.
pub fn nodes(arguments: &[String]) -> ExitCode {
    let mut json = false;
    for argument in arguments {
        match argument.as_str() {
            "--json" => json = true,
            other => {
                return usage(&format!(
                    "rvrs nodes takes --json and nothing else, and got \u{201C}{other}\u{201D}."
                ))
            }
        }
    }
    if account::token().is_none() {
        return fail("Listing nodes takes an account. Sign in with rvrs login.");
    }
    let mut listed: Vec<Value> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let path = match &cursor {
            Some(cursor) => format!("/v1/nodes?limit=200&cursor={cursor}"),
            None => "/v1/nodes?limit=200".to_string(),
        };
        let (status, body) = match account::call("GET", &path, None, 20) {
            Ok(answer) => answer,
            Err(message) => return fail(&message),
        };
        match status {
            200 => {}
            403 => return fail(
                "A token is a node's credential and reads no other node. Sign in with rvrs login.",
            ),
            _ => return fail(&refusal(status, &body)),
        }
        listed.extend(body["nodes"].as_array().cloned().unwrap_or_default());
        cursor = body["next"].as_str().map(str::to_string);
        if cursor.is_none() {
            break;
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&listed).unwrap_or_else(|_| "[]".into())
        );
        return ExitCode::SUCCESS;
    }
    if listed.is_empty() {
        println!("No nodes yet. Make a token for one under Installs at app.rvrs.dev.");
        return ExitCode::SUCCESS;
    }
    let style = Style::detect(ColorChoice::Auto);
    for line in node_lines(&listed, style, format::now()) {
        println!("{line}");
    }
    ExitCode::SUCCESS
}

/// The Nodes line of `rvrs status`: how many nodes the workspace has and how many are online.
/// True when one can be asked for a run now.
pub fn status_line() -> (bool, String) {
    if account::token().is_none() {
        return (
            false,
            "runs come from this device \u{00B7} sign in to ask a node".into(),
        );
    }
    match account::call("GET", "/v1/nodes?limit=200", None, 6) {
        Ok((200, body)) => nodes_line(body["nodes"].as_array().map_or(&[][..], Vec::as_slice)),
        // A token is a node's own credential. It asks no other node.
        Ok((403, _)) => (false, "this credential is a node's own".into()),
        _ => (false, "couldn't ask the rvrs API for the nodes".into()),
    }
}

fn nodes_line(nodes: &[Value]) -> (bool, String) {
    let online = nodes
        .iter()
        .filter(|node| node["online"].as_bool() == Some(true))
        .count();
    let count = match nodes.len() {
        0 => {
            return (
                false,
                "no nodes yet \u{00B7} make a token for one at app.rvrs.dev".into(),
            )
        }
        1 => "1 node".to_string(),
        many => format!("{many} nodes"),
    };
    (online > 0, format!("{count} \u{00B7} {online} online"))
}

/// One line per node under a header: name, whether it is online or when it was last seen, its
/// version and its tags.
pub fn node_lines(nodes: &[Value], style: Style, now: u64) -> Vec<String> {
    let text = |node: &Value, key: &str| node[key].as_str().unwrap_or("\u{2013}").to_string();
    let seen = |node: &Value| {
        if node["online"].as_bool() == Some(true) {
            return "Online".to_string();
        }
        match node["lastSeenAt"].as_str().and_then(format::seconds_of) {
            Some(then) => format::relative(then, now),
            None => "Never".to_string(),
        }
    };
    let tags = |node: &Value| {
        node["tags"]
            .as_object()
            .map(|tags| {
                tags.iter()
                    .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    };
    let widest = |texts: Vec<usize>, floor: usize| texts.into_iter().max().unwrap_or(0).max(floor);
    let name_width = widest(
        nodes.iter().map(|node| text(node, "name").len()).collect(),
        4,
    );
    let seen_width = widest(nodes.iter().map(|node| seen(node).len()).collect(), 4);
    let version_width = widest(
        nodes
            .iter()
            .map(|node| text(node, "version").len())
            .collect(),
        7,
    );
    let mut lines = vec![style.dim(&format!(
        "{:<name_width$}  {:<seen_width$}  {:<version_width$}  TAGS",
        "NODE", "SEEN", "VERSION"
    ))];
    for node in nodes {
        let state = format!("{:<seen_width$}", seen(node));
        let state = if node["online"].as_bool() == Some(true) {
            style.green(&state)
        } else {
            style.dim(&state)
        };
        lines.push(
            format!(
                "{:<name_width$}  {state}  {:<version_width$}  {}",
                text(node, "name"),
                text(node, "version"),
                tags(node)
            )
            .trim_end()
            .to_string(),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn only_what_the_person_set_travels() {
        let crate::args::Parsed::Run(plain) = crate::args::parse(&words("example.com")).unwrap()
        else {
            panic!("a run")
        };
        assert_eq!(traceroute(&plain).unwrap(), json!({}));
        let crate::args::Parsed::Run(set) = crate::args::parse(&words(
            "-4 -q 5 -w 2 -m 20 -f 2 -T -p 443 --mtu -n example.com",
        ))
        .unwrap() else {
            panic!("a run")
        };
        assert_eq!(
            traceroute(&set).unwrap(),
            json!({
                "family": "IPv4",
                "rounds": 5,
                "waitMilliseconds": 2000,
                "maxHops": 20,
                "firstHop": 2,
                "probe": { "protocol": "tcp", "port": 443 },
                "mtu": true,
                "lookups": false,
            })
        );
    }

    #[test]
    fn ping_and_mtr_name_their_count_as_the_protocol_does() {
        let crate::ping::Parsed::Run(ping_args) =
            crate::ping::parse(&words("-c 20 -i 0.5 -6 example.com")).unwrap()
        else {
            panic!("a run")
        };
        assert_eq!(
            ping(&ping_args).unwrap(),
            json!({ "family": "IPv6", "packets": 20, "intervalMilliseconds": 500 })
        );
        let crate::mtr::Parsed::Run(mtr_args) =
            crate::mtr::parse(&words("-c 12 -m 20 example.com")).unwrap()
        else {
            panic!("a run")
        };
        assert_eq!(
            mtr(&mtr_args).unwrap(),
            json!({ "rounds": 12, "maxHops": 20 })
        );
    }

    #[test]
    fn a_dns_lookup_takes_its_record_type_along() {
        let parsed = |line: &str| match crate::dig::parse(&words(line), false).unwrap() {
            crate::dig::Parsed::Run(args) => args,
            _ => panic!("a run"),
        };
        assert_eq!(dig(&parsed("example.com")).unwrap(), json!({}));
        assert_eq!(
            dig(&parsed("example.com AAAA")).unwrap(),
            json!({ "type": "AAAA" })
        );
        assert!(dig(&parsed("@9.9.9.9 example.com")).is_err());
        assert!(dig(&parsed("example.com +short")).is_err());
    }

    #[test]
    fn a_flag_without_an_option_of_its_own_is_refused() {
        let crate::ping::Parsed::Run(sized) =
            crate::ping::parse(&words("-s 1400 example.com")).unwrap()
        else {
            panic!("a run")
        };
        assert_eq!(
            ping(&sized).unwrap_err(),
            "A ping from a node takes -4, -6, -c, -i, -W, -n and --no-lookups. The other flags don't travel to a node yet."
        );
    }

    #[test]
    fn the_flags_of_waiting_are_taken_before_a_tool_reads_its_own() {
        let mut arguments = words("example.com --node fra-1 --no-wait --expires-in 60 -c 5");
        take_flags(&mut arguments).unwrap();
        assert_eq!(arguments, words("example.com --node fra-1 -c 5"));
        let mut alone = words("example.com --no-wait");
        assert_eq!(
            take_flags(&mut alone).unwrap_err(),
            "--no-wait and --expires-in go with --node."
        );
        let mut short = words("example.com --node fra-1 --expires-in 5");
        assert!(take_flags(&mut short).is_err());
    }

    #[test]
    fn a_request_that_expired_says_who_and_how_long() {
        let run = json!({
            "state": "expired",
            "reason": "unclaimed",
            "node": { "name": "cpe-4471", "online": false, "lastSeenAt": null },
            "requestedAt": "2026-11-22T20:40:00.000Z",
            "expiresAt": "2026-11-22T20:45:00.000Z",
        });
        assert_eq!(
            expired(&run),
            "cpe-4471 didn't claim the run within 300 s, so the request expired. It has never connected."
        );
        assert_eq!(
            standing(&json!({ "state": "requested", "node": { "name": "fra-1", "online": true } })),
            "Requested \u{00B7} fra-1"
        );
        assert_eq!(
            standing(&json!({ "state": "running", "node": { "name": "fra-1" } })),
            "Running on fra-1"
        );
    }

    #[test]
    fn the_status_line_counts_the_nodes_and_who_is_online() {
        assert_eq!(
            nodes_line(&[]),
            (
                false,
                "no nodes yet \u{00B7} make a token for one at app.rvrs.dev".into()
            )
        );
        let nodes = [
            json!({ "name": "fra-1", "online": true }),
            json!({ "name": "cpe-4471", "online": false }),
        ];
        assert_eq!(
            nodes_line(&nodes),
            (true, "2 nodes \u{00B7} 1 online".into())
        );
        assert!(!nodes_line(&nodes[1..]).0);
    }

    #[test]
    fn the_node_list_says_who_is_online() {
        let nodes = vec![
            json!({ "name": "cpe-4471", "online": false, "lastSeenAt": "2026-11-22T17:00:00.000Z",
                "version": "0.14.2", "tags": { "isp": "telia" } }),
            json!({ "name": "fra-1", "online": true, "lastSeenAt": null, "version": null, "tags": {} }),
        ];
        let now = format::seconds_of("2026-11-22T20:00:00.000Z").unwrap();
        let lines = node_lines(&nodes, Style::detect(ColorChoice::Never), now);
        assert_eq!(
            lines,
            [
                "NODE      SEEN    VERSION  TAGS",
                "cpe-4471  3h ago  0.14.2   isp=telia",
                "fra-1     Online  \u{2013}",
            ]
        );
    }
}
