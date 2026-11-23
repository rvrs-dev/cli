//! The commands around a run: history, saved runs and cases, status, completions.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use rvrs_core::cases::{self, CaseError, Cases, Kept};
use rvrs_core::favorites::Favorites;
use rvrs_core::history::History;
use rvrs_core::model::{Outcome, Run};
use rvrs_core::report::Report;
use rvrs_core::socket::IcmpSocket;
use rvrs_core::{format, paths, resolve, Family};

use crate::style::{truncate_middle, width, ColorChoice, Style};
use crate::{completions, fail, term, trace, usage};

struct ShowFlags {
    json: bool,
    classic: bool,
    rest: Vec<String>,
}

fn show_flags(arguments: &[String], command: &str) -> Result<ShowFlags, String> {
    let mut flags = ShowFlags {
        json: false,
        classic: false,
        rest: Vec::new(),
    };
    for argument in arguments {
        match argument.as_str() {
            "--json" => flags.json = true,
            "--classic" => flags.classic = true,
            other if other.starts_with('-') => {
                return Err(format!(
                    "rvrs {command} has no flag {other}. It takes --json and --classic."
                ))
            }
            other => flags.rest.push(other.to_string()),
        }
    }
    Ok(flags)
}

pub fn last(arguments: &[String]) -> ExitCode {
    let flags = match show_flags(arguments, "last") {
        Ok(flags) => flags,
        Err(message) => return usage(&message),
    };
    let history = History::open();
    let Some(run) = history.last() else {
        return fail("No runs yet. Start one with rvrs traceroute 1.1.1.1.");
    };
    trace::show(
        run,
        flags.json,
        flags.classic,
        Style::detect(ColorChoice::Auto),
    );
    ExitCode::SUCCESS
}

pub fn show(arguments: &[String]) -> ExitCode {
    let flags = match show_flags(arguments, "show") {
        Ok(flags) => flags,
        Err(message) => return usage(&message),
    };
    let Some(id) = flags.rest.first() else {
        return usage("rvrs show needs the id of a run. List them with rvrs history.");
    };
    let history = History::open();
    let saved = History::saved();
    // A saved run outlives its place in history, so both are searched.
    let Some(run) = history.find(id).or_else(|| saved.find(id)) else {
        return fail(&format!(
            "No run in history has the id \u{201C}{id}\u{201D}. List them with rvrs history."
        ));
    };
    trace::show(
        run,
        flags.json,
        flags.classic,
        Style::detect(ColorChoice::Auto),
    );
    ExitCode::SUCCESS
}

pub fn history(arguments: &[String]) -> ExitCode {
    let mut history = History::open();
    match arguments.first().map(String::as_str) {
        Some("clear") => {
            let count = history.runs.len();
            if count == 0 {
                println!("History is already empty.");
                return ExitCode::SUCCESS;
            }
            let confirmed = arguments
                .iter()
                .any(|argument| argument == "--yes" || argument == "-y");
            if !confirmed {
                if !std::io::stdin().is_terminal() {
                    return usage("rvrs history clear deletes every run and can't be undone. Add --yes to confirm.");
                }
                let noun = if count == 1 { "run" } else { "runs" };
                print!("Delete {count} {noun} from history? This can't be undone. [y/N] ");
                let _ = std::io::stdout().flush();
                let mut answer = String::new();
                let _ = std::io::stdin().read_line(&mut answer);
                if !matches!(answer.trim(), "y" | "Y" | "yes") {
                    println!("Nothing deleted.");
                    return ExitCode::SUCCESS;
                }
            }
            match history.clear() {
                Ok(()) => {
                    println!("History cleared.");
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&format!("Couldn't clear history ({error}).")),
            }
        }
        _ => {
            let mut json = false;
            let mut words: Vec<&str> = Vec::new();
            for argument in arguments {
                match argument.as_str() {
                    "--json" => json = true,
                    flag if flag.starts_with('-') => {
                        return usage(&format!(
                            "rvrs history has no flag {flag}. It takes a tool, a target, --json or clear."
                        ))
                    }
                    word => words.push(word),
                }
            }
            let runs: Vec<Run> = history
                .runs
                .iter()
                .filter(|run| matches_filter(run, &words))
                .cloned()
                .collect();
            if json {
                let reports: Vec<Report> = runs.iter().map(Report::of).collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&reports).unwrap_or_else(|_| "[]".into())
                );
                return ExitCode::SUCCESS;
            }
            if history.runs.is_empty() {
                println!("No runs yet. Start one with rvrs traceroute 1.1.1.1.");
                return ExitCode::SUCCESS;
            }
            if runs.is_empty() {
                println!("No runs match \u{201C}{}\u{201D}.", words.join(" "));
                return ExitCode::SUCCESS;
            }
            let style = Style::detect(ColorChoice::Auto);
            let columns = list_columns();
            for line in history_lines(&runs, style, columns, format::now()) {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
    }
}

