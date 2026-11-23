//! `rvrs bgp`: the app's BGP lookup on the command line. A hostname, an address, a prefix or an
//! AS number goes in. The route, its origin validation, the AS paths collectors see, its history
//! and the network behind it come out, each part as its source answers. `rvrs bgp check` asks
//! whether one route would pass origin validation, and `rvrs bgp glass` keeps the user's looking
//! glasses, which every lookup of a route asks too.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::time::Duration;

use rvrs_core::bgp::glass::{self, GlassKind, GlassResult, LookingGlass};
use rvrs_core::bgp::result::{self, count, counted, date, date_time, no_route_line};
use rvrs_core::bgp::{
    BgpNetwork, BgpResult, Glasses, Grade, Path, RpkiStatus, Side, Target, TargetKind,
    TraceComparison, Validation,
};
use rvrs_core::format;
use rvrs_core::history::History;
use rvrs_core::model::{Outcome, Run};
use rvrs_core::report::Report;
use rvrs_core::session::{Lookups, Session};

use crate::args::{self, Common};
use crate::pretty;
use crate::style::{truncate_end, truncate_middle, visible_width, ColorChoice, Style, SPINNER};
use crate::term::{self, LiveRegion};
use crate::trace::{self, Look};
use crate::EXIT_USAGE;

/// The prefix a looking glass test asks for when none is given: one every full table carries.
const TEST_PREFIX: &str = "1.1.1.0/24";

/// Which parts of the result print. With none chosen it is Overview, as the app opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Parts {
    pub paths: bool,
    pub neighbors: bool,
    pub prefixes: bool,
    pub history: bool,
    pub rpki: bool,
    pub sources: bool,
    pub all: bool,
}

impl Parts {
    fn overview(self) -> bool {
        self.all || self == Parts::default()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BgpArgs {
    pub target: String,
    pub parts: Parts,
    /// How far back History looks: 7, 30 or 90 days.
    pub days: u64,
    pub no_lookups: bool,
    pub no_glasses: bool,
    pub common: Common,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GlassCommand {
    List,
    Add {
        host: String,
        kind: Option<GlassKind>,
        name: Option<String>,
        routers: Vec<String>,
    },
    Remove(String),
    Test {
        host: String,
        prefix: Option<String>,
        router: Option<String>,
        raw: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Run(Box<BgpArgs>),
    Check {
        prefix: String,
        asn: u32,
        json: bool,
        color: ColorChoice,
    },
    Glass(GlassCommand, ColorChoice),
    Help,
    Version,
}

pub fn parse(arguments: &[String]) -> Result<Parsed, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut parts = Parts::default();
    let mut days = 7;
    let mut no_lookups = false;
    let mut no_glasses = false;
    let mut common = Common::default();
    let mut kind = None;
    let mut name = None;
    let mut routers: Vec<String> = Vec::new();
    let mut raw = false;

    let mut index = 0;
    let value = |index: &mut usize, flag: &str| -> Result<String, String> {
        *index += 1;
        arguments
            .get(*index)
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value."))
    };
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        match argument {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-V" | "--version" => return Ok(Parsed::Version),
            "--json" => common.json = true,
            "--classic" => common.classic = true,
            "--fleet" => common.fleet = true,
            "--no-lookups" => no_lookups = true,
            "--no-glasses" => no_glasses = true,
            "--paths" => parts.paths = true,
            "--neighbors" => parts.neighbors = true,
            "--prefixes" => parts.prefixes = true,
            "--history" => parts.history = true,
            "--rpki" => parts.rpki = true,
            "--sources" => parts.sources = true,
            "--all" => parts.all = true,
            "--raw" => raw = true,
            "--days" => {
                let text = value(&mut index, "--days")?;
                days = match text.as_str() {
                    "7" => 7,
                    "30" => 30,
                    "90" => 90,
                    _ => return Err("--days takes 7, 30 or 90.".to_string()),
                };
                parts.history = true;
            }
            "--color" => {
                let text = value(&mut index, "--color")?;
                common.color = ColorChoice::parse(&text)
                    .ok_or_else(|| "--color takes auto, always or never.".to_string())?;
            }
            "--node" => common.node = Some(value(&mut index, "--node")?),
            "--case" => common.case = Some(args::case_name(&value(&mut index, "--case")?)?),
            "--type" => {
                let text = value(&mut index, "--type")?;
                kind = Some(GlassKind::from_id(&text).ok_or_else(|| {
                    "--type takes alice-lg, lg-cgi, hyperglass or bird-lg-go.".to_string()
                })?);
            }
            "--name" => name = Some(value(&mut index, "--name")?),
            "--router" => {
                let text = value(&mut index, "--router")?;
                routers.extend(
                    text.split(',')
                        .map(str::trim)
                        .filter(|router| !router.is_empty())
                        .map(str::to_string),
                );
            }
            flag if flag.starts_with('-') && flag.len() > 1 => {
                return Err(format!(
                    "rvrs bgp doesn't take {flag}. Its flags: rvrs bgp --help"
                ));
            }
            _ => positional.push(argument.to_string()),
        }
        index += 1;
    }

    let words: Vec<&str> = positional.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] => Err(
            "rvrs bgp needs a hostname, an address, a prefix or an AS number: rvrs bgp AS64500"
                .to_string(),
        ),
        ["check", rest @ ..] => {
            let problem = "rvrs bgp check takes a prefix with its length, such as 203.0.113.0/24, and an AS number.";
            let [prefix, asn] = rest else {
                return Err(problem.to_string());
            };
            let prefix = Target::parse(prefix)
                .filter(|target| target.kind == TargetKind::Prefix)
                .ok_or(problem)?;
            let asn = result::as_number(asn).ok_or(problem)?;
            Ok(Parsed::Check {
                prefix: prefix.canonical,
                asn,
                json: common.json,
                color: common.color,
            })
        }
        ["glass" | "glasses" | "looking-glass" | "looking-glasses" | "lg", rest @ ..] => {
            let host = |raw: &str| {
                LookingGlass::host_from(raw)
                    .ok_or_else(|| format!("\u{201C}{raw}\u{201D} isn't a host name."))
            };
            let command = match rest {
                [] | ["list"] => GlassCommand::List,
                ["add", raw_host] => GlassCommand::Add {
                    host: host(raw_host)?,
                    kind,
                    name,
                    routers,
                },
                ["remove" | "rm", raw_host] => GlassCommand::Remove(host(raw_host)?),
                ["test", raw_host, rest @ ..] if rest.len() <= 1 => {
                    let prefix = match rest.first() {
                        Some(text) => Some(
                            Target::parse(text)
                                .filter(|target| target.kind == TargetKind::Prefix)
                                .map(|target| target.canonical)
                                .ok_or_else(|| {
                                    format!("\u{201C}{text}\u{201D} isn't a prefix with its length, such as 203.0.113.0/24.")
                                })?,
                        ),
                        None => None,
                    };
                    GlassCommand::Test {
                        host: host(raw_host)?,
                        prefix,
                        router: routers.into_iter().next(),
                        raw,
                    }
                }
                _ => {
                    return Err("rvrs bgp glass takes add <host>, remove <host>, test <host> [prefix], or nothing to list them.".to_string());
                }
            };
            Ok(Parsed::Glass(command, common.color))
        }
        [target] => Ok(Parsed::Run(Box::new(BgpArgs {
            target: (*target).to_string(),
            parts,
            days,
            no_lookups,
            no_glasses,
            common,
        }))),
        [_, extra, ..] => Err(format!(
            "rvrs bgp looks up one target at a time, and \u{201C}{extra}\u{201D} is a second one."
        )),
    }
}

