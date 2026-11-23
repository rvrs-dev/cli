//! Schedules: `rvrs schedule` and `rvrs schedules`.
//!
//! A schedule is a run a node repeats at an interval (`schema/node-protocol/README.md`,
//! Schedules). A person who is signed in makes one here, the API hands each node its copy, and
//! the nodes repeat the run by themselves, with the API in reach or not.
//!
//! The tool's own flags are read by the tool's own parser and travel as the protocol's options,
//! through the code `--node` uses (`remote.rs`). So a schedule takes the flags a run from a node
//! takes, and refuses the ones that run refuses.

use std::process::ExitCode;

use rvrs_core::format;
use serde_json::{json, Map, Value};

use crate::account;
use crate::args::Common;
use crate::remote;
use crate::style::{ColorChoice, Style};
use crate::{fail, usage};

/// The shortest and the longest interval a schedule takes, in seconds.
const EVERY: std::ops::RangeInclusive<u64> = 60..=86_400;

const NO_ACCOUNT: &str = "Schedules take an account. Sign in with rvrs login.";
const NODES_TOKEN: &str =
    "A token is a node's credential and can't make or read a schedule. Sign in with rvrs login.";
const WHERE: &str = "A schedule runs on the nodes --node names, on the nodes a selector matches with --fleet, or on every node with --all-nodes. Give one of the three.";
const ADD_USAGE: &str = "rvrs schedule add takes a tool and a target, as in rvrs schedule add ping example.com --every 5m --node fra-1.";

/// What follows `rvrs schedule add`, with the schedule's own flags taken out.
#[derive(Debug, Clone, Default, PartialEq)]
struct Add {
    /// `--every`, in seconds.
    every: Option<u64>,
    /// Each `--node`.
    nodes: Vec<String>,
    /// `--fleet <selector>`.
    selector: Option<String>,
    /// `--all-nodes`.
    all_nodes: bool,
    /// `--name`.
    name: Option<String>,
    json: bool,
    /// The tool and what its own parser reads.
    rest: Vec<String>,
}

/// An interval as `--every` takes it: seconds, or a number with `s`, `m` or `h`.
fn interval(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let (number, unit) = match text.char_indices().last() {
        Some((at, 's')) => (&text[..at], 1),
        Some((at, 'm')) => (&text[..at], 60),
        Some((at, 'h')) => (&text[..at], 3600),
        _ => (text, 1),
    };
    number
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(unit))
        .filter(|seconds| EVERY.contains(seconds))
        .ok_or_else(|| {
            "--every takes an interval from 60 seconds to 24 hours, as in --every 5m or --every 300."
                .to_string()
        })
}

/// An interval as a line shows it: `90s`, `5m`, `2h`.
fn spoken(seconds: u64) -> String {
    match seconds {
        0 => "0s".to_string(),
        hours if hours % 3600 == 0 => format!("{}h", hours / 3600),
        minutes if minutes % 60 == 0 => format!("{}m", minutes / 60),
        seconds => format!("{seconds}s"),
    }
}

/// Takes the schedule's own flags out of what follows `add`, so the tool's parser never meets
/// them. `--node` and `--fleet` mean something else to a tool on its own.
fn take_add_flags(arguments: &[String]) -> Result<Add, String> {
    let mut add = Add::default();
    let mut at = 0;
    while at < arguments.len() {
        let argument = &arguments[at];
        at += 1;
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
            _ => (argument.as_str(), None),
        };
        let wants = match name {
            "--every" => "an interval, as in --every 5m",
            "--node" => "the name of a node",
            "--fleet" => "a selector, as in --fleet isp=telia. --all-nodes takes every node",
            "--name" => "a name, as in --name \"Evening jitter\"",
            "--all-nodes" => {
                add.all_nodes = true;
                continue;
            }
            "--json" => {
                add.json = true;
                continue;
            }
            _ => {
                add.rest.push(argument.clone());
                continue;
            }
        };
        let value = match inline {
            Some(value) => value,
            None => {
                let next = arguments.get(at).filter(|next| !next.starts_with("--"));
                let Some(next) = next else {
                    return Err(format!("{name} takes {wants}."));
                };
                at += 1;
                next.clone()
            }
        };
        let value = value.trim().to_string();
        if value.is_empty() {
            return Err(format!("{name} takes {wants}."));
        }
        match name {
            "--every" => add.every = Some(interval(&value)?),
            "--node" => add.nodes.push(value),
            "--fleet" => add.selector = Some(value),
            _ => {
                if value.chars().count() > 80 {
                    return Err("--name takes a name of 80 characters at most.".into());
                }
                add.name = Some(value);
            }
        }
    }
    Ok(add)
}