/// The Console's filter, for `rvrs history <words>`: every word must be the run's tool or appear
/// in its target, whatever the case. `dns` finds dig runs, since `rvrs dns` is the same command.
pub fn matches_filter(run: &Run, words: &[&str]) -> bool {
    let target = run.target.to_lowercase();
    let tool = run.tool.command();
    words.iter().all(|word| {
        let word = word.to_lowercase();
        word == tool || (word == "dns" && tool == "dig") || target.contains(&word)
    })
}

/// One line per run under a header: id, tool, target, summary, where it was made from, age.
/// SOURCE is the address probes left from; PUBLIC is the address the internet saw.
pub fn history_lines(runs: &[Run], style: Style, columns: usize, now: u64) -> Vec<String> {
    let widest =
        |texts: &mut dyn Iterator<Item = usize>, floor: usize| texts.max().unwrap_or(0).max(floor);
    let address = |address: Option<std::net::IpAddr>| {
        address.map_or_else(|| "\u{2013}".to_string(), |address| address.to_string())
    };
    let source_width = widest(&mut runs.iter().map(|run| width(&address(run.source))), 6);
    let public_width = widest(
        &mut runs.iter().map(|run| width(&address(run.public_source))),
        6,
    );
    let summary_width = widest(&mut runs.iter().map(|run| width(&run.summary())), 7);
    // The addresses go first when the terminal is too narrow for everything.
    let show_addresses =
        columns >= 6 + 12 + 16 + summary_width + source_width + public_width + 9 + 12;
    let fixed = 6
        + 12
        + summary_width
        + 9
        + 8
        + if show_addresses {
            source_width + public_width + 4
        } else {
            0
        };
    let target_width = widest(&mut runs.iter().map(|run| width(&run.target)), 6)
        .min(columns.saturating_sub(fixed).max(16));

    let pad =
        |text: &str, wide: usize| format!("{text}{}", " ".repeat(wide.saturating_sub(width(text))));
    let mut header = format!(
        "{}  {}  {}  {}",
        pad("ID", 6),
        pad("TOOL", 10),
        pad("TARGET", target_width),
        pad("SUMMARY", summary_width)
    );
    if show_addresses {
        header.push_str(&format!(
            "  {}  {}",
            pad("SOURCE", source_width),
            pad("PUBLIC", public_width)
        ));
    }
    header.push_str("  AGE");
    // Runs the account handed this device name the install that made them.
    let from_elsewhere = runs.iter().any(|run| run.from.is_some());
    if from_elsewhere {
        header.push_str("       FROM");
    }
    let mut out = vec![style.bold(&header)];
    for run in runs {
        let target = truncate_middle(&run.target, target_width);
        let summary = run.summary();
        let gap = " ".repeat(summary_width - width(&summary));
        let lossy = run.tool == rvrs_core::model::Tool::Ping && run.ping_stats().lost() > 0;
        let summary = match run.outcome {
            Some(Outcome::Reached) if !lossy => summary,
            Some(Outcome::Stopped) if !lossy => style.dim(&summary),
            _ => style.yellow(&summary),
        };
        let mut line = format!(
            "{}  {}  {}{}  {summary}{gap}",
            style.dim(&run.id),
            style.dim(&pad(run.tool.command(), 10)),
            style.bold(&target),
            " ".repeat(target_width - width(&target)),
        );
        if show_addresses {
            line.push_str(&format!(
                "  {}  {}",
                pad(&address(run.source), source_width),
                pad(&address(run.public_source), public_width)
            ));
        }
        let age = format::relative(run.started_at, now);
        line.push_str(&format!("  {}", style.dim(&age)));
        if from_elsewhere {
            let from = run.from.as_deref().unwrap_or("this device");
            line.push_str(&format!(
                "{}  {}",
                " ".repeat(8usize.saturating_sub(width(&age))),
                style.dim(from)
            ));
        }
        out.push(line);
    }
    out
}