pub fn run(args: BgpArgs) -> ExitCode {
    if args.common.fleet {
        return crate::usage(trace::NO_FLEET);
    }
    if let Some(node) = &args.common.node {
        return crate::remote::run(
            node,
            "bgp",
            &args.target,
            crate::remote::bgp(),
            &args.common,
        );
    }
    let Some(target) = Target::parse(&args.target) else {
        return crate::usage(&format!(
            "\u{201C}{}\u{201D} isn't a hostname, an address, a prefix or an AS number.",
            args.target
        ));
    };
    let look = trace::look(args.common.json, args.common.classic, false);
    let style = Style::detect(args.common.color);
    let glasses = if args.no_glasses {
        Vec::new()
    } else {
        Glasses::open().list
    };
    let lookups = Lookups {
        names: false,
        networks: !args.no_lookups && look == Look::Pretty,
    };
    let mut history = History::open();

    term::catch_interrupts();
    let mut session = Session::start_bgp(&target, glasses, lookups);
    let mut live = (look == Look::Pretty).then(LiveRegion::start);
    let mut frame = 0usize;
    let mut stop_sent = false;
    let view = |run: &Run, history: &History, frame: usize| {
        let (width, _) = term::size();
        let lines = result_lines(&View {
            run,
            trace: last_trace(history, run),
            parts: args.parts,
            days: args.days,
            style,
            width,
            frame,
            now: format::now(),
        });
        lines
    };
    loop {
        session.pump(Duration::from_millis(80));
        frame += 1;
        match term::interrupts() {
            0 => {}
            1 if !stop_sent => {
                session.stop();
                stop_sent = true;
            }
            1 => {}
            _ => {
                drop(live);
                return ExitCode::from(130);
            }
        }
        if let Some(region) = live.as_mut() {
            // A result taller than the terminal shows its top while it fills in. The whole of
            // it is printed once the lookup has ended.
            let (_, rows) = term::size();
            let mut lines = view(&session.run, &history, frame);
            lines.truncate(rows.saturating_sub(2));
            region.draw(&lines, true);
        }
        if !session.run.is_running() {
            break;
        }
    }
    // The network this device is in marks its own routes among the paths. It is worth a moment.
    session.finish_lookups(Duration::from_millis(600));
    let run = &session.run;

    if let Some(Outcome::Failed(message)) = &run.outcome {
        if let Some(mut region) = live.take() {
            region.draw(&[], false);
        }
        if look == Look::Json {
            println!("{}", Report::of(run).json());
        }
        eprintln!("{}", trace::style_error(Style::detect_stderr(), message));
        return ExitCode::from(EXIT_USAGE);
    }

    let found = run.bgp.as_ref().is_some_and(|bgp| !bgp.is_empty());
    let keep = std::env::var("RVRS_HISTORY").map_or(true, |value| value != "0");
    match look {
        Look::Pretty => {
            if let Some(mut region) = live.take() {
                region.draw(&view(run, &history, 0), false);
                region.finish();
            }
        }
        Look::Json => println!("{}", Report::of(run).json()),
        Look::Classic => {
            if let Some(bgp) = &run.bgp {
                println!("{}", bgp.export(&run.target, run.started_at));
                // The export is the app's Copy Results. A part asked for by flag is what a
                // script or an agent wants, so a pipe gets it too, as plain text.
                let plain = Style::detect(ColorChoice::Never);
                let parts = args.parts;
                let now = format::now();
                let width = 100;
                let mut sections: Vec<Vec<String>> = Vec::new();
                if parts.rpki || parts.all {
                    for origin in &bgp.origins {
                        if let Some(validation) = bgp.validation_for(origin.asn) {
                            sections.push(validation_lines(validation, plain, width, now));
                        }
                    }
                }
                if (parts.neighbors || parts.all) && bgp.is_network() {
                    sections.extend(neighbors_part(bgp, run, plain, width));
                }
                if parts.prefixes || parts.all {
                    sections.extend(prefixes_part(bgp, run, plain, width));
                }
                if (parts.history || parts.all) && !bgp.is_network() && bgp.prefix.is_some() {
                    sections.extend(history_part(bgp, run, args.days, now, plain, width));
                }
                for section in sections {
                    println!();
                    for line in section {
                        println!("{}", line.trim_end());
                    }
                }
            }
        }
    }
    let _ = std::io::stdout().flush();
    if keep && found {
        crate::pulse::count_run(run.tool);
        crate::sync::queue(run);
        if let Err(error) = history.save(run) {
            eprintln!("rvrs: Couldn't save this run to history ({error}).");
        }
    }
    if let Some(case) = args.common.case.as_deref().filter(|_| found) {
        crate::commands::save_after_run(run, case);
    }
    if run.outcome == Some(Outcome::Reached) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// The newest traceroute or MTR to what the lookup was asked about, to lay against its paths.
pub fn last_trace<'a>(history: &'a History, run: &Run) -> Option<&'a Run> {
    let bgp = run.bgp.as_ref()?;
    match bgp.kind {
        TargetKind::Host => history.last_trace(&bgp.query),
        TargetKind::Address => history.last_trace(bgp.address.as_deref().unwrap_or(&bgp.query)),
        TargetKind::Prefix | TargetKind::Asn => None,
    }
}

/// Everything `result_lines` needs to draw one frame.
pub struct View<'a> {
    pub run: &'a Run,
    pub trace: Option<&'a Run>,
    pub parts: Parts,
    pub days: u64,
    pub style: Style,
    pub width: usize,
    pub frame: usize,
    pub now: u64,
}

fn header(style: Style, text: &str) -> String {
    style.bold(text)
}

fn graded(style: Style, grade: Grade, text: &str) -> String {
    match grade {
        Grade::Good => style.bold_green(text),
        Grade::Attention => style.yellow(text),
        Grade::Bad => style.red(text),
        Grade::Neutral => style.bold(text),
    }
}

pub fn badge(style: Style, status: RpkiStatus) -> String {
    graded_plain(style, status.grade(), status.badge())
}

fn graded_plain(style: Style, grade: Grade, text: &str) -> String {
    match grade {
        Grade::Good => style.green(text),
        Grade::Attention => style.yellow(text),
        Grade::Bad => style.red(text),
        Grade::Neutral => style.dim(text),
    }
}

/// "AS64510 › AS64501 › AS64500", the last network in bold.
pub fn chain(asns: &[u32], style: Style) -> String {
    let last = asns.len().saturating_sub(1);
    asns.iter()
        .enumerate()
        .map(|(index, asn)| {
            let text = format!("AS{asn}");
            if index == last {
                style.bold(&text)
            } else {
                text
            }
        })
        .collect::<Vec<_>>()
        .join(&style.dim(" \u{203A} "))
}

/// Label and value rows, the labels in one column.
fn rows(pairs: &[(String, String)], style: Style) -> Vec<String> {
    let widest = pairs
        .iter()
        .map(|(label, _)| visible_width(label))
        .max()
        .unwrap_or(0);
    pairs
        .iter()
        .map(|(label, value)| {
            let pad = " ".repeat(widest - visible_width(label));
            format!("  {}{pad}  {value}", style.dim(label))
        })
        .collect()
}

fn note(text: &str, style: Style, width: usize) -> Vec<String> {
    pretty::wrap(text, width.saturating_sub(2))
        .into_iter()
        .map(|line| format!("  {}", style.dim(&line)))
        .collect()
}

/// Two parts on one line where they fit, and the second under the first where they don't.
fn pair(left: String, right: String, width: usize) -> Vec<String> {
    if visible_width(&left) + 2 + visible_width(&right) <= width {
        vec![format!("{left}  {right}")]
    } else {
        vec![left.trim_end().to_string(), format!("      {right}")]
    }
}

fn padded(text: &str, cells: usize) -> String {
    let short = cells.saturating_sub(visible_width(text));
    format!("{text}{}", " ".repeat(short))
}

/// A bar of `cells` for a share of 0...1.
pub fn share_bar(fraction: f64, cells: usize) -> String {
    pretty::bar(
        Duration::from_secs_f64(fraction.clamp(0.0, 1.0)),
        Duration::from_secs(1),
        cells,
    )
}