/// rvrs's own flags of a run that a schedule has no use for.
fn of_a_run(common: &Common) -> Result<(), String> {
    if common.classic || common.case.is_some() {
        return Err("--classic and --case go with a run. A schedule takes neither.".into());
    }
    Ok(())
}

/// The tool, the target and the options of a schedule, read by the tool's own parser and turned
/// into the protocol's options as `--node` turns them.
fn asked(rest: &[String]) -> Result<(&'static str, String, Value), String> {
    let Some((tool, flags)) = rest.split_first() else {
        return Err(ADD_USAGE.into());
    };
    let no_run = || ADD_USAGE.to_string();
    match tool.as_str() {
        "traceroute" => match crate::args::parse(flags)? {
            crate::args::Parsed::Run(args) => {
                of_a_run(&args.common)?;
                let options = remote::traceroute(&args)?;
                Ok(("traceroute", args.target.clone(), options))
            }
            _ => Err(no_run()),
        },
        "ping" => match crate::ping::parse(flags)? {
            crate::ping::Parsed::Run(args) => {
                of_a_run(&args.common)?;
                let options = remote::ping(&args)?;
                Ok(("ping", args.target.clone(), options))
            }
            _ => Err(no_run()),
        },
        "mtr" => match crate::mtr::parse(flags)? {
            crate::mtr::Parsed::Run(args) => {
                of_a_run(&args.common)?;
                let options = remote::mtr(&args)?;
                Ok(("mtr", args.target.clone(), options))
            }
            _ => Err(no_run()),
        },
        "dig" | "dns" => match crate::dig::parse(flags, false)? {
            crate::dig::Parsed::Run(args) => {
                of_a_run(&args.common)?;
                let options = remote::dig(&args)?;
                Ok(("dns", args.name.clone(), options))
            }
            _ => Err(no_run()),
        },
        "bgp" => match crate::bgp::parse(flags)? {
            crate::bgp::Parsed::Run(args) => {
                of_a_run(&args.common)?;
                Ok(("bgp", args.target.clone(), remote::bgp()))
            }
            _ => Err(
                "A schedule takes a BGP lookup of one target, as in rvrs schedule add bgp 1.1.1.1 --every 1h --node fra-1."
                    .into(),
            ),
        },
        "iperf3" => Err("rvrs iperf3 isn't in the CLI yet, so no schedule can name it.".into()),
        other if other.starts_with('-') => Err(ADD_USAGE.into()),
        other => Err(format!(
            "\u{201C}{other}\u{201D} is no tool. A schedule takes traceroute, ping, mtr, dig or bgp."
        )),
    }
}

/// The body of `POST /v1/schedules`: what to run, how often and where. Exactly one of `nodes`,
/// `selector` and `fleet` says where.
fn body(add: &Add) -> Result<Value, String> {
    let (tool, target, options) = asked(&add.rest)?;
    if target.trim().is_empty() {
        return Err(ADD_USAGE.into());
    }
    let Some(every) = add.every else {
        return Err(
            "A schedule takes --every, the time between two runs on one node, as in --every 5m."
                .into(),
        );
    };
    let ways = [!add.nodes.is_empty(), add.selector.is_some(), add.all_nodes];
    if ways.iter().filter(|given| **given).count() != 1 {
        return Err(WHERE.into());
    }
    if add.nodes.len() > 50 {
        return Err("A schedule names 50 nodes at most. --fleet takes a selector for more.".into());
    }
    let mut body = Map::new();
    if let Some(name) = &add.name {
        body.insert("name".into(), name.clone().into());
    }
    body.insert("tool".into(), tool.into());
    body.insert("target".into(), target.into());
    body.insert("options".into(), options);
    body.insert("intervalSeconds".into(), every.into());
    if let Some(selector) = &add.selector {
        body.insert("selector".into(), selector.clone().into());
    } else if add.all_nodes {
        body.insert("fleet".into(), true.into());
    } else {
        body.insert("nodes".into(), add.nodes.clone().into());
    }
    Ok(Value::Object(body))
}

/// What the person calls a schedule: its name, or its tool and target.
fn title(schedule: &Value) -> String {
    match schedule["name"].as_str().filter(|name| !name.is_empty()) {
        Some(name) => name.to_string(),
        None => what(schedule),
    }
}

/// The tool and the target, as typed: `ping example.com`.
fn what(schedule: &Value) -> String {
    format!(
        "{} {}",
        schedule["tool"].as_str().unwrap_or("\u{2013}"),
        schedule["target"].as_str().unwrap_or("\u{2013}")
    )
}

/// The interval the nodes keep, with the one the person asked for when the two differ.
fn every(schedule: &Value) -> String {
    let asked = schedule["intervalSeconds"].as_u64().unwrap_or(0);
    let kept = schedule["everySeconds"].as_u64().unwrap_or(asked);
    if kept == asked {
        spoken(kept)
    } else {
        format!("{} ({} asked)", spoken(kept), spoken(asked))
    }
}