/// The width a list may use: the terminal's, or every column when the output goes to a pipe.
fn list_columns() -> usize {
    if std::io::stdout().is_terminal() {
        term::size().0
    } else {
        400
    }
}

/// `rvrs favorite <host>`: keeps a host in Favorites.
pub fn favorite(arguments: &[String]) -> ExitCode {
    let Some(host) = arguments
        .first()
        .filter(|argument| !argument.starts_with('-'))
    else {
        return usage("rvrs favorite needs a host, such as rvrs favorite example.com");
    };
    let host = crate::args::normalize_target(host);
    let mut favorites = Favorites::open();
    match favorites.add(&host) {
        Ok(true) => {
            println!("{host} is a favorite now.");
            ExitCode::SUCCESS
        }
        Ok(false) => {
            println!("{host} is already a favorite.");
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("Couldn't save the favorite ({error}).")),
    }
}

/// `rvrs favorites`: lists the hosts with their newest run. `rvrs favorites remove <host>`
/// takes one out.
pub fn favorites(arguments: &[String]) -> ExitCode {
    let mut favorites = Favorites::open();
    match arguments.first().map(String::as_str) {
        Some("remove") => {
            let Some(host) = arguments.get(1) else {
                return usage("rvrs favorites remove needs a host. List them with rvrs favorites.");
            };
            match favorites.remove(host) {
                Ok(true) => {
                    println!("{host} is no longer a favorite.");
                    ExitCode::SUCCESS
                }
                Ok(false) => fail(&format!(
                    "\u{201C}{host}\u{201D} isn't a favorite. List them with rvrs favorites."
                )),
                Err(error) => fail(&format!("Couldn't remove the favorite ({error}).")),
            }
        }
        Some(other) => usage(&format!(
            "rvrs favorites takes remove, and \u{201C}{other}\u{201D} isn't it."
        )),
        None => {
            if favorites.hosts.is_empty() {
                println!("No favorites. Add one with rvrs favorite example.com.");
                return ExitCode::SUCCESS;
            }
            let style = Style::detect(ColorChoice::Auto);
            let history = History::open();
            for line in favorite_lines(&favorites.hosts, &history.runs, style, format::now()) {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
    }
}

/// One line per favorite: the host, how many runs history holds for it, and the newest.
pub fn favorite_lines(hosts: &[String], runs: &[Run], style: Style, now: u64) -> Vec<String> {
    let host_width = hosts
        .iter()
        .map(|host| width(host))
        .max()
        .unwrap_or(0)
        .max(4);
    let pad =
        |text: &str, wide: usize| format!("{text}{}", " ".repeat(wide.saturating_sub(width(text))));
    let mut out = vec![style.bold(&format!("{}  RUNS  LAST RUN", pad("HOST", host_width)))];
    for host in hosts {
        let of_host: Vec<&Run> = runs
            .iter()
            .filter(|run| run.target.eq_ignore_ascii_case(host))
            .collect();
        let last = match of_host.first() {
            Some(run) => format!(
                "{} \u{00B7} {}  {}",
                run.tool.command(),
                run.summary(),
                style.dim(&format::relative(run.started_at, now))
            ),
            None => style.dim("\u{2013}"),
        };
        out.push(format!(
            "{}  {:>4}  {last}",
            style.bold(&pad(host, host_width)),
            of_host.len()
        ));
    }
    out
}

fn count_of(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

/// How messages name a run: "1.1.1.1 · traceroute (abc234)".
fn run_name(run: &Run) -> String {
    format!(
        "{} \u{00B7} {} ({})",
        run.target,
        run.tool.command(),
        run.id
    )
}

fn case_error(error: CaseError) -> ExitCode {
    match error {
        CaseError::Blank => usage("A case needs a name, such as vpn-drop."),
        CaseError::Exists(name) => fail(&format!(
            "A case named \u{201C}{name}\u{201D} already exists. List them with rvrs case."
        )),
        CaseError::Missing(name) => fail(&format!(
            "No case is named \u{201C}{name}\u{201D}. List them with rvrs case."
        )),
        CaseError::Io(error) => fail(&format!("Couldn't save the case ({error}).")),
    }
}

/// What saving did, in one line: "Saved 1.1.1.1 · traceroute (abc234) to vpn-drop".
pub fn kept_message(run: &Run, kept: &Kept) -> String {
    let name = run_name(run);
    let into = |case: &cases::KeptIn| {
        if case.created {
            format!(" to the new case {}", case.name)
        } else {
            format!(" to {}", case.name)
        }
    };
    match (&kept.case, kept.newly_saved) {
        (None, true) => format!("Saved {name}"),
        (None, false) => format!("Already saved: {name}"),
        (Some(case), true) => format!("Saved {name}{}", into(case)),
        (Some(case), false) if case.added => format!("Added {name}{}", into(case)),
        (Some(case), false) => format!("{name} is already in {}.", case.name),
    }
}

/// A run named by id, from history and then saved runs, or the newest in history.
fn pick_run(id: Option<&str>) -> Result<Run, ExitCode> {
    let history = History::open();
    match id {
        Some(id) => {
            if let Some(run) = history.find(id) {
                return Ok(run.clone());
            }
            History::saved().find(id).cloned().ok_or_else(|| {
                fail(&format!(
                    "No run in history has the id \u{201C}{id}\u{201D}. List them with rvrs history."
                ))
            })
        }
        None => history
            .last()
            .cloned()
            .ok_or_else(|| fail("No runs yet. Start one with rvrs traceroute 1.1.1.1.")),
    }
}

/// Saves a run, optionally into a case, and says what happened.
fn keep(run: &Run, case: Option<&str>) -> Result<String, ExitCode> {
    let mut saved = History::saved();
    let mut cases = Cases::open(&saved);
    match cases::save(&mut saved, &mut cases, run, case) {
        Ok(kept) => Ok(kept_message(run, &kept)),
        Err(CaseError::Io(error)) => Err(fail(&format!("Couldn't save the run ({error})."))),
        Err(error) => Err(case_error(error)),
    }
}

/// `--case <name>` on a tool command: once the run is made, it is saved into the case. The
/// confirmation goes to stderr so a pipe gets only the run.
pub fn save_after_run(run: &Run, case: &str) {
    // An error has been printed already, and the run itself was made.
    if let Ok(message) = keep(run, Some(case)) {
        eprintln!("{message}");
    }
}

/// `rvrs save [id] [--case <name>]`: keeps the newest run, or the one named.
pub fn save(arguments: &[String]) -> ExitCode {
    let mut id: Option<&str> = None;
    let mut case: Option<String> = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        index += 1;
        if let Some(value) = argument.strip_prefix("--case=") {
            case = Some(value.to_string());
        } else if argument == "--case" {
            let Some(value) = arguments.get(index) else {
                return usage("--case takes the name of a case, such as rvrs save --case vpn-drop");
            };
            index += 1;
            case = Some(value.clone());
        } else if argument.starts_with('-') {
            return usage(&format!(
                "rvrs save has no flag {argument}. It takes the id of a run and --case <name>."
            ));
        } else if id.is_none() {
            id = Some(argument);
        } else {
            return usage("rvrs save takes one run id, or none for the newest.");
        }
    }
    let run = match pick_run(id) {
        Ok(run) => run,
        Err(code) => return code,
    };
    match keep(&run, case.as_deref()) {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

fn print_reports(runs: &[&Run]) {
    let reports: Vec<Report> = runs.iter().map(|run| Report::of(run)).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&reports).unwrap_or_else(|_| "[]".into())
    );
}

fn print_runs(runs: &[Run]) {
    let style = Style::detect(ColorChoice::Auto);
    for line in history_lines(runs, style, list_columns(), format::now()) {
        println!("{line}");
    }
}

/// `rvrs saved`: lists the saved runs. `rvrs saved remove <id>` removes one and its place in
/// every case.
pub fn saved(arguments: &[String]) -> ExitCode {
    let mut saved = History::saved();
    match arguments.first().map(String::as_str) {
        Some("remove") => {
            let Some(id) = arguments.get(1) else {
                return usage(
                    "rvrs saved remove needs the id of a saved run. List them with rvrs saved.",
                );
            };
            let Some(run) = saved.find(id).cloned() else {
                return fail(&format!(
                    "No saved run has the id \u{201C}{id}\u{201D}. List them with rvrs saved."
                ));
            };
            let mut cases = Cases::open(&saved);
            let held = cases.holding(&run.id).count();
            match cases::unsave(&mut saved, &mut cases, &run.id) {
                Ok(_) if held > 0 => {
                    println!(
                        "Removed {} from saved runs and from {}.",
                        run_name(&run),
                        count_of(held, "case", "cases")
                    );
                    ExitCode::SUCCESS
                }
                Ok(_) => {
                    println!("Removed {} from saved runs.", run_name(&run));
                    ExitCode::SUCCESS
                }
                Err(error) => fail(&format!("Couldn't remove the saved run ({error}).")),
            }
        }
        Some("--json") => {
            print_reports(&saved.runs.iter().collect::<Vec<_>>());
            ExitCode::SUCCESS
        }
        Some(other) => usage(&format!(
            "rvrs saved takes remove or --json, and \u{201C}{other}\u{201D} is neither."
        )),
        None => {
            if saved.runs.is_empty() {
                println!("No saved runs. Save the newest run with rvrs save.");
                return ExitCode::SUCCESS;
            }
            print_runs(&saved.runs);
            ExitCode::SUCCESS
        }
    }
}

/// The saved runs a case holds, newest saved first.
fn runs_in<'a>(saved: &'a History, case: &rvrs_core::cases::Case) -> Vec<&'a Run> {
    saved
        .runs
        .iter()
        .filter(|run| case.holds(&run.id))
        .collect()
}