/// One cell per value, as tall as the value is against the largest. Zero is a blank.
pub fn spark(values: &[f64]) -> String {
    const LEVELS: [char; 8] = [
        '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];
    let top = values.iter().copied().fold(0.0, f64::max);
    values
        .iter()
        .map(|value| {
            if *value <= 0.0 || top <= 0.0 {
                ' '
            } else {
                LEVELS[(((value / top) * 7.0).round() as usize).min(7)]
            }
        })
        .collect()
}

/// A timeline lane: a mark in every cell some stretch touches.
pub fn lane(stretches: &[(u64, u64)], since: u64, now: u64, cells: usize) -> String {
    let step = now.saturating_sub(since) as f64 / cells.max(1) as f64;
    (0..cells)
        .map(|cell| {
            let from = since as f64 + cell as f64 * step;
            let to = from + step;
            let touched = stretches
                .iter()
                .any(|(start, end)| (*start as f64) < to && (*end as f64) > from);
            if touched {
                '\u{2501}'
            } else {
                ' '
            }
        })
        .collect()
}

fn title_line(run: &Run, style: Style, width: usize) -> String {
    let lead = format!("{} {} ", style.bold("//"), style.dim("bgp"));
    let room = width.saturating_sub(visible_width(&lead) + 18);
    format!(
        "{lead}{}  {}",
        style.bold(&truncate_middle(&run.target, room)),
        style.dim("route collectors")
    )
}

/// The whole result, top to bottom. While the lookup runs, the verdict's place is taken by how
/// many sources have answered.
pub fn result_lines(view: &View) -> Vec<String> {
    let run = view.run;
    let style = view.style;
    let width = view.width.clamp(40, 120);
    let mut out = vec![String::new(), title_line(run, style, width), String::new()];
    let empty = BgpResult::new(TargetKind::Host, &run.target);
    let bgp = run.bgp.as_ref().unwrap_or(&empty);

    if run.is_running() {
        let spinner = style.blue(SPINNER[view.frame % SPINNER.len()]);
        out.push(format!("{spinner} {}", bgp.progress()));
    } else if bgp.is_empty() {
        out.push(style.yellow(if run.outcome == Some(Outcome::Stopped) {
            "Stopped"
        } else {
            "No result"
        }));
        return out;
    } else {
        let verdict = bgp.verdict();
        out.push(graded(style, verdict.grade, &verdict.title));
        out.extend(pretty::wrap(&verdict.detail, width));
    }
    if let Some(tiles) = tiles(bgp, style, run.is_running()) {
        let line = tiles.join(&style.dim(" \u{00B7} "));
        if visible_width(&line) <= width {
            out.push(line);
        } else {
            out.extend(tiles);
        }
    }

    let parts = view.parts;
    let mut sections: Vec<Vec<String>> = Vec::new();
    if parts.overview() {
        if bgp.is_network() {
            sections.extend(network_overview(bgp, run.own_asn(), style, width));
        } else {
            sections.extend(route_overview(view, bgp, width));
        }
    }
    if parts.rpki || parts.all {
        for origin in &bgp.origins {
            if let Some(validation) = bgp.validation_for(origin.asn) {
                sections.push(validation_lines(validation, style, width, view.now));
            }
        }
    }
    if (parts.paths || parts.all) && !bgp.is_network() && bgp.prefix.is_some() {
        sections.extend(paths_part(bgp, run, style, width));
    }
    if (parts.neighbors || parts.all) && bgp.is_network() {
        sections.extend(neighbors_part(bgp, run, style, width));
    }
    if parts.prefixes || parts.all {
        sections.extend(prefixes_part(bgp, run, style, width));
    }
    if (parts.history || parts.all) && !bgp.is_network() && bgp.prefix.is_some() {
        sections.extend(history_part(bgp, run, view.days, view.now, style, width));
    }
    if parts.sources || parts.all {
        sections.push(sources_part(bgp, style, width));
    } else if parts.overview() && !bgp.sources.is_empty() {
        sections.push(vec![
            header(style, "Sources"),
            format!("  {}", sources_line(bgp, style, view.now)),
        ]);
    }
    for section in sections.into_iter().filter(|section| !section.is_empty()) {
        out.push(String::new());
        out.extend(section);
    }
    out
}

fn tiles(bgp: &BgpResult, style: Style, running: bool) -> Option<Vec<String>> {
    let placeholder = if running { "\u{2026}" } else { "\u{2014}" };
    let tile = |label: &str, value: String| format!("{} {value}", style.dim(label));
    let tiles = if bgp.is_network() {
        let network = bgp.network.as_ref()?;
        let prefixes = |count_of: Option<u64>| {
            count_of.map_or(placeholder.to_string(), |number| {
                counted(number, "prefix", "prefixes")
            })
        };
        let roa = network
            .coverage()
            .map_or(placeholder.to_string(), |(valid, checked)| {
                let text = format!("{valid}/{checked} checked");
                if network.invalid_count() > 0 {
                    style.red(&text)
                } else {
                    text
                }
            });
        vec![
            tile("IPv4", prefixes(network.v4_count)),
            tile("IPv6", prefixes(network.v6_count)),
            tile("ROA", roa),
        ]
    } else {
        bgp.prefix
            .as_ref()
            .filter(|_| bgp.unrouted_reason.is_none())?;
        let seen = match (bgp.seeing, bgp.total_peers) {
            (Some(seeing), Some(total)) => {
                let text = format!("{seeing}/{total} peers");
                if bgp.visibility().is_some_and(|share| share < 0.8) {
                    style.yellow(&text)
                } else {
                    text
                }
            }
            _ => placeholder.to_string(),
        };
        vec![
            tile("Seen by", seen),
            tile(
                "Paths",
                if bgp.path_count > 0 {
                    bgp.path_count.to_string()
                } else {
                    placeholder.to_string()
                },
            ),
            tile(
                "First seen",
                bgp.first_seen
                    .map_or(placeholder.to_string(), |seen| date(seen)[..4].to_string()),
            ),
        ]
    };
    Some(tiles)
}

fn route_overview(view: &View, bgp: &BgpResult, width: usize) -> Vec<Vec<String>> {
    let style = view.style;
    let run = view.run;
    let own = run.own_asn();
    let mut sections = Vec::new();

    if let Some(prefix) = &bgp.prefix {
        let mut pairs: Vec<(String, String)> = Vec::new();
        if bgp.kind == TargetKind::Prefix {
            pairs.push(("Prefix".into(), prefix.clone()));
        } else {
            if let Some(address) = &bgp.address {
                pairs.push(("Address".into(), address.clone()));
            }
            pairs.push(("Announced prefix".into(), prefix.clone()));
        }
        for origin in &bgp.origins {
            pairs.push((
                origin.holder.clone().unwrap_or_else(|| "Origin AS".into()),
                format!("AS{}", origin.asn),
            ));
            let label = if bgp.origins.len() > 1 {
                format!("RPKI for AS{}", origin.asn)
            } else {
                "RPKI".to_string()
            };
            match bgp.validation_for(origin.asn) {
                Some(validation) => pairs.push((label, badge(style, validation.status))),
                None if run.is_running() => pairs.push((label, "\u{2026}".into())),
                None => {}
            }
        }
        if bgp.origins.is_empty() && run.is_running() {
            pairs.push(("Origin AS".into(), "\u{2026}".into()));
        }
        if let Some(origin) = bgp.origin() {
            let objects = if origin.route_objects.is_empty() {
                "None on record".to_string()
            } else {
                origin
                    .route_objects
                    .iter()
                    .map(|registry| registry.to_uppercase())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            pairs.push(("Route object".into(), objects));
        }
        if let Some(registry) = &bgp.registry {
            pairs.push(("Registry".into(), registry.clone()));
        }
        for glass in &bgp.glasses {
            pairs.push((glass.name.clone(), glass.summary()));
        }
        let mut section = vec![header(style, "Route")];
        section.extend(rows(&pairs, style));
        sections.push(section);
    }

    if bgp.addresses.len() > 1 {
        let widest = bgp
            .addresses
            .iter()
            .map(|entry| entry.address.len())
            .max()
            .unwrap_or(0);
        let mut section = vec![header(style, "Addresses")];
        for entry in &bgp.addresses {
            let mut route: Vec<String> = entry.prefix.iter().cloned().collect();
            route.extend(entry.asn.map(|asn| format!("AS{asn}")));
            let route = if route.is_empty() {
                "No route observed".to_string()
            } else {
                route.join(" \u{00B7} ")
            };
            let shown = if Some(&entry.address) == bgp.address.as_ref() {
                format!("  {}", style.dim("Shown"))
            } else {
                String::new()
            };
            section.push(format!(
                "  {}  {}{shown}",
                padded(&entry.address, widest),
                style.dim(&route)
            ));
        }
        sections.push(section);
    }

    if !bgp.upstreams.is_empty() {
        let top = bgp.upstreams[0].peers.max(1);
        let mut section = vec![header(style, "Reached through")];
        for share in bgp.upstreams.iter().take(4) {
            section.push(share_row(
                bgp,
                share.asn,
                share.peers as f64 / top as f64,
                Some(format::percent(bgp.share(share.peers))),
                own,
                style,
                width,
            ));
        }
        sections.push(section);
    }

    if bgp.origin().is_some() && matches!(bgp.kind, TargetKind::Host | TargetKind::Address) {
        let comparison = view
            .trace
            .and_then(|trace| Some((trace, TraceComparison::new(trace, bgp)?)));
        let mut section = vec![header(style, "Your path")];
        match comparison {
            Some((trace, comparison)) => {
                section.push(format!("  {}", chain(&comparison.trace_asns, style)));
                section.extend(note(&comparison.sentence, style, width));
                section.push(format!(
                    "  {}",
                    style.dim(&format!(
                        "{} \u{00B7} {}",
                        trace.tool.title(),
                        format::relative(trace.started_at, view.now)
                    ))
                ));
            }
            None => {
                let target = match bgp.kind {
                    TargetKind::Host => bgp.query.as_str(),
                    _ => bgp.address.as_deref().unwrap_or(&bgp.query),
                };
                section.push(format!(
                    "  {}",
                    style.dim(&format!("rvrs traceroute {target}"))
                ));
            }
        }
        sections.push(section);
    }

    if let Some(network) = &bgp.network {
        let mut section = vec![header(style, "Network")];
        section.push(format!("  About AS{}", network.asn));
        section.extend(note(&network.line(), style, width));
        sections.push(section);
    }

    if bgp.prefix.is_some() && (!bgp.spans.is_empty() || !bgp.activity.is_empty()) {
        let changes = bgp.changes(7, view.now);
        let mut section = vec![header(style, "Recent changes")];
        section.push(if changes.is_empty() {
            "  No origin AS changes in 7 days".to_string()
        } else {
            format!(
                "  {}",
                style.yellow(&format!(
                    "{} {} in 7 days",
                    changes.len(),
                    if changes.len() == 1 {
                        "change"
                    } else {
                        "changes"
                    }
                ))
            )
        });
        if !bgp.activity.is_empty() {
            section.extend(note(
                &format!(
                    "{} BGP updates seen by collectors in that time.",
                    count(bgp.updates())
                ),
                style,
                width,
            ));
        }
        sections.push(section);
    }

    if !bgp.related.is_empty() {
        let own_origin = bgp.origin().map(|origin| origin.asn);
        let widest = bgp
            .related
            .iter()
            .take(8)
            .map(|related| related.prefix.len())
            .max()
            .unwrap_or(0);
        let mut section = vec![header(style, "Related prefixes")];
        let mut takes_traffic = false;
        for related in bgp.related.iter().take(8) {
            let mut facts = vec![if related.is_more_specific {
                "More specific".to_string()
            } else {
                "Covering".to_string()
            }];
            facts.extend(related.origin.map(|asn| format!("AS{asn}")));
            let other = related.origin.is_some() && related.origin != own_origin;
            takes_traffic |= other && related.is_more_specific;
            let tag = if other && own_origin.is_some() {
                format!("  {}", style.yellow("Other origin AS"))
            } else {
                String::new()
            };
            section.push(format!(
                "  {}  {}{tag}",
                padded(&related.prefix, widest),
                style.dim(&facts.join(" \u{00B7} "))
            ));
        }
        if takes_traffic {
            section.extend(note("A more specific prefix from another origin AS takes the traffic for its part of the range.", style, width));
        }
        sections.push(section);
    }
    sections
}

/// One network with a bar for its share: the number, the holder, the bar and the figure.
fn share_row(
    bgp: &BgpResult,
    asn: u32,
    fraction: f64,
    figure: Option<String>,
    own: Option<u32>,
    style: Style,
    width: usize,
) -> String {
    // A narrow terminal gets a shorter bar, so the holder keeps some room.
    let bar_cells = if width < 56 { 6 } else { 12 };
    let number = padded(&format!("AS{asn}"), 12);
    let mut name = bgp.name_of(asn).unwrap_or("").to_string();
    if Some(asn) == own {
        name = if name.is_empty() {
            "Your network".to_string()
        } else {
            format!("{name} \u{00B7} Your network")
        };
    }
    let figure = figure.map_or(String::new(), |figure| format!("  {figure:>6}"));
    let room = width.saturating_sub(2 + 12 + 2 + bar_cells + 2 + visible_width(&figure));
    let name_width = room.clamp(4, 36);
    format!(
        "  {number}  {}  {}{}",
        padded(&truncate_end(&name, name_width), name_width),
        padded(&share_bar(fraction, bar_cells), bar_cells),
        style.dim(&figure)
    )
}

fn network_overview(
    bgp: &BgpResult,
    own: Option<u32>,
    style: Style,
    width: usize,
) -> Vec<Vec<String>> {
    let Some(network) = &bgp.network else {
        return Vec::new();
    };
    let mut pairs: Vec<(String, String)> = vec![(
        "Holder".into(),
        network.holder.clone().unwrap_or_else(|| {
            if bgp.pending > 0 {
                "\u{2026}".into()
            } else {
                "Not listed".into()
            }
        }),
    )];
    if let Some(registry) = &network.registry {
        pairs.push(("Registry".into(), registry.clone()));
    }
    if let Some(country) = &network.country {
        pairs.push(("Country".into(), country.clone()));
    }
    if let Some(announced) = network.announced {
        pairs.push((
            "In public routing".into(),
            if announced { "Seen" } else { "Not seen" }.into(),
        ));
    }
    pairs.push((
        "Whois".into(),
        format!("https://stat.ripe.net/AS{}", network.asn),
    ));
    let mut registration = vec![header(style, "Registration")];
    registration.extend(rows(&pairs, style));
    let mut sections = vec![registration];

    let upstreams = network.side(Side::Left);
    if !upstreams.is_empty() {
        let top = upstreams[0].power.max(1);
        let mut section = vec![header(style, "Seen upstream")];
        for neighbor in upstreams.iter().take(4) {
            section.push(share_row(
                bgp,
                neighbor.asn,
                neighbor.power as f64 / top as f64,
                None,
                own,
                style,
                width,
            ));
        }
        sections.push(section);
    }
    sections
}

fn path_row(bgp: &BgpResult, path: &Path, style: Style) -> Vec<String> {
    let mut facts = bgp.path_line(path);
    if path.prepended {
        facts.push_str(" \u{00B7} prepending");
    }
    vec![
        format!(
            "  {:>6}  {}",
            format::percent(bgp.share(path.peers)),
            chain(&path.asns, style)
        ),
        format!("          {}", style.dim(&facts)),
    ]
}

fn paths_part(bgp: &BgpResult, run: &Run, style: Style, width: usize) -> Vec<Vec<String>> {
    let mut sections = Vec::new();
    if bgp.paths.is_empty() {
        let line = if run.is_running() {
            "Getting collector paths\u{2026}".to_string()
        } else {
            "No collector peer reported a route for this prefix.".to_string()
        };
        sections.push(vec![
            header(style, "Observed AS paths"),
            format!("  {}", style.dim(&line)),
        ]);
    } else {
        let tiles = [
            format!(
                "{} {}",
                style.dim("Shortest"),
                counted(bgp.shortest_path as u64, "network", "networks")
            ),
            format!(
                "{} {}",
                style.dim("Typical"),
                counted(bgp.typical_path as u64, "network", "networks")
            ),
            format!(
                "{} {} of peers",
                style.dim("Direct"),
                format::percent(bgp.share(bgp.direct_peers))
            ),
        ];
        let line = tiles.join(&style.dim(" \u{00B7} "));
        let mut figures = if visible_width(&line) <= width {
            vec![line]
        } else {
            tiles.to_vec()
        };
        if let Some(summary) = bgp.path_summary() {
            figures.extend(pretty::wrap(&summary, width));
        }
        sections.push(figures);

        if let Some(own) = run.own_asn() {
            let from_own: Vec<&Path> = bgp
                .paths
                .iter()
                .filter(|path| path.asns.first() == Some(&own))
                .collect();
            if !from_own.is_empty() {
                let mut section = vec![header(style, "From your network")];
                for path in &from_own {
                    section.extend(path_row(bgp, path, style));
                }
                section.extend(note(
                    &if from_own[0].asns.len() == 1 {
                        format!("This device is in AS{own}, which announces the prefix itself.")
                    } else {
                        format!("Reported by a collector peer inside AS{own}, the network this device is in. Your packets likely leave the same way.")
                    },
                    style,
                    width,
                ));
                sections.push(section);
            }
        }

        let mut section = vec![header(style, "Observed AS paths")];
        for path in &bgp.paths {
            section.extend(path_row(bgp, path, style));
        }
        section.extend(note(
            &format!(
                "{} distinct {} from {} peers at {} collectors.",
                bgp.path_count,
                if bgp.path_count == 1 { "path" } else { "paths" },
                bgp.path_peers,
                bgp.collector_count
            ),
            style,
            width,
        ));
        sections.push(section);
    }
    for glass in &bgp.glasses {
        sections.push(glass_lines(
            glass,
            bgp.prefix.as_deref().unwrap_or("this prefix"),
            style,
            width,
        ));
    }
    sections
}

/// "192.0.2.0/24 · from Example Net · 2 route servers · RPKI valid", under one path a looking
/// glass holds.
pub fn glass_path_line(glass: &GlassResult, path: &glass::GlassPath) -> String {
    let servers = path.route_servers.len();
    let mut facts: Vec<String> = path.network.iter().cloned().collect();
    facts.extend(
        path.neighbor_name
            .as_ref()
            .map(|name| format!("from {name}")),
    );
    facts.push(format!(
        "{servers} {}{}",
        glass.unit(),
        if servers == 1 { "" } else { "s" }
    ));
    facts.extend(match path.rpki.as_deref() {
        Some("valid") => Some("RPKI valid".to_string()),
        Some("invalid") => Some("RPKI invalid".to_string()),
        Some("unknown") => Some("No ROA".to_string()),
        _ => None,
    });
    facts.join(" \u{00B7} ")
}

/// "Filtered", "Best" or "Active", when the path has a state worth a word.
pub fn glass_path_state(glass: &GlassResult, path: &glass::GlassPath) -> Option<&'static str> {
    if path.is_filtered {
        Some("Filtered")
    } else if path.is_best {
        Some(if glass.kind == GlassKind::LgCgi {
            "Active"
        } else {
            "Best"
        })
    } else {
        None
    }
}

/// What one looking glass holds for the prefix.
fn glass_lines(glass: &GlassResult, prefix: &str, style: Style, width: usize) -> Vec<String> {
    let mut section = vec![format!(
        "{}  {}",
        header(style, &glass.name),
        style.dim(&format!(
            "{} \u{00B7} {} \u{00B7} {}",
            glass.kind.title(),
            glass.host,
            glass.summary()
        ))
    )];
    if glass.failed {
        section.extend(note(
            &format!("{} didn't answer.", glass.host),
            style,
            width,
        ));
    } else if glass.routes.is_empty() {
        section.extend(note(&no_route_line(glass, prefix), style, width));
    } else {
        for path in glass.paths() {
            let state = match glass_path_state(glass, &path) {
                Some("Filtered") => format!("  {}", style.yellow("Filtered")),
                Some(word) => format!("  {}", style.dim(word)),
                None => String::new(),
            };
            section.push(format!("  {}{state}", chain(&path.asns, style)));
            section.push(format!("    {}", style.dim(&glass_path_line(glass, &path))));
        }
        if let Some(observed) = glass.observed_at {
            section.extend(note(
                &format!("Data from {}.", date_time(observed)),
                style,
                width,
            ));
        }
    }
    section
}

fn neighbors_part(bgp: &BgpResult, run: &Run, style: Style, width: usize) -> Vec<Vec<String>> {
    let Some(network) = bgp
        .network
        .as_ref()
        .filter(|network| !network.neighbors.is_empty())
    else {
        let line = if run.is_running() {
            "Getting neighbors\u{2026}"
        } else {
            "Collectors report no network next to this one in an AS path."
        };
        return vec![vec![
            header(style, "Neighbors"),
            format!("  {}", style.dim(line)),
        ]];
    };
    let own = run.own_asn();
    Side::ALL
        .into_iter()
        .filter_map(|side| {
            let shown = network.side(side);
            if shown.is_empty() {
                return None;
            }
            let total = network.side_count(side).unwrap_or(shown.len() as u64);
            let top = shown[0].power.max(1);
            let mut section = vec![format!(
                "{} {}",
                header(style, side.title()),
                style.dim(&total.to_string())
            )];
            for neighbor in &shown {
                section.push(share_row(
                    bgp,
                    neighbor.asn,
                    neighbor.power as f64 / top as f64,
                    None,
                    own,
                    style,
                    width,
                ));
            }
            section.extend(note(&network.side_note(side, shown.len()), style, width));
            Some(section)
        })
        .collect()
}

fn prefixes_part(bgp: &BgpResult, run: &Run, style: Style, width: usize) -> Vec<Vec<String>> {
    let Some(network) = bgp
        .network
        .as_ref()
        .filter(|network| !network.prefixes.is_empty())
    else {
        if bgp.unrouted_reason.is_some() {
            return Vec::new();
        }
        let line = if run.is_running() {
            "Getting prefixes\u{2026}"
        } else {
            "Collectors see no prefix originated by this network."
        };
        return vec![vec![
            header(style, "Prefixes"),
            format!("  {}", style.dim(line)),
        ]];
    };
    let widest = network.prefixes.iter().map(String::len).max().unwrap_or(0);
    let mut section = vec![header(style, &format!("Originated by AS{}", network.asn))];
    for prefix in &network.prefixes {
        let rpki = network
            .validation_for(prefix)
            .map_or(String::new(), |validation| badge(style, validation.status));
        let this = if Some(prefix) == bgp.prefix.as_ref() {
            format!("  {}", style.dim("This route"))
        } else {
            String::new()
        };
        section.push(
            format!("  {}  {}{this}", padded(prefix, widest), padded(&rpki, 9))
                .trim_end()
                .to_string(),
        );
    }
    let footer = prefix_footer(network, run.is_running());
    if !footer.is_empty() {
        section.extend(note(&footer, style, width));
    }
    vec![section]
}

/// How much of the network's list is shown and how much of it has an RPKI result.
pub fn prefix_footer(network: &BgpNetwork, running: bool) -> String {
    let mut parts = Vec::new();
    if let Some(total) = network.prefix_total() {
        if total > network.prefixes.len() as u64 {
            parts.push(format!(
                "The first {} of {} in address order.",
                network.prefixes.len(),
                count(total)
            ));
        }
    }
    let checked = network.validations.len();
    if checked < network.prefixes.len() {
        parts.push(if running && checked < BgpNetwork::VALIDATION_LIMIT {
            "Checking RPKI\u{2026}".to_string()
        } else {
            format!("RPKI is checked for {checked} of them.")
        });
    }
    parts.join(" ")
}

fn chart_cells(width: usize) -> usize {
    width.saturating_sub(4).clamp(20, 72)
}

fn history_part(
    bgp: &BgpResult,
    run: &Run,
    days: u64,
    now: u64,
    style: Style,
    width: usize,
) -> Vec<Vec<String>> {
    let since = now.saturating_sub(days * 86_400);
    let cells = chart_cells(width);
    let lanes = bgp.lanes(since);
    let mut sections = Vec::new();
    let axis = || {
        let (from, to) = (date(since), date(now));
        format!(
            "  {}",
            style.dim(&format!(
                "{from}{}{to}",
                " ".repeat(cells.saturating_sub(from.len() + to.len()))
            ))
        )
    };

    if lanes.is_empty() {
        let line = if run.is_running() {
            "Getting the routing history\u{2026}".to_string()
        } else {
            format!("No origin AS was seen for this prefix in the last {days} days by at least ten full-table peers.")
        };
        let mut section = vec![header(style, &format!("Announced by, {days} days"))];
        section.extend(note(&line, style, width));
        sections.push(section);
    } else {
        let mut section = vec![header(style, &format!("Announced by, {days} days"))];
        for (index, (origin, stretches)) in lanes.iter().enumerate() {
            let label = match bgp.name_of(*origin) {
                Some(name) => format!("AS{origin} \u{00B7} {name}"),
                None => format!("AS{origin}"),
            };
            section.push(format!("  {}", style.dim(&truncate_end(&label, cells))));
            let marks = lane(stretches, since, now, cells);
            section.push(format!(
                "  {}",
                if index == 0 {
                    style.green(&marks)
                } else {
                    style.yellow(&marks)
                }
            ));
        }
        section.push(axis());
        sections.push(section);

        let peers = bgp.peers_over(since, now, cells);
        let top = peers.iter().copied().fold(0.0, f64::max);
        sections.push(vec![
            format!(
                "{}  {}",
                header(style, "Peers seeing it"),
                style.dim(&format!("up to {}", top as u64))
            ),
            format!("  {}", style.blue(&spark(&peers))),
            axis(),
        ]);
    }

    let changes = bgp.changes(days, now);
    let mut events = vec![header(style, "Events")];
    if changes.is_empty() {
        events.push("  No changes".to_string());
        events.extend(note(
            &format!("Same origin AS and steady visibility over the last {days} days."),
            style,
            width,
        ));
    } else {
        for change in &changes {
            let mark = if change.is_good() {
                style.green("+")
            } else {
                style.yellow("\u{2212}")
            };
            events.push(format!(
                "  {mark} {}  {}",
                change.title(),
                style.dim(&date_time(change.date))
            ));
            events.extend(
                pretty::wrap(&bgp.change_line(change), width.saturating_sub(4))
                    .into_iter()
                    .map(|line| format!("    {}", style.dim(&line))),
            );
        }
    }
    events.extend(note(
        "Read from 8-hour collector snapshots, so times are approximate.",
        style,
        width,
    ));
    sections.push(events);

    if !bgp.activity.is_empty() {
        let series = |pick: fn(&result::Activity) -> u64| -> Vec<f64> {
            bgp.activity
                .iter()
                .map(|bucket| pick(bucket) as f64)
                .collect()
        };
        let announcements = series(|bucket| bucket.announcements);
        let withdrawals = series(|bucket| bucket.withdrawals);
        // Both rows share one scale, so a withdrawal reads against the announcements.
        let top = announcements
            .iter()
            .chain(&withdrawals)
            .copied()
            .fold(0.0, f64::max);
        let scaled = |values: &[f64]| {
            let mut with_top = values.to_vec();
            with_top.push(top);
            let mut drawn = spark(&with_top);
            drawn.pop();
            drawn
        };
        sections.push(vec![
            format!(
                "{}  {}",
                header(style, "BGP updates, 7 days"),
                style.dim(&format!("{} total", count(bgp.updates())))
            ),
            format!(
                "  {}  {}",
                style.dim("Announcements"),
                style.blue(&scaled(&announcements))
            ),
            format!(
                "  {}    {}",
                style.dim("Withdrawals"),
                style.red(&scaled(&withdrawals))
            ),
        ]);
    }
    sections
}

/// "RIPE RIS and RPKI · observed 3h ago · 1 unavailable".
pub fn sources_line(bgp: &BgpResult, style: Style, now: u64) -> String {
    let observed = bgp
        .sources
        .iter()
        .filter_map(|source| source.observed_at)
        .max();
    let failed = bgp.sources.iter().filter(|source| source.failed).count();
    let mut parts = vec!["RIPE RIS and RPKI".to_string()];
    if let Some(observed) = observed {
        parts.push(format!(
            "observed {}",
            format::relative(observed, now).to_lowercase()
        ));
    }
    let text = parts.join(" \u{00B7} ");
    if failed > 0 {
        format!(
            "{}{}",
            style.dim(&format!("{text} \u{00B7} ")),
            style.yellow(&format!("{failed} unavailable"))
        )
    } else {
        style.dim(&text)
    }
}

fn sources_part(bgp: &BgpResult, style: Style, width: usize) -> Vec<String> {
    if bgp.sources.is_empty() {
        return Vec::new();
    }
    let mut sources: Vec<&result::Source> = bgp.sources.iter().collect();
    sources.sort_by(|a, b| a.title.cmp(&b.title));
    let mut section = vec![header(style, "Sources")];
    for source in sources {
        let failed = if source.failed {
            format!("  {}", style.yellow("Unavailable"))
        } else {
            String::new()
        };
        section.push(format!("  {}{failed}", source.title));
        section.push(format!("    {}", style.dim(&source.basis)));
        section.push(format!(
            "    {}",
            style.dim(&truncate_end(&source.line(), width.saturating_sub(4)))
        ));
    }
    section.extend(note(
        "Each part was observed at its own time, so together they are not one snapshot.",
        style,
        width,
    ));
    section
}

/// Origin validation of one route, spelled out: the result, the three checks and the ROAs that
/// cover it.
pub fn validation_lines(
    validation: &Validation,
    style: Style,
    width: usize,
    now: u64,
) -> Vec<String> {
    let mut out = vec![format!(
        "{}  {}",
        header(style, &format!("RPKI for AS{}", validation.origin)),
        graded_plain(style, validation.status.grade(), validation.status.title())
    )];
    out.extend(note(&validation.explanation(), style, width));
    let checks = validation.checks();
    let widest = checks
        .iter()
        .map(|check| check.title.len())
        .max()
        .unwrap_or(0);
    for check in &checks {
        let mark = if check.passed {
            style.green("\u{2713}")
        } else {
            style.red("\u{00D7}")
        };
        out.extend(pair(
            format!("  {mark} {}", padded(check.title, widest)),
            style.dim(&check.detail),
            width,
        ));
    }
    if !validation.roas.is_empty() {
        out.push(format!("  {}", style.dim("Covering ROAs")));
        let widest = validation
            .roas
            .iter()
            .map(|roa| roa.prefix.len())
            .max()
            .unwrap_or(0);
        for roa in &validation.roas {
            out.extend(pair(
                format!("    {}", padded(&roa.prefix, widest)),
                format!(
                    "{}  {}",
                    style.dim(&format!(
                        "AS{} \u{00B7} max /{}",
                        roa.origin, roa.max_length
                    )),
                    badge(style, RpkiStatus::from_provider(&roa.validity))
                ),
                width,
            ));
        }
        out.extend(note(
            &format!(
                "Validated by Routinator through RIPEstat {}.",
                format::relative(validation.checked_at, now).to_lowercase()
            ),
            style,
            width,
        ));
    }
    out
}

/// A saved lookup with every part, as `rvrs show` prints it.
pub fn saved_lines(run: &Run, style: Style, width: usize) -> Vec<String> {
    result_lines(&View {
        run,
        trace: None,
        parts: Parts {
            all: true,
            ..Parts::default()
        },
        days: 7,
        style,
        width,
        frame: 0,
        // A saved result is read against the time it was made, not against today.
        now: run.started_at,
    })
}

/// `rvrs bgp check`: would this prefix, announced by this AS, pass RPKI origin validation?
pub fn check(prefix: &str, asn: u32, json: bool, color: ColorChoice) -> ExitCode {
    let validation = rvrs_core::bgp::lookup::check(prefix, asn);
    if json {
        let roas: Vec<serde_json::Value> = validation
            .roas
            .iter()
            .map(|roa| {
                serde_json::json!({
                    "prefix": roa.prefix,
                    "origin": roa.origin,
                    "maxLength": roa.max_length,
                    "validity": roa.validity,
                })
            })
            .collect();
        let report = serde_json::json!({
            "prefix": validation.prefix,
            "origin": validation.origin,
            "status": validation.status.badge(),
            "title": validation.status.title(),
            "sentence": validation.explanation(),
            "roas": roas,
            "checkedAt": format::iso8601(validation.checked_at),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_default()
        );
    } else {
        let style = Style::detect(color);
        let (width, _) = term::size();
        let width = width.clamp(40, 120);
        println!();
        println!(
            "{} {} {}",
            style.bold("//"),
            style.dim("bgp check"),
            style.bold(&format!("{prefix} AS{asn}"))
        );
        println!();
        for line in validation_lines(&validation, style, width, format::now()) {
            println!("{line}");
        }
        if validation.status != RpkiStatus::Unavailable {
            for line in note(
                "A validation result says nothing about whether the route is announced.",
                style,
                width,
            ) {
                println!("{line}");
            }
        }
    }
    match validation.status {
        RpkiStatus::Valid => ExitCode::SUCCESS,
        RpkiStatus::Unavailable => ExitCode::from(EXIT_USAGE),
        _ => ExitCode::from(1),
    }
}

/// `rvrs bgp glass`: the user's looking glasses.
pub fn glass(command: GlassCommand, color: ColorChoice) -> ExitCode {
    let style = Style::detect(color);
    let mut glasses = Glasses::open();
    match command {
        GlassCommand::List => {
            for line in glass_list(&glasses.list, style) {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        GlassCommand::Add {
            host,
            kind,
            name,
            routers,
        } => {
            let narrate = std::io::stderr().is_terminal();
            let kinds: Vec<GlassKind> = kind.map_or(GlassKind::ALL.to_vec(), |kind| vec![kind]);
            let mut found = None;
            let mut last_error = String::new();
            for kind in kinds {
                if narrate {
                    eprintln!(
                        "{}",
                        Style::detect_stderr()
                            .dim(&format!("Checking {host} as {}\u{2026}", kind.title()))
                    );
                }
                match glass::describe(kind, &host) {
                    Ok(setup) => {
                        found = Some(setup);
                        break;
                    }
                    Err(message) => last_error = message,
                }
            }
            let Some(setup) = found else {
                return crate::fail(&if kind.is_some() {
                    last_error
                } else {
                    format!("{host} didn't answer as a looking glass rvrs can read: Alice-LG, lg.cgi, hyperglass or bird-lg-go.")
                });
            };
            let chosen = match choose_routers(&setup.routers, &routers, setup.kind, &host) {
                Ok(chosen) => chosen,
                Err(message) => return crate::usage(&message),
            };
            let name = name
                .or_else(|| setup.name.clone())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| host.clone());
            let glass = LookingGlass::from_setup(&setup, &name, &host, chosen);
            let asked = if glass.kind.has_routers() {
                format!(
                    ", {} {}",
                    glass.routers.len(),
                    if glass.routers.len() == 1 {
                        "router"
                    } else {
                        "routers"
                    }
                )
            } else {
                String::new()
            };
            let line = format!("{name} ({host}, {}{asked})", glass.kind.title());
            match glasses.upsert(glass) {
                Ok(true) => println!("Updated {line}."),
                Ok(false) => println!("Added {line}. Every rvrs bgp lookup of a route asks it."),
                Err(error) => {
                    return crate::fail(&format!("Couldn't save the looking glass ({error})."))
                }
            }
            ExitCode::SUCCESS
        }
        GlassCommand::Remove(host) => match glasses.remove(&host) {
            Ok(true) => {
                println!("Removed {host}.");
                ExitCode::SUCCESS
            }
            Ok(false) => crate::fail(&no_glass(&host)),
            Err(error) => crate::fail(&format!("Couldn't save the looking glasses ({error}).")),
        },
        GlassCommand::Test {
            host,
            prefix,
            router,
            raw,
        } => {
            let Some(glass) = glasses.find(&host) else {
                return crate::fail(&no_glass(&host));
            };
            let router = router.or_else(|| glass.routers.first().cloned());
            let result = glass::test(
                glass,
                router.as_deref(),
                prefix.as_deref().unwrap_or(TEST_PREFIX),
            );
            println!(
                "{}",
                if result.passed {
                    style.bold_green(&result.headline)
                } else {
                    style.red(&result.headline)
                }
            );
            let widest = result
                .steps
                .iter()
                .map(|step| step.title.len())
                .max()
                .unwrap_or(0);
            for step in &result.steps {
                let mark = if step.passed {
                    style.green("\u{2713}")
                } else {
                    style.red("\u{00D7}")
                };
                println!(
                    "  {mark} {}  {}",
                    padded(&step.title, widest),
                    style.dim(&step.detail)
                );
            }
            if raw {
                if let Some(text) = &result.raw {
                    println!();
                    for line in text.lines() {
                        println!("{}", style.dim(line));
                    }
                }
            }
            if result.passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
    }
}

fn no_glass(host: &str) -> String {
    format!("No looking glass has the host \u{201C}{host}\u{201D}. List them with rvrs bgp glass.")
}

/// The routers a new looking glass asks. A few are all taken. Past that the choice is the
/// user's, since every router is one more request on every lookup.
fn choose_routers(
    offered: &[String],
    wanted: &[String],
    kind: GlassKind,
    host: &str,
) -> Result<Vec<String>, String> {
    if !kind.has_routers() {
        return Ok(Vec::new());
    }
    if wanted.is_empty() {
        if offered.len() <= 4 {
            return Ok(offered.to_vec());
        }
        return Err(format!(
            "{host} has {} routers. Name the ones to ask with --router, separated by commas:\n  {}",
            offered.len(),
            offered.join("\n  ")
        ));
    }
    wanted
        .iter()
        .map(|router| {
            offered
                .iter()
                .find(|known| known.eq_ignore_ascii_case(router))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "{host} has no router \u{201C}{router}\u{201D}. Its routers:\n  {}",
                        offered.join("\n  ")
                    )
                })
        })
        .collect()
}

pub fn glass_list(glasses: &[LookingGlass], style: Style) -> Vec<String> {
    let mut out = vec![
        String::new(),
        format!("{} {}", style.bold("//"), style.bold("looking glasses")),
        String::new(),
    ];
    if glasses.is_empty() {
        out.push("No looking glasses".to_string());
        out.push(style.dim("Add one with rvrs bgp glass add <host>."));
        return out;
    }
    let name_width = glasses
        .iter()
        .map(|glass| visible_width(&glass.name))
        .max()
        .unwrap_or(0);
    let host_width = glasses
        .iter()
        .map(|glass| glass.host.len())
        .max()
        .unwrap_or(0);
    for glass in glasses {
        let mut kind = glass.kind.title().to_string();
        if glass.kind.has_routers() {
            kind.push_str(&format!(
                " \u{00B7} {} {}",
                glass.routers.len(),
                if glass.routers.len() == 1 {
                    "router"
                } else {
                    "routers"
                }
            ));
        }
        out.push(format!(
            "  {}  {}  {}",
            padded(&glass.name, name_width),
            padded(&glass.host, host_width),
            style.dim(&kind)
        ));
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rvrs_core::bgp::result::fixtures::routed;
    use rvrs_core::model::Tool;
    use rvrs_core::FamilyChoice;

    fn line(words: &str) -> Result<Parsed, String> {
        let arguments: Vec<String> = words.split_whitespace().map(str::to_string).collect();
        parse(&arguments)
    }

    /// A finished lookup of example.com, as history would hold it.
    pub(crate) fn finished() -> Run {
        let mut run = Run::new(Tool::Bgp, "example.com", FamilyChoice::Auto, 0, 0);
        run.started_at = 1_800_000_000;
        run.bgp = Some(routed());
        run.outcome = Some(Outcome::Reached);
        run
    }

    fn plain(run: &Run, parts: Parts, width: usize) -> Vec<String> {
        result_lines(&View {
            run,
            trace: None,
            parts,
            days: 7,
            style: Style::PLAIN,
            width,
            frame: 0,
            now: 1_800_000_000,
        })
    }

    #[test]
    fn a_target_alone_is_a_lookup_and_flags_pick_its_parts() {
        let Ok(Parsed::Run(args)) = line("AS64500") else {
            panic!("not a run");
        };
        assert_eq!(args.target, "AS64500");
        assert!(args.parts.overview());

        let Ok(Parsed::Run(args)) = line("example.com --paths --days 30 --no-glasses --json")
        else {
            panic!("not a run");
        };
        assert!(args.parts.paths && args.parts.history && !args.parts.overview());
        assert_eq!(args.days, 30);
        assert!(args.no_glasses && args.common.json);

        assert_eq!(line("--help"), Ok(Parsed::Help));
        assert!(line("").unwrap_err().contains("rvrs bgp AS64500"));
        assert!(line("a.example b.example")
            .unwrap_err()
            .contains("b.example"));
        assert_eq!(
            line("example.com --days 8").unwrap_err(),
            "--days takes 7, 30 or 90."
        );
        assert!(line("example.com -x")
            .unwrap_err()
            .contains("doesn't take -x"));
    }

    #[test]
    fn check_takes_a_prefix_and_an_as_number() {
        assert_eq!(
            line("check 192.0.2.0/24 as64500"),
            Ok(Parsed::Check {
                prefix: "192.0.2.0/24".to_string(),
                asn: 64500,
                json: false,
                color: ColorChoice::Auto
            })
        );
        assert!(line("check 192.0.2.1 64500")
            .unwrap_err()
            .contains("203.0.113.0/24"));
        assert!(line("check 192.0.2.0/24").is_err());
    }

    #[test]
    fn looking_glasses_are_added_removed_tested_and_listed() {
        assert_eq!(
            line("glass"),
            Ok(Parsed::Glass(GlassCommand::List, ColorChoice::Auto))
        );
        assert_eq!(
            line("glass add https://LG.example.net/ --type hyperglass --router a,b --router c"),
            Ok(Parsed::Glass(
                GlassCommand::Add {
                    host: "lg.example.net".to_string(),
                    kind: Some(GlassKind::Hyperglass),
                    name: None,
                    routers: vec!["a".to_string(), "b".to_string(), "c".to_string()],
                },
                ColorChoice::Auto
            ))
        );
        assert_eq!(
            line("glass test lg.example.net 192.0.2.0/24 --raw"),
            Ok(Parsed::Glass(
                GlassCommand::Test {
                    host: "lg.example.net".to_string(),
                    prefix: Some("192.0.2.0/24".to_string()),
                    router: None,
                    raw: true,
                },
                ColorChoice::Auto
            ))
        );
        assert_eq!(
            line("glass rm lg.example.net"),
            Ok(Parsed::Glass(
                GlassCommand::Remove("lg.example.net".to_string()),
                ColorChoice::Auto
            ))
        );
        assert!(line("glass add").is_err());
        assert!(line("glass add lg.example.net --type other").is_err());
        assert!(line("glass test lg.example.net nonsense").is_err());

        let empty = glass_list(&[], Style::PLAIN);
        assert_eq!(empty[3], "No looking glasses");
    }

    #[test]
    fn routers_are_all_taken_when_few_and_chosen_when_many() {
        let few: Vec<String> = ["a", "b"].iter().map(|name| name.to_string()).collect();
        let many: Vec<String> = (0..6).map(|index| format!("r{index}")).collect();
        let host = "lg.example.net";
        assert_eq!(
            choose_routers(&few, &[], GlassKind::LgCgi, host),
            Ok(few.clone())
        );
        assert_eq!(
            choose_routers(&few, &[], GlassKind::AliceLg, host),
            Ok(Vec::new())
        );
        let asked = choose_routers(&many, &[], GlassKind::LgCgi, host).unwrap_err();
        assert!(asked.starts_with("lg.example.net has 6 routers."));
        assert_eq!(
            choose_routers(&many, &["R3".to_string()], GlassKind::LgCgi, host),
            Ok(vec!["r3".to_string()])
        );
        assert!(
            choose_routers(&many, &["x".to_string()], GlassKind::LgCgi, host)
                .unwrap_err()
                .contains("has no router \u{201C}x\u{201D}")
        );
    }

    #[test]
    fn the_overview_leads_with_the_verdict_and_the_route() {
        let run = finished();
        let lines = plain(&run, Parts::default(), 100);
        assert_eq!(lines[1], "// bgp example.com  route collectors");
        assert_eq!(lines[3], "Routed and authorized");
        let text = lines.join("\n");
        assert!(text.contains("Seen by 318/320 peers \u{00B7} Paths 3 \u{00B7} First seen 2017"));
        assert!(text.contains("  Announced prefix  192.0.2.0/24"));
        assert!(text.contains("  Example Net       AS64500"));
        assert!(text.contains("  RPKI              Valid"));
        assert!(text.contains("  Route object      RADB"));
        assert!(text.contains("Reached through\n  AS64501       Transit One"));
        assert!(text.contains("68.6%"));
        assert!(text.contains("Your path\n  rvrs traceroute example.com"));
        assert!(text.contains("Network\n  About AS64500"));
        assert!(text.contains("Related prefixes\n  192.0.2.0/23  Covering \u{00B7} AS64500"));
        assert!(text.contains("Sources\n  RIPE RIS and RPKI \u{00B7} observed 2h ago"));
        assert!(!text.contains("Observed AS paths"));
    }

    #[test]
    fn paths_prefixes_rpki_and_sources_print_when_asked_for() {
        let run = finished();
        let text = plain(
            &run,
            Parts {
                paths: true,
                prefixes: true,
                rpki: true,
                sources: true,
                ..Parts::default()
            },
            100,
        )
        .join("\n");
        assert!(!text.contains("Reached through"));
        assert!(text.contains(
            "Shortest 2 networks \u{00B7} Typical 3 networks \u{00B7} Direct 5.7% of peers"
        ));
        assert!(text
            .contains("   62.9%  AS64510 \u{203A} AS64501 \u{203A} AS64500\n          200 peers"));
        assert!(text.contains("from Transit One \u{00B7} 18 peers"));
        assert!(text.contains("100 peers \u{00B7} prepending"));
        assert!(text.contains("3 distinct paths from 318 peers at 2 collectors."));
        assert!(text.contains(
            "Originated by AS64500\n  192.0.2.0/24   Valid      This route\n  2001:db8::/32"
        ));
        assert!(text.contains("RPKI for AS64500  Origin authorized"));
        assert!(text.contains("\u{2713} Prefix length allowed  /24 is within the maximum of /24."));
        assert!(text.contains("    192.0.2.0/24  AS64500 \u{00B7} max /24  Valid"));
        assert!(text.contains("Sources\n  Origin AS and holder\n    RIPE RIS route collectors\n    RIPEstat prefix-overview \u{00B7} observed 2027-01-15 05:13 UTC"));
    }

    #[test]
    fn history_draws_a_lane_per_origin_and_lists_what_changed() {
        let day = 86_400;
        let now = 1_800_000_000;
        let mut run = finished();
        let bgp = run.bgp.as_mut().unwrap();
        let span = |origin, start: u64, end: u64, peers| result::Span {
            origin,
            prefix: "192.0.2.0/24".to_string(),
            start: now - start * day,
            end: now - end * day,
            peers,
        };
        bgp.spans = vec![span(64500, 60, 0, 300.0), span(64496, 3, 2, 40.0)];
        bgp.activity = vec![
            result::Activity {
                start: now - day,
                announcements: 8,
                withdrawals: 0,
            },
            result::Activity {
                start: now,
                announcements: 4,
                withdrawals: 8,
            },
        ];
        let lines = plain(
            &run,
            Parts {
                history: true,
                ..Parts::default()
            },
            64,
        );
        let text = lines.join("\n");
        assert!(text.contains("Announced by, 7 days\n  AS64500 \u{00B7} Example Net\n"));
        let full = lines
            .iter()
            .position(|line| line.contains("AS64500 \u{00B7}"))
            .unwrap()
            + 1;
        assert_eq!(lines[full].trim().chars().count(), 60);
        let partial = lines
            .iter()
            .position(|line| line.trim() == "AS64496")
            .unwrap()
            + 1;
        let marks = lines[partial].trim().chars().count();
        assert!((8..=10).contains(&marks), "{marks} cells");
        assert!(text.contains("Peers seeing it  up to 340"));
        assert!(text.contains("+ New origin AS observed  2027-01-12 08:00 UTC"));
        assert!(text.contains("\u{2212} Origin AS no longer seen  2027-01-13 08:00 UTC"));
        assert!(text.contains("BGP updates, 7 days  20 total"));
        assert!(text.contains("Announcements  \u{2588}\u{2585}"));
        assert!(text.contains("Withdrawals     \u{2588}"));
        for line in &lines {
            assert!(visible_width(line) <= 64, "{line}");
        }
    }

    #[test]
    fn a_network_shows_its_registration_neighbors_and_prefixes() {
        use rvrs_core::bgp::{BgpEvent, Neighbor, NetworkPart};
        let mut run = Run::new(Tool::Bgp, "AS64500", FamilyChoice::Auto, 0, 0);
        let mut bgp = BgpResult::new(TargetKind::Asn, "AS64500");
        let events = [
            BgpEvent::Network(64500),
            BgpEvent::NetworkPart(NetworkPart::Identity {
                holder: Some("Example Net".to_string()),
                announced: Some(true),
                block: None,
            }),
            BgpEvent::NetworkPart(NetworkPart::Registry("RIPE NCC".to_string())),
            BgpEvent::NetworkPart(NetworkPart::Country("DE".to_string())),
            BgpEvent::NetworkPart(NetworkPart::Neighbors {
                list: vec![
                    Neighbor {
                        asn: 64501,
                        side: Side::Left,
                        power: 200,
                    },
                    Neighbor {
                        asn: 64502,
                        side: Side::Left,
                        power: 100,
                    },
                    Neighbor {
                        asn: 64510,
                        side: Side::Right,
                        power: 7,
                    },
                ],
                upstream: Some(12),
                downstream: Some(1),
                uncertain: Some(0),
            }),
            BgpEvent::NetworkPart(NetworkPart::Prefixes {
                list: vec!["192.0.2.0/24".to_string()],
                v4: Some(1),
                v6: Some(0),
            }),
            BgpEvent::Finished,
        ];
        for event in &events {
            bgp.apply(event);
        }
        run.bgp = Some(bgp);
        run.outcome = Some(Outcome::Reached);
        let text = plain(
            &run,
            Parts {
                all: true,
                ..Parts::default()
            },
            120,
        )
        .join("\n");
        assert!(text.contains("Originates 1 prefix\n"));
        assert!(text.contains("IPv4 1 prefix \u{00B7} IPv6 0 prefixes \u{00B7} ROA \u{2014}"));
        assert!(text.contains("  Holder             Example Net"));
        assert!(text.contains("  In public routing  Seen"));
        assert!(text.contains("  Whois              https://stat.ripe.net/AS64500"));
        assert!(text.contains("Seen upstream\n  AS64501"));
        assert!(text.contains("Upstream 12\n  AS64501"));
        assert!(text.contains("The 2 seen most often, of 12. Seen before AS64500 in AS paths, which suggests an upstream."));
        assert!(text.contains("Downstream 1\n  AS64510"));
        assert!(text.contains("Originated by AS64500\n  192.0.2.0/24"));
        assert!(text.contains("RPKI is checked for 0 of them."));
        assert!(!text.contains("Observed AS paths"));
    }

    #[test]
    fn a_looking_glass_shows_its_paths_or_why_it_has_none() {
        let mut run = finished();
        let held: GlassResult = serde_json::from_value(serde_json::json!({
            "host": "lg.example.net",
            "name": "Example IX",
            "kind": "alice-lg",
            "routes": [
                { "route_server": "rs1", "neighbor": 64501, "neighbor_name": "Transit One",
                  "asns": [64501, 64500], "is_best": true, "is_filtered": false,
                  "communities": [], "rpki": null, "network": null },
                { "route_server": "rs2", "neighbor": 64501, "neighbor_name": "Transit One",
                  "asns": [64501, 64500], "is_best": false, "is_filtered": false,
                  "communities": [], "rpki": null, "network": null }
            ]
        }))
        .unwrap();
        let silent: GlassResult = serde_json::from_value(serde_json::json!({
            "host": "lg.example.org", "name": "Example Net", "kind": "lg-cgi"
        }))
        .unwrap();
        run.bgp.as_mut().unwrap().glasses = vec![held, silent];
        let text = plain(
            &run,
            Parts {
                paths: true,
                ..Parts::default()
            },
            100,
        )
        .join("\n");
        assert!(text.contains("Example IX  Alice-LG \u{00B7} lg.example.net \u{00B7} 2 routes \u{00B7} 2 route servers"));
        assert!(text.contains(
            "  AS64501 \u{203A} AS64500  Best\n    from Transit One \u{00B7} 2 route servers"
        ));
        assert!(text.contains("Example Net  lg.cgi \u{00B7} lg.example.org \u{00B7} No route\n  Its routers hold no BGP route for 192.0.2.0/24."));
        let overview = plain(&run, Parts::default(), 100).join("\n");
        assert!(overview.contains("  Example IX        2 routes \u{00B7} 2 route servers"));
    }

    #[test]
    fn a_running_lookup_says_how_many_sources_have_answered() {
        let mut run = finished();
        run.outcome = None;
        run.bgp.as_mut().unwrap().pending = 9;
        let lines = plain(&run, Parts::default(), 80);
        assert!(lines[3].ends_with("1 of 10 sources"));
        let mut empty = Run::new(Tool::Bgp, "example.com", FamilyChoice::Auto, 0, 0);
        empty.bgp = Some(BgpResult::new(TargetKind::Host, "example.com"));
        assert!(plain(&empty, Parts::default(), 80)[3].ends_with("Finding the route"));
        empty.outcome = Some(Outcome::Stopped);
        assert_eq!(plain(&empty, Parts::default(), 80)[3], "Stopped");
    }

    #[test]
    fn nothing_overflows_a_narrow_terminal() {
        let run = finished();
        for line in plain(
            &run,
            Parts {
                all: true,
                ..Parts::default()
            },
            40,
        ) {
            assert!(visible_width(&line) <= 40, "{line}");
        }
    }

    #[test]
    fn bars_and_sparklines_scale_to_their_largest_value() {
        assert_eq!(spark(&[0.0, 1.0, 4.0, 8.0]), " \u{2582}\u{2585}\u{2588}");
        assert_eq!(spark(&[]), "");
        assert_eq!(share_bar(1.0, 4), "\u{2588}\u{2588}\u{2588}\u{2588}");
        assert_eq!(lane(&[(0, 50)], 0, 100, 4), "\u{2501}\u{2501}  ");
    }
}