/// Where a schedule runs: the nodes it names, its selector, or every node.
fn runs_on(schedule: &Value) -> String {
    if schedule["fleet"].as_bool() == Some(true) {
        return "every node".to_string();
    }
    if let Some(selector) = schedule["selector"].as_str() {
        return selector.to_string();
    }
    let names: Vec<&str> = schedule["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|node| node["name"].as_str())
        .collect();
    if names.is_empty() {
        "\u{2013}".to_string()
    } else {
        names.join(", ")
    }
}

fn last_run(schedule: &Value, now: u64) -> String {
    match schedule["lastRunAt"].as_str().and_then(format::seconds_of) {
        Some(then) => format::relative(then, now),
        None => "Never".to_string(),
    }
}

/// A text cut to `width` characters, with an ellipsis where something was left out.
fn fitted(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

/// How many characters of an id tell every schedule in a list apart, eight at least.
fn id_width(schedules: &[Value]) -> usize {
    let ids: Vec<&str> = schedules
        .iter()
        .filter_map(|schedule| schedule["id"].as_str())
        .collect();
    (8..=36)
        .find(|width| {
            let mut starts: Vec<&str> = ids
                .iter()
                .map(|id| id.get(..*width).unwrap_or(id))
                .collect();
            starts.sort_unstable();
            starts.windows(2).all(|pair| pair[0] != pair[1])
        })
        .unwrap_or(36)
}

/// One line per schedule under a header: the start of its id, its name or its tool and target,
/// its interval, where it runs, on how many nodes, when it last ran and whether it is paused.
fn schedule_lines(schedules: &[Value], style: Style, now: u64) -> Vec<String> {
    let id_width = id_width(schedules);
    let rows: Vec<[String; 7]> = schedules
        .iter()
        .map(|schedule| {
            let id = schedule["id"].as_str().unwrap_or("\u{2013}");
            [
                id.get(..id_width).unwrap_or(id).to_string(),
                fitted(&title(schedule), 36),
                every(schedule),
                fitted(&runs_on(schedule), 32),
                schedule["nodeCount"]
                    .as_u64()
                    .map_or("\u{2013}".to_string(), |count| count.to_string()),
                last_run(schedule, now),
                if schedule["paused"].as_bool() == Some(true) {
                    "Paused".to_string()
                } else {
                    String::new()
                },
            ]
        })
        .collect();
    let header = [
        "ID", "SCHEDULE", "EVERY", "RUNS ON", "NODES", "LAST RUN", "",
    ];
    let widths: Vec<usize> = (0..7)
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .chain([header[column].len()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: &[String]| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(column, cell)| {
                let pad = widths[column].saturating_sub(cell.chars().count());
                format!("{cell}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    let mut lines = vec![style.dim(&line(&header.map(str::to_string)))];
    for row in &rows {
        let mut shown = line(row);
        if !row[6].is_empty() {
            // The state is the last word of its line.
            let at = shown.len() - row[6].len();
            shown = format!("{}{}", &shown[..at], style.yellow(&row[6]));
        }
        lines.push(shown);
    }
    lines
}

/// One schedule, a fact a line: its id, what it runs, how often, where and when it last ran.
fn detail_lines(schedule: &Value, style: Style, now: u64) -> Vec<String> {
    let row =
        |label: &str, value: String| format!("  {}  {value}", style.dim(&format!("{label:<8}")));
    let options = schedule["options"]
        .as_object()
        .map(|options| {
            options
                .iter()
                .map(|(key, value)| match value {
                    Value::String(text) => format!("{key}={text}"),
                    other => format!("{key}={other}"),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let count = match schedule["nodeCount"].as_u64() {
        Some(1) => " \u{00B7} 1 node".to_string(),
        Some(count) => format!(" \u{00B7} {count} nodes"),
        None => String::new(),
    };
    let mut lines = vec![
        style.bold(&title(schedule)),
        row(
            "ID",
            schedule["id"].as_str().unwrap_or("\u{2013}").to_string(),
        ),
        row(
            "Runs",
            format!("{} {options}", what(schedule))
                .trim_end()
                .to_string(),
        ),
        row("Every", every(schedule)),
        row("On", format!("{}{count}", runs_on(schedule))),
        row("Last run", last_run(schedule, now)),
    ];
    if schedule["paused"].as_bool() == Some(true) {
        lines.push(row("State", style.yellow("Paused")));
    }
    lines
}

/// The sentence for a schedule whose nodes keep a longer interval than the person asked for.
fn longer(schedule: &Value) -> Option<String> {
    let asked = schedule["intervalSeconds"].as_u64()?;
    let kept = schedule["everySeconds"].as_u64()?;
    (kept > asked).then(|| {
        format!(
            "Each node runs it every {kept} s, where you asked for {asked} s, since one target gets about 10 scheduled run starts a second from a workspace."
        )
    })
}

/// The sentence of an answer that is no 2xx. `nodes` are the nodes the request named.
fn refused(status: u16, body: &Value, id: Option<&str>, nodes: &[String]) -> String {
    let code = body["error"]["code"].as_str().unwrap_or_default();
    match (status, code) {
        (401, _) => NO_ACCOUNT.to_string(),
        (403, "SCHEDULE_LIMIT") => {
            "This workspace has as many schedules as it may. Remove one with rvrs schedule rm <id>."
                .to_string()
        }
        (403, "FORBIDDEN") => NODES_TOKEN.to_string(),
        // The API names the node it didn't find in `details.node`. An API that doesn't leaves the
        // CLI with the names it sent.
        (404, "NODE_NOT_FOUND") => {
            let named = body["error"]["details"]["node"].as_str().or(match nodes {
                [one] => Some(one.as_str()),
                _ => None,
            });
            match named {
                Some(one) => format!(
                    "This workspace has no node called \u{201C}{one}\u{201D}. List them with rvrs nodes."
                ),
                None => format!(
                    "This workspace lacks one of the nodes {}. List them with rvrs nodes.",
                    nodes.join(", ")
                ),
            }
        }
        (404, "SCHEDULE_NOT_FOUND") => format!(
            "You have no schedule with the id {}. List them with rvrs schedules.",
            id.unwrap_or("you gave")
        ),
        // Each issue is a sentence that names the field and its rule.
        (422, _) => {
            let issues: Vec<&str> = body["error"]["details"]["issues"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|issue| issue["message"].as_str())
                .collect();
            if issues.is_empty() {
                remote::refusal(status, body)
            } else {
                issues.join(" ")
            }
        }
        _ => remote::refusal(status, body),
    }
}

/// A call as this install, tried again while the API is out of reach. Every call a schedule
/// command makes is safe to make twice.
fn call(method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
    let mut last = String::new();
    for _ in 0..3 {
        match account::call(method, path, body, 20) {
            Ok(answer) => return Ok(answer),
            Err(message) => last = message,
        }
    }
    Err(last)
}

fn pretty(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into())
    );
}

fn show_one(schedule: &Value, json: bool) {
    if json {
        return pretty(schedule);
    }
    let style = Style::detect(ColorChoice::Auto);
    for line in detail_lines(schedule, style, format::now()) {
        println!("{line}");
    }
}

/// The person's schedules, newest first.
fn listed() -> Result<Value, String> {
    let (status, body) = call("GET", "/v1/schedules", None)?;
    match status {
        200 => Ok(body),
        _ => Err(refused(status, &body, None, &[])),
    }
}

/// `rvrs schedules`: the schedules the person made.
fn list(arguments: &[String]) -> ExitCode {
    let mut json = false;
    for argument in arguments {
        match argument.as_str() {
            "--json" => json = true,
            other => {
                return usage(&format!(
                    "rvrs schedules takes --json and nothing else, and got \u{201C}{other}\u{201D}."
                ))
            }
        }
    }
    if account::token().is_none() {
        return fail(NO_ACCOUNT);
    }
    let body = match listed() {
        Ok(body) => body,
        Err(message) => return fail(&message),
    };
    if json {
        pretty(&body);
        return ExitCode::SUCCESS;
    }
    let schedules = body["schedules"].as_array().cloned().unwrap_or_default();
    if schedules.is_empty() {
        println!("No schedules yet. Make one with rvrs schedule add ping example.com --every 5m --node <name>.");
        return ExitCode::SUCCESS;
    }
    let style = Style::detect(ColorChoice::Auto);
    for line in schedule_lines(&schedules, style, format::now()) {
        println!("{line}");
    }
    ExitCode::SUCCESS
}

/// `rvrs schedule add`: makes a schedule.
fn add(arguments: &[String]) -> ExitCode {
    if arguments.is_empty() {
        return usage(ADD_USAGE);
    }
    let waiting = remote::waiting();
    if waiting.no_wait || waiting.expires_in.is_some() {
        return usage(
            "--no-wait and --expires-in go with a run asked of a node. A schedule takes neither.",
        );
    }
    let add = match take_add_flags(arguments) {
        Ok(add) => add,
        Err(message) => return usage(&message),
    };
    if add
        .rest
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print!(
            "{}",
            crate::help::schedule(Style::detect(ColorChoice::Auto))
        );
        return ExitCode::SUCCESS;
    }
    let body = match body(&add) {
        Ok(body) => body,
        Err(message) => return usage(&message),
    };
    if account::token().is_none() {
        return fail(NO_ACCOUNT);
    }
    let (status, schedule) = match call("POST", "/v1/schedules", Some(&body)) {
        Ok(answer) => answer,
        Err(message) => return fail(&message),
    };
    if !matches!(status, 200 | 201) {
        return fail(&refused(status, &schedule, None, &add.nodes));
    }
    show_one(&schedule, add.json);
    if add.json {
        return ExitCode::SUCCESS;
    }
    if status == 201 {
        println!("Made the schedule.");
    } else {
        println!("You already have this schedule. Nothing changed.");
    }
    if let Some(sentence) = longer(&schedule) {
        println!("{sentence}");
    }
    ExitCode::SUCCESS
}

/// True for a whole id, as the API writes it.
fn is_whole_id(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.iter().map(|group| group.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
        && text.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// The schedule whose id starts with `typed`, among the ones listed.
fn matching(typed: &str, schedules: &[Value]) -> Result<String, String> {
    let start = typed.to_lowercase();
    let found: Vec<&str> = schedules
        .iter()
        .filter_map(|schedule| schedule["id"].as_str())
        .filter(|id| id.starts_with(&start))
        .collect();
    match found.as_slice() {
        [one] => Ok(one.to_string()),
        [] => Err(format!(
            "No schedule's id starts with \u{201C}{typed}\u{201D}. List them with rvrs schedules."
        )),
        many => Err(format!(
            "The ids of {} schedules start with \u{201C}{typed}\u{201D}. Type more of the id.",
            many.len()
        )),
    }
}

/// The whole id for what the person typed: an id, or the start of one that `rvrs schedules`
/// lists.
fn whole_id(typed: &str) -> Result<String, String> {
    if is_whole_id(typed) {
        return Ok(typed.to_lowercase());
    }
    let body = listed()?;
    matching(
        typed,
        body["schedules"].as_array().map_or(&[][..], Vec::as_slice),
    )
}

/// The id and `--json` of a command that takes one schedule.
fn one(command: &str, arguments: &[String]) -> Result<(String, bool), String> {
    let mut json = false;
    let mut id: Option<&String> = None;
    for argument in arguments {
        match argument.as_str() {
            "--json" => json = true,
            other if other.starts_with('-') || id.is_some() => {
                return Err(format!(
                    "rvrs schedule {command} takes the id of one schedule, and got \u{201C}{other}\u{201D} too."
                ))
            }
            _ => id = Some(argument),
        }
    }
    match id {
        Some(id) => Ok((id.clone(), json)),
        None => Err(format!(
            "rvrs schedule {command} takes the id of a schedule. rvrs schedules lists them."
        )),
    }
}

/// What a command that takes one schedule does once the id is whole.
fn with_one(
    command: &str,
    arguments: &[String],
    act: impl FnOnce(&str, bool) -> ExitCode,
) -> ExitCode {
    let (typed, json) = match one(command, arguments) {
        Ok(read) => read,
        Err(message) => return usage(&message),
    };
    if account::token().is_none() {
        return fail(NO_ACCOUNT);
    }
    match whole_id(&typed) {
        Ok(id) => act(&id, json),
        Err(message) => fail(&message),
    }
}

/// `rvrs schedule show <id>`.
fn show(arguments: &[String]) -> ExitCode {
    with_one("show", arguments, |id, json| {
        match call("GET", &format!("/v1/schedules/{id}"), None) {
            Ok((200, schedule)) => {
                show_one(&schedule, json);
                if let Some(sentence) = longer(&schedule).filter(|_| !json) {
                    println!("{sentence}");
                }
                ExitCode::SUCCESS
            }
            Ok((status, body)) => fail(&refused(status, &body, Some(id), &[])),
            Err(message) => fail(&message),
        }
    })
}

/// `rvrs schedule pause <id>` and `rvrs schedule resume <id>`.
fn pause(arguments: &[String], paused: bool) -> ExitCode {
    let command = if paused { "pause" } else { "resume" };
    with_one(command, arguments, |id, json| {
        let change = json!({ "paused": paused });
        match call("PATCH", &format!("/v1/schedules/{id}"), Some(&change)) {
            Ok((200, schedule)) => {
                if json {
                    pretty(&schedule);
                } else if paused {
                    println!(
                        "Paused {}. No node runs it until rvrs schedule resume.",
                        title(&schedule)
                    );
                } else {
                    println!("Resumed {}. Its nodes run it again.", title(&schedule));
                }
                ExitCode::SUCCESS
            }
            Ok((status, body)) => fail(&refused(status, &body, Some(id), &[])),
            Err(message) => fail(&message),
        }
    })
}

/// `rvrs schedule rm <id>`: deletes a schedule and keeps its runs.
fn remove(arguments: &[String]) -> ExitCode {
    with_one("rm", arguments, |id, _| {
        match call("DELETE", &format!("/v1/schedules/{id}"), None) {
            Ok((200 | 204, _)) => {
                println!("Deleted the schedule {id}. The runs it made are kept.");
                ExitCode::SUCCESS
            }
            Ok((status, body)) => fail(&refused(status, &body, Some(id), &[])),
            Err(message) => fail(&message),
        }
    })
}

/// `rvrs schedule` and `rvrs schedules`.
pub fn command(arguments: &[String]) -> ExitCode {
    let rest = arguments.get(1..).unwrap_or_default();
    match arguments.first().map(String::as_str) {
        None => list(&[]),
        Some("--json") => list(arguments),
        Some("list") => list(rest),
        Some("--help" | "-h" | "help") => {
            print!("{}", crate::help::schedule(Style::detect(ColorChoice::Auto)));
            ExitCode::SUCCESS
        }
        Some("add") => add(rest),
        Some("show") => show(rest),
        Some("pause") => pause(rest, true),
        Some("resume") => pause(rest, false),
        Some("rm" | "remove") => remove(rest),
        Some("traceroute" | "ping" | "mtr" | "dig" | "dns" | "bgp") => usage(
            "A schedule is made with add, as in rvrs schedule add ping example.com --every 5m --node fra-1.",
        ),
        Some(other) => usage(&format!(
            "rvrs schedule has no command \u{201C}{other}\u{201D}. It has add, show, pause, resume and rm."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    fn fixture(name: &str) -> Value {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../schema/node-protocol/fixtures")
            .join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn typed(arguments: &[&str]) -> Result<Value, String> {
        let arguments: Vec<String> = arguments.iter().map(|word| word.to_string()).collect();
        body(&take_add_flags(&arguments)?)
    }

    #[test]
    fn the_command_line_makes_the_protocols_example() {
        // `rvrs schedule add ping -c 10 example.com --every 60 --node fra-1 --node cpe-4471
        // --name "Evening jitter"` sends the body the protocol's page shows.
        let made = typed(&[
            "ping",
            "-c",
            "10",
            "example.com",
            "--every",
            "60",
            "--node",
            "fra-1",
            "--node",
            "cpe-4471",
            "--name",
            "Evening jitter",
        ])
        .unwrap();
        assert_eq!(made, fixture("schedule.request.json"));
        // A selector travels as it was typed, and the tool's flags may come last.
        let by_selector = typed(&[
            "mtr",
            "192.0.2.53",
            "--fleet=isp=telia,line in (ftth,cable)",
            "--every",
            "5m",
            "-c",
            "10",
        ])
        .unwrap();
        assert_eq!(by_selector, fixture("schedule.selector.request.json"));
    }

    #[test]
    fn a_schedule_runs_where_one_of_three_ways_says() {
        let fleet = typed(&["dig", "example.com", "AAAA", "--every", "1h", "--all-nodes"]).unwrap();
        assert_eq!(
            fleet,
            json!({
                "tool": "dns", "target": "example.com", "options": { "type": "AAAA" },
                "intervalSeconds": 3600, "fleet": true,
            })
        );
        let bgp = typed(&["bgp", "1.1.1.1", "--every", "24h", "--node", "fra-1"]).unwrap();
        assert_eq!(bgp["options"], json!({}));
        assert_eq!(bgp["nodes"], json!(["fra-1"]));
        // None of the three, or two of them, is refused before anything is sent.
        assert_eq!(
            typed(&["ping", "example.com", "--every", "5m"]).unwrap_err(),
            WHERE
        );
        assert_eq!(
            typed(&[
                "ping",
                "example.com",
                "--every",
                "5m",
                "--node",
                "fra-1",
                "--all-nodes"
            ])
            .unwrap_err(),
            WHERE
        );
        assert_eq!(
            typed(&[
                "ping",
                "example.com",
                "--every",
                "5m",
                "--fleet",
                "isp=telia",
                "--node",
                "fra-1"
            ])
            .unwrap_err(),
            WHERE
        );
        // --fleet takes a selector. Every node is --all-nodes.
        assert_eq!(
            typed(&["ping", "example.com", "--every", "5m", "--fleet"]).unwrap_err(),
            "--fleet takes a selector, as in --fleet isp=telia. --all-nodes takes every node."
        );
    }

    #[test]
    fn every_is_seconds_or_a_number_with_a_unit() {
        assert_eq!(interval("60"), Ok(60));
        assert_eq!(interval("90s"), Ok(90));
        assert_eq!(interval("5m"), Ok(300));
        assert_eq!(interval("24h"), Ok(86_400));
        for wrong in [
            "59", "30s", "25h", "86401", "5", "m", "", "5 min", "1d", "-5m",
        ] {
            assert_eq!(
                interval(wrong).unwrap_err(),
                "--every takes an interval from 60 seconds to 24 hours, as in --every 5m or --every 300.",
                "{wrong}"
            );
        }
        assert_eq!(
            typed(&["ping", "example.com", "--node", "fra-1"]).unwrap_err(),
            "A schedule takes --every, the time between two runs on one node, as in --every 5m."
        );
        assert_eq!(spoken(60), "1m");
        assert_eq!(spoken(90), "90s");
        assert_eq!(spoken(1200), "20m");
        assert_eq!(spoken(86_400), "24h");
    }

    #[test]
    fn a_flag_that_does_not_travel_to_a_node_is_refused_as_with_node() {
        let sized = typed(&[
            "ping",
            "-s",
            "1400",
            "example.com",
            "--every",
            "5m",
            "--node",
            "fra-1",
        ]);
        assert_eq!(
            sized.unwrap_err(),
            "A ping from a node takes -4, -6, -c, -i, -W, -n and --no-lookups. The other flags don't travel to a node yet."
        );
        assert!(typed(&[
            "dig",
            "@9.9.9.9",
            "example.com",
            "--every",
            "5m",
            "--all-nodes"
        ])
        .is_err());
        // A run's own flags of rvrs mean nothing to a schedule.
        assert_eq!(
            typed(&[
                "ping",
                "example.com",
                "--case",
                "office",
                "--every",
                "5m",
                "--all-nodes"
            ])
            .unwrap_err(),
            "--classic and --case go with a run. A schedule takes neither."
        );
        assert_eq!(
            typed(&["whois", "example.com", "--every", "5m", "--all-nodes"]).unwrap_err(),
            "\u{201C}whois\u{201D} is no tool. A schedule takes traceroute, ping, mtr, dig or bgp."
        );
        assert_eq!(
            typed(&["--every", "5m", "--all-nodes"]).unwrap_err(),
            ADD_USAGE
        );
        assert!(typed(&["iperf3", "example.com", "--every", "5m", "--all-nodes"]).is_err());
    }

    #[test]
    fn the_list_names_each_schedule_and_where_it_runs() {
        let listed = fixture("schedules.response.json");
        let schedules = listed["schedules"].as_array().unwrap();
        let now = format::seconds_of("2026-11-23T22:15:41.000Z").unwrap();
        let mut paused = schedules.clone();
        paused[1]["paused"] = true.into();
        let lines = schedule_lines(&paused, Style::detect(ColorChoice::Never), now);
        assert_eq!(
            lines,
            [
                "ID        SCHEDULE        EVERY           RUNS ON                         NODES  LAST RUN",
                "c4d8e1f2  mtr 192.0.2.53  20m (5m asked)  isp=telia,line in (ftth,cable)  12000  2h ago",
                "7a1c9e52  Evening jitter  1m              cpe-4471, fra-1                 2      2h ago    Paused",
            ]
        );
        // A schedule on every node says so, and one that never ran says that.
        let mut fleet = schedules[0].clone();
        fleet["fleet"] = true.into();
        fleet["selector"] = Value::Null;
        fleet["lastRunAt"] = Value::Null;
        let lines = schedule_lines(&[fleet], Style::detect(ColorChoice::Never), now);
        assert!(lines[1].contains("every node"), "{}", lines[1]);
        assert!(lines[1].ends_with("Never"), "{}", lines[1]);
    }

    #[test]
    fn ids_are_shown_as_long_as_tells_them_apart() {
        let schedule = |id: &str| json!({ "id": id });
        let apart = [
            schedule("7a1c9e52-3b4d-4f6a-8c0e-2d5f7b9a1c3e"),
            schedule("c4d8e1f2-6a7b-4c9d-8e0f-1a2b3c4d5e6f"),
        ];
        assert_eq!(id_width(&apart), 8);
        let close = [
            schedule("7a1c9e52-3b4d-4f6a-8c0e-2d5f7b9a1c3e"),
            schedule("7a1c9e52-3b5d-4f6a-8c0e-2d5f7b9a1c3e"),
        ];
        assert_eq!(id_width(&close), 12);
        assert_eq!(id_width(&[]), 8);
    }

    #[test]
    fn the_start_of_an_id_finds_its_schedule() {
        let listed = fixture("schedules.response.json");
        let schedules = listed["schedules"].as_array().unwrap();
        assert_eq!(
            matching("7a1c", schedules).unwrap(),
            "7a1c9e52-3b4d-4f6a-8c0e-2d5f7b9a1c3e"
        );
        assert_eq!(
            matching("C4D8E1F2", schedules).unwrap(),
            "c4d8e1f2-6a7b-4c9d-8e0f-1a2b3c4d5e6f"
        );
        assert_eq!(
            matching("ffff", schedules).unwrap_err(),
            "No schedule's id starts with \u{201C}ffff\u{201D}. List them with rvrs schedules."
        );
        assert_eq!(
            matching("", schedules).unwrap_err(),
            "The ids of 2 schedules start with \u{201C}\u{201D}. Type more of the id."
        );
        assert!(is_whole_id("7a1c9e52-3b4d-4f6a-8c0e-2d5f7b9a1c3e"));
        assert!(!is_whole_id("7a1c9e52"));
        assert_eq!(
            one("pause", &words("7a1c --json")).unwrap(),
            ("7a1c".to_string(), true)
        );
        assert_eq!(
            one("pause", &[]).unwrap_err(),
            "rvrs schedule pause takes the id of a schedule. rvrs schedules lists them."
        );
        assert!(one("rm", &words("7a1c c4d8")).is_err());
    }

    #[test]
    fn one_schedule_is_shown_a_fact_a_line() {
        let now = format::seconds_of("2026-11-23T22:15:16.000Z").unwrap();
        let style = Style::detect(ColorChoice::Never);
        assert_eq!(
            detail_lines(&fixture("schedule.paused.response.json"), style, now),
            [
                "Evening jitter",
                "  ID        7a1c9e52-3b4d-4f6a-8c0e-2d5f7b9a1c3e",
                "  Runs      ping example.com packets=10",
                "  Every     1m",
                "  On        cpe-4471, fra-1 \u{00B7} 2 nodes",
                "  Last run  Just now",
                "  State     Paused",
            ]
        );
        let by_selector = fixture("schedule.selector.response.json");
        let lines = detail_lines(&by_selector, style, now);
        assert_eq!(lines[0], "mtr 192.0.2.53");
        assert_eq!(lines[3], "  Every     20m (5m asked)");
        assert_eq!(
            lines[4],
            "  On        isp=telia,line in (ftth,cable) \u{00B7} 12000 nodes"
        );
        // The cap made its interval longer, and one sentence says so with both numbers.
        assert_eq!(
            longer(&by_selector).unwrap(),
            "Each node runs it every 1200 s, where you asked for 300 s, since one target gets about 10 scheduled run starts a second from a workspace."
        );
        assert_eq!(longer(&fixture("schedule.response.json")), None);
    }

    #[test]
    fn the_apis_refusals_are_sentences_with_a_next_step() {
        let error = |code: &str| json!({ "error": { "code": code, "message": "x." } });
        assert_eq!(
            refused(404, &error("SCHEDULE_NOT_FOUND"), Some("7a1c9e52"), &[]),
            "You have no schedule with the id 7a1c9e52. List them with rvrs schedules."
        );
        assert_eq!(
            refused(404, &error("NODE_NOT_FOUND"), None, &words("fra-9")),
            "This workspace has no node called \u{201C}fra-9\u{201D}. List them with rvrs nodes."
        );
        assert_eq!(
            refused(404, &error("NODE_NOT_FOUND"), None, &words("fra-1 fra-9")),
            "This workspace lacks one of the nodes fra-1, fra-9. List them with rvrs nodes."
        );
        let named = json!({ "error": {
            "code": "NODE_NOT_FOUND",
            "message": "x.",
            "details": { "node": "fra-9" },
        } });
        assert_eq!(
            refused(404, &named, None, &words("fra-1 fra-9")),
            "This workspace has no node called \u{201C}fra-9\u{201D}. List them with rvrs nodes."
        );
        assert_eq!(
            refused(403, &error("SCHEDULE_LIMIT"), None, &[]),
            "This workspace has as many schedules as it may. Remove one with rvrs schedule rm <id>."
        );
        assert_eq!(refused(403, &error("FORBIDDEN"), None, &[]), NODES_TOKEN);
        assert_eq!(refused(401, &error("UNAUTHORIZED"), None, &[]), NO_ACCOUNT);
        // A body that breaks a rule comes back with a sentence per field.
        let invalid = json!({ "error": {
            "code": "VALIDATION_ERROR",
            "message": "selector must not be empty; details.issues lists 1 more.",
            "details": { "issues": [
                { "path": "selector", "message": "selector must not be empty." },
                { "path": "intervalSeconds", "message": "intervalSeconds must be at least 60." },
            ] },
        } });
        assert_eq!(
            refused(422, &invalid, None, &[]),
            "selector must not be empty. intervalSeconds must be at least 60."
        );
        assert_eq!(refused(422, &error("VALIDATION_ERROR"), None, &[]), "x.");
        assert_eq!(
            refused(429, &json!(null), None, &[]),
            "The rvrs API answered 429."
        );
    }
}