/// One line per case: its name, how many runs it holds and how old the newest is. The case
/// used last says so.
pub fn case_lines(cases: &Cases, saved: &History, style: Style, now: u64) -> Vec<String> {
    let name_width = cases
        .cases
        .iter()
        .map(|case| width(&case.name))
        .max()
        .unwrap_or(0)
        .max(4);
    let pad =
        |text: &str, wide: usize| format!("{text}{}", " ".repeat(wide.saturating_sub(width(text))));
    let mut out = vec![style.bold(&format!("{}  RUNS  NEWEST RUN", pad("CASE", name_width)))];
    for case in &cases.cases {
        let runs = runs_in(saved, case);
        let newest = runs
            .iter()
            .map(|run| run.started_at)
            .max()
            .map_or_else(|| "\u{2013}".to_string(), |at| format::relative(at, now));
        let mut line = format!(
            "{}  {:>4}  {}",
            style.bold(&pad(&case.name, name_width)),
            runs.len(),
            style.dim(&pad(&newest, 10))
        );
        if cases.last_used.as_deref() == Some(case.id.as_str()) {
            line.push_str(&format!("  {}", style.dim("used last")));
        }
        out.push(line.trim_end().to_string());
    }
    out
}

/// One argument after a subcommand of `rvrs case`, or a usage error that names it.
fn one<'a>(arguments: &'a [String], at: usize, message: &str) -> Result<&'a str, ExitCode> {
    arguments
        .get(at)
        .map(String::as_str)
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| usage(message))
}

/// `rvrs case`: lists the cases. `rvrs case <name>` lists the runs in one; `new`, `rename`,
/// `delete`, `add` and `remove` change them.
pub fn case(arguments: &[String]) -> ExitCode {
    let words: Vec<&str> = arguments
        .iter()
        .map(String::as_str)
        .filter(|word| *word != "--json")
        .collect();
    let json = words.len() < arguments.len();
    let saved = History::saved();
    let mut cases = Cases::open(&saved);
    let outcome = (|| -> Result<ExitCode, ExitCode> {
        let extra = |count: usize, command: &str| {
            if words.len() > count {
                Err(usage(&format!(
                    "rvrs case {command} takes {}. Put a name with spaces in quotes.",
                    match count {
                        2 => "one name",
                        _ => "a name and one more word",
                    }
                )))
            } else {
                Ok(())
            }
        };
        match words.first().copied() {
            None => {
                if json {
                    let entries: Vec<serde_json::Value> = cases
                        .cases
                        .iter()
                        .map(|case| {
                            let runs = runs_in(&saved, case);
                            serde_json::json!({
                                "id": case.id,
                                "name": case.name,
                                "created_at": case.created_at,
                                "runs": runs.len(),
                                "run_ids": case.run_ids,
                                "newest_run_at": runs.iter().map(|run| run.started_at).max(),
                                "last_used": cases.last_used.as_deref() == Some(case.id.as_str()),
                            })
                        })
                        .collect();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".into())
                    );
                } else if cases.cases.is_empty() {
                    println!("No cases. Start one with rvrs save --case <name>.");
                } else {
                    let style = Style::detect(ColorChoice::Auto);
                    for line in case_lines(&cases, &saved, style, format::now()) {
                        println!("{line}");
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
            Some("new") => {
                let name = one(
                    arguments,
                    1,
                    "rvrs case new needs a name, such as rvrs case new vpn-drop",
                )?;
                extra(2, "new")?;
                cases.create(name).map_err(case_error)?;
                println!("Created the case {}.", name.trim());
                Ok(ExitCode::SUCCESS)
            }
            Some("rename") => {
                let (Some(name), Some(new_name)) = (words.get(1), words.get(2)) else {
                    return Err(usage("rvrs case rename needs the name and the new name, such as rvrs case rename vpn-drop vpn-flap"));
                };
                extra(3, "rename")?;
                let old = cases
                    .find(name)
                    .map_or_else(|| name.trim().to_string(), |case| case.name.clone());
                cases.rename(name, new_name).map_err(case_error)?;
                println!("Renamed {old} to {}.", new_name.trim());
                Ok(ExitCode::SUCCESS)
            }
            Some("delete") => {
                let name = one(
                    arguments,
                    1,
                    "rvrs case delete needs the name of a case. List them with rvrs case.",
                )?;
                extra(2, "delete")?;
                let deleted = cases.delete(name).map_err(case_error)?;
                let held = deleted.run_ids.len();
                if held == 0 {
                    println!("Deleted the case {}.", deleted.name);
                } else {
                    println!(
                        "Deleted the case {}. Its {} stay saved.",
                        deleted.name,
                        count_of(held, "run", "runs")
                    );
                }
                Ok(ExitCode::SUCCESS)
            }
            Some("add") => {
                let name = one(
                    arguments,
                    1,
                    "rvrs case add needs the name of a case, such as rvrs case add vpn-drop",
                )?;
                extra(3, "add")?;
                let run = pick_run(words.get(2).copied())?;
                let message = keep(&run, Some(name))?;
                println!("{message}");
                Ok(ExitCode::SUCCESS)
            }
            Some("remove") => {
                let (Some(name), Some(id)) = (words.get(1), words.get(2)) else {
                    return Err(usage(
                        "rvrs case remove needs the name of a case and the id of a run in it.",
                    ));
                };
                extra(3, "remove")?;
                let case = cases
                    .find(name)
                    .cloned()
                    .ok_or_else(|| case_error(CaseError::Missing(name.trim().to_string())))?;
                let Some(run) = saved.find(id).filter(|run| case.holds(&run.id)).cloned() else {
                    return Err(fail(&format!(
                        "No run in {} has the id \u{201C}{id}\u{201D}. rvrs case {} lists them.",
                        case.name,
                        quoted(&case.name)
                    )));
                };
                cases
                    .remove(&case.id, &run.id)
                    .map_err(|error| fail(&format!("Couldn't save the case ({error}).")))?;
                println!(
                    "Took {} out of {}. It stays saved.",
                    run_name(&run),
                    case.name
                );
                Ok(ExitCode::SUCCESS)
            }
            Some(name) => {
                extra(1, "<name>")?;
                let case = cases
                    .find(name)
                    .cloned()
                    .ok_or_else(|| case_error(CaseError::Missing(name.trim().to_string())))?;
                let runs = runs_in(&saved, &case);
                if json {
                    print_reports(&runs);
                } else if runs.is_empty() {
                    println!(
                        "{} holds no runs. rvrs case add {} puts the newest run in it.",
                        case.name,
                        quoted(&case.name)
                    );
                } else {
                    let runs: Vec<Run> = runs.into_iter().cloned().collect();
                    print_runs(&runs);
                }
                Ok(ExitCode::SUCCESS)
            }
        }
    })();
    outcome.unwrap_or_else(|code| code)
}

/// A case name as it would be typed: in quotes when it has a space.
fn quoted(name: &str) -> String {
    if name.contains(char::is_whitespace) {
        format!("\"{name}\"")
    } else {
        name.to_string()
    }
}

pub fn status() -> ExitCode {
    let style = Style::detect(ColorChoice::Auto);
    let ok = |label: &str, detail: &str| {
        println!("  {} {label:<16} {}", style.green("\u{2713}"), detail)
    };
    let bad =
        |label: &str, detail: &str| println!("  {} {label:<16} {}", style.red("\u{2717}"), detail);
    let off = |label: &str, detail: &str| {
        println!(
            "  {} {label:<16} {}",
            style.dim("\u{2013}"),
            style.dim(detail)
        )
    };

    println!("{} rvrs {}\n", style.bold("//"), crate::help::VERSION);
    let mut healthy = true;
    match IcmpSocket::open(Family::V4) {
        Ok(_) => ok("ICMP over IPv4", "allowed for this user"),
        Err(error) => {
            healthy = false;
            bad("ICMP over IPv4", &format!("not allowed ({error})"));
            if cfg!(target_os = "linux") {
                println!(
                    "    {}",
                    style.dim(
                        "To allow it: sudo sysctl -w net.ipv4.ping_group_range=\"0 2147483647\""
                    )
                );
            }
        }
    }
    match IcmpSocket::open(Family::V6) {
        Ok(_) => ok("ICMP over IPv6", "allowed for this user"),
        Err(error) => bad("ICMP over IPv6", &format!("not allowed ({error})")),
    }
    let v6_route = resolve::source_toward("2606:4700:4700::1111".parse().expect("literal"))
        .is_some_and(|source| !rvrs_core::is_private(source));
    if v6_route {
        ok("IPv6", "this device has a public IPv6 address");
    } else {
        off(
            "IPv6",
            "this device has no public IPv6 address, so names resolve to IPv4",
        );
    }
    let history = History::open();
    let count = history.runs.len();
    let noun = if count == 1 { "run" } else { "runs" };
    ok(
        "History",
        &format!("{count} {noun} \u{00B7} {}", paths::display(history.path())),
    );
    let hosts = Favorites::open().hosts.len();
    ok(
        "Favorites",
        &if hosts == 1 {
            "1 host".to_string()
        } else {
            format!("{hosts} hosts")
        },
    );
    let saved = History::saved();
    let kept = saved.runs.len();
    let case_count = Cases::open(&saved).cases.len();
    ok(
        "Saved",
        &format!(
            "{} \u{00B7} {}",
            count_of(kept, "run", "runs"),
            count_of(case_count, "case", "cases")
        ),
    );
    match crate::update::notice() {
        Some(line) => println!("  {} {:<16} {line}", style.yellow("\u{2191}"), "Update"),
        None => ok(
            "Version",
            &format!("{} ({})", crate::help::VERSION, crate::help::COMMIT),
        ),
    }
    // The build features this build has, by the names a node announces and --with takes.
    ok("Build features", &crate::features::included().join(", "));
    let left_out = crate::features::left_out();
    if !left_out.is_empty() {
        let names = left_out.join(",");
        off(
            "Left out",
            &format!(
                "{} \u{00B7} rvrs update --with {names} adds {}",
                left_out.join(", "),
                if left_out.len() == 1 { "it" } else { "them" }
            ),
        );
    }
    match crate::account::status_line() {
        (true, line) => ok("Account", &line),
        (false, line) => off("Account", &line),
    }
    match crate::remote::status_line() {
        (true, line) => ok("Nodes", &line),
        (false, line) => off("Nodes", &line),
    }
    if healthy {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(crate::EXIT_FAILED)
    }
}

pub fn completions(arguments: &[String]) -> ExitCode {
    match arguments.first().map(String::as_str) {
        Some("install") => {
            let shell = arguments.get(1).cloned().or_else(|| {
                std::env::var("SHELL").ok().and_then(|path| path.rsplit('/').next().map(str::to_string))
            });
            let Some(shell) = shell else {
                return usage("Couldn't tell which shell you use. Name it: rvrs completions install zsh");
            };
            let (Some(script), Some((path, note))) = (completions::script(&shell), completions::install_target(&shell)) else {
                return usage(&format!("rvrs has completions for zsh, bash and fish, and \u{201C}{shell}\u{201D} is none of them."));
            };
            if let Some(parent) = path.parent() {
                if let Err(error) = std::fs::create_dir_all(parent) {
                    return fail(&format!("Couldn't create {} ({error}).", paths::display(parent)));
                }
            }
            if let Err(error) = std::fs::write(&path, script) {
                return fail(&format!("Couldn't write {} ({error}).", paths::display(&path)));
            }
            println!("Installed {shell} completions at {}.", paths::display(&path));
            match note {
                Some(note) => println!("{note}"),
                None => println!("They load in the next shell you open."),
            }
            ExitCode::SUCCESS
        }
        Some(shell) => match completions::script(shell) {
            Some(script) => {
                print!("{script}");
                ExitCode::SUCCESS
            }
            None => usage(&format!("rvrs has completions for zsh, bash and fish, and \u{201C}{shell}\u{201D} is none of them.")),
        },
        None => usage("rvrs completions takes install, or zsh, bash or fish to print that shell's script."),
    }
}

/// For shell completion: targets from history, most recent first.
pub fn targets() -> ExitCode {
    let history = History::open();
    // Favorites first, then history, each host once.
    let favorites = Favorites::open();
    for host in &favorites.hosts {
        println!("{host}");
    }
    for target in history
        .targets()
        .into_iter()
        .filter(|target| !favorites.contains(target))
        .take(50)
    {
        println!("{target}");
    }
    ExitCode::SUCCESS
}

/// For shell completion: the names of the cases, the one used last first.
pub fn case_names() -> ExitCode {
    let saved = History::saved();
    let cases = Cases::open(&saved);
    let last = cases.last_used().map(|case| case.id.clone());
    let mut names: Vec<&rvrs_core::cases::Case> = cases.cases.iter().collect();
    names.sort_by_key(|case| Some(&case.id) != last.as_ref());
    for case in names {
        println!("{}", case.name);
    }
    ExitCode::SUCCESS
}

/// For shell completion: "id:target · summary" per run.
pub fn runs() -> ExitCode {
    let history = History::open();
    let saved = History::saved();
    let kept = saved.runs.iter().filter(|run| !history.contains(&run.id));
    for run in history.runs.iter().take(50).chain(kept) {
        println!("{}:{} \u{00B7} {}", run.id, run.target, run.summary());
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pretty::tests::sample;

    #[test]
    fn history_rows_carry_id_tool_target_summary_origin_and_age() {
        let mut run = sample();
        run.id = "abc234".into();
        run.started_at = 1000;
        run.source = Some("192.168.1.20".parse().unwrap());
        run.public_source = Some("203.0.113.9".parse().unwrap());
        let lines = history_lines(&[run.clone()], Style::PLAIN, 120, 1000 + 5 * 3600);
        assert_eq!(
            lines[0],
            "ID      TOOL        TARGET           SUMMARY           SOURCE        PUBLIC       AGE"
        );
        assert_eq!(lines[1], "abc234  traceroute  one.one.one.one  4 hops \u{00B7} 12.4 ms  192.168.1.20  203.0.113.9  5h ago");
        // A narrow terminal keeps the run and drops where it came from.
        let narrow = history_lines(&[run], Style::PLAIN, 70, 1000 + 5 * 3600);
        assert!(!narrow[0].contains("SOURCE"));
        assert!(narrow[1].ends_with("4 hops \u{00B7} 12.4 ms  5h ago"));
    }

    #[test]
    fn history_is_filtered_by_tool_and_target() {
        use rvrs_core::{model::Tool, FamilyChoice};
        let mtr = Run::new(Tool::Mtr, "api.Example.com", FamilyChoice::Auto, 1, 30);
        let dig = Run::new(Tool::Dns, "example.net", FamilyChoice::Auto, 0, 0);
        assert!(matches_filter(&mtr, &[]));
        assert!(matches_filter(&mtr, &["mtr"]));
        assert!(matches_filter(&mtr, &["example.com"]));
        assert!(matches_filter(&mtr, &["MTR", "api"]));
        assert!(!matches_filter(&mtr, &["ping"]));
        assert!(!matches_filter(&mtr, &["mtr", "example.net"]));
        // dig runs answer to both of the command's names.
        assert!(matches_filter(&dig, &["dig"]));
        assert!(matches_filter(&dig, &["dns", "example"]));
    }
}
