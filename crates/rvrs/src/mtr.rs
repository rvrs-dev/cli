//! `rvrs mtr`: mtr's flags, its report in a pipe, and on a terminal a table that updates in
//! place, hops grouped under their network, until Ctrl-C or the last round. It prints and
//! returns; it never opens a full-screen view. The Console does that.

use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use rvrs_core::engine::MtrOptions;
use rvrs_core::format;
use rvrs_core::history::History;
use rvrs_core::model::{groups, Grade, Hop, Outcome, Run, Slot, Verdict};
use rvrs_core::report::Report;
use rvrs_core::session::{Lookups, Session};
use rvrs_core::FamilyChoice;

use crate::args::{normalize_target, Common};
use crate::pretty::{self, View};
use crate::style::{
    truncate_end, truncate_middle, visible_width, width, ColorChoice, Style, SPINNER,
};
use crate::term::{self, LiveRegion};
use crate::trace::{self, Look};
use crate::{fail, EXIT_USAGE};

/// Rounds a report sends when `-c` doesn't say, as mtr's own report does.
pub const REPORT_ROUNDS: u32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct MtrArgs {
    pub target: String,
    pub options: MtrOptions,
    pub family: FamilyChoice,
    /// `-r`: print the report when the rounds are done.
    pub report: bool,
    /// `-w`: a report whose host column fits the longest name.
    pub wide: bool,
    /// `-n`: addresses only, no reverse DNS.
    pub numeric: bool,
    /// `-b`: names and addresses.
    pub show_ips: bool,
    /// `-z`: each hop's AS number in the report.
    pub as_lookups: bool,
    pub no_lookups: bool,
    pub common: Common,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Run(Box<MtrArgs>),
    Help,
    Version,
}

/// mtr's short flags that take a value, supported or not, so the parser reads past them right.
const VALUED: &str = "cimfsZUGBQPLoyaIF";

fn long_to_short(name: &str) -> Option<char> {
    Some(match name {
        "report-cycles" => 'c',
        "report" => 'r',
        "report-wide" => 'w',
        "no-dns" => 'n',
        "show-ips" => 'b',
        "interval" => 'i',
        "max-ttl" => 'm',
        "first-ttl" => 'f',
        "psize" => 's',
        "aslookup" => 'z',
        "timeout" => 'Z',
        "max-unknown" => 'U',
        "gracetime" => 'G',
        "udp" => 'u',
        "tcp" => 'T',
        "sctp" => 'S',
        "port" => 'P',
        "curses" => 't',
        "gtk" => 'g',
        "csv" => 'C',
        "xml" => 'x',
        "raw" => 'l',
        "split" => 'p',
        "order" => 'o',
        "ipinfo" => 'y',
        "address" => 'a',
        "interface" => 'I',
        "filename" => 'F',
        "tos" => 'Q',
        "mpls" => 'e',
        "bitpattern" => 'B',
        "localport" => 'L',
        "displaymode" => 'd',
        _ => return None,
    })
}

fn number<T: std::str::FromStr>(flag: &str, value: &str, what: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} takes {what}, and \u{201C}{value}\u{201D} isn't one."))
}

pub fn parse(arguments: &[String]) -> Result<Parsed, String> {
    let mut options = MtrOptions::default();
    let mut family = FamilyChoice::Auto;
    let (mut report, mut wide, mut numeric, mut show_ips, mut as_lookups, mut no_lookups) =
        (false, false, false, false, false, false);
    let mut packet_size: Option<usize> = None;
    let mut common = Common::default();
    let mut positional: Vec<&str> = Vec::new();

    let mut index = 0;
    let mut only_positional = false;
    let value_after = |index: &mut usize, flag: &str| -> Result<String, String> {
        *index += 1;
        arguments
            .get(*index - 1)
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value."))
    };
    // One flag, by its short name, with the value it came with.
    let mut apply = |flag: char,
                     shown: &str,
                     value: &str,
                     options: &mut MtrOptions,
                     family: &mut FamilyChoice|
     -> Result<(), String> {
        match flag {
            'c' => {
                let rounds: u32 = number(shown, value, "a number of rounds")?;
                if rounds == 0 {
                    return Err(format!("{shown} takes at least 1 round."));
                }
                options.count = Some(rounds);
            }
            'i' => {
                let seconds: f64 = number(shown, value, "a number of seconds")?;
                if !(0.1..=3600.0).contains(&seconds) {
                    return Err(format!("{shown} takes 0.1 to 3600 seconds."));
                }
                options.interval = Duration::from_secs_f64(seconds);
            }
            'm' => {
                let hops: u32 = number(shown, value, "a number of hops")?;
                if !(1..=255).contains(&hops) {
                    return Err(format!("{shown} takes 1 to 255 hops."));
                }
                options.max_ttl = hops as u8;
            }
            'f' => {
                let first: u32 = number(shown, value, "a hop number")?;
                if !(1..=255).contains(&first) {
                    return Err(format!("{shown} takes a hop number from 1 to 255."));
                }
                options.first_ttl = first as u8;
            }
            's' => {
                let size: usize = number(shown, value, "a number of bytes")?;
                if !(28..=1428).contains(&size) {
                    return Err(format!("{shown} takes 28 to 1428 bytes."));
                }
                packet_size = Some(size);
            }
            'Z' => {
                let seconds: f64 = number(shown, value, "a number of seconds")?;
                if !(seconds > 0.0 && seconds <= 60.0) {
                    return Err(format!("{shown} takes more than 0 and at most 60 seconds."));
                }
                options.wait = Duration::from_secs_f64(seconds);
            }
            'U' => {
                let hops: u32 = number(shown, value, "a number of hops")?;
                if !(1..=255).contains(&hops) {
                    return Err(format!("{shown} takes 1 to 255 hops."));
                }
                options.max_unknown = hops as u8;
            }
            // rvrs ends a run when its last probe has settled, so there is no grace time to set.
            'G' => {
                let _: f64 = number(shown, value, "a number of seconds")?;
            }
            'r' => report = true,
            'w' => {
                report = true;
                wide = true;
            }
            'n' => numeric = true,
            'b' => show_ips = true,
            'z' => as_lookups = true,
            '4' => *family = FamilyChoice::V4,
            '6' => *family = FamilyChoice::V6,
            'u' | 'T' | 'S' | 'P' | 'L' => {
                return Err(format!(
                    "{shown} asks for a probe type rvrs doesn't send yet. rvrs mtr sends ICMP echo probes, as mtr does by default."
                ));
            }
            't' | 'g' | 'd' => {
                return Err(format!(
                    "{shown} asks for mtr's full-screen view, which rvrs mtr doesn't have. Run rvrs with no command for the Console."
                ));
            }
            'C' | 'x' | 'l' | 'p' => {
                return Err(format!("{shown} is an output format rvrs doesn't write yet. --json prints the run as a report."));
            }
            'a' | 'I' => {
                return Err(format!(
                    "{shown} isn't supported yet. rvrs sends from the address and interface the system routes through."
                ));
            }
            other => {
                return Err(format!(
                    "-{other} isn't supported yet. Supported flags: rvrs mtr --help"
                ))
            }
        }
        Ok(())
    };

    while index < arguments.len() {
        let argument = arguments[index].as_str();
        index += 1;
        if only_positional || argument == "-" || !argument.starts_with('-') {
            positional.push(argument);
            continue;
        }
        if argument == "--" {
            only_positional = true;
            continue;
        }
        if let Some(long) = argument.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            match name {
                "help" => return Ok(Parsed::Help),
                "version" => return Ok(Parsed::Version),
                "json" => common.json = true,
                "classic" => common.classic = true,
                "fleet" => common.fleet = true,
                "no-lookups" => no_lookups = true,
                "color" => {
                    let value = match inline {
                        Some(value) => value,
                        None => value_after(&mut index, "--color")?,
                    };
                    common.color = ColorChoice::parse(&value)
                        .ok_or_else(|| format!("--color takes auto, always or never, and \u{201C}{value}\u{201D} is none of them."))?;
                }
                "node" => {
                    common.node = Some(match inline {
                        Some(value) => value,
                        None => value_after(&mut index, "--node")?,
                    });
                }
                "case" => {
                    let value = match inline {
                        Some(value) => value,
                        None => value_after(&mut index, "--case")?,
                    };
                    common.case = Some(crate::args::case_name(&value)?);
                }
                other => {
                    let Some(short) = long_to_short(other) else {
                        return Err(format!(
                            "rvrs mtr has no flag --{other}. Supported flags: rvrs mtr --help"
                        ));
                    };
                    let shown = format!("--{other}");
                    let value = if VALUED.contains(short) {
                        match inline {
                            Some(value) => value,
                            None => value_after(&mut index, &shown)?,
                        }
                    } else {
                        String::new()
                    };
                    apply(short, &shown, &value, &mut options, &mut family)?;
                }
            }
            continue;
        }
        let cluster: Vec<char> = argument[1..].chars().collect();
        let mut position = 0;
        while position < cluster.len() {
            let flag = cluster[position];
            position += 1;
            let shown = format!("-{flag}");
            match flag {
                'h' => return Ok(Parsed::Help),
                'v' => return Ok(Parsed::Version),
                // mtr's own JSON has another shape. rvrs has one result format, the report.
                'j' => {
                    common.json = true;
                    continue;
                }
                _ => {}
            }
            let value = if VALUED.contains(flag) {
                let rest: String = cluster[position..].iter().collect();
                position = cluster.len();
                if rest.is_empty() {
                    value_after(&mut index, &shown)?
                } else {
                    rest
                }
            } else {
                String::new()
            };
            apply(flag, &shown, &value, &mut options, &mut family)?;
        }
    }

    let Some(target) = positional.first() else {
        return Err("rvrs mtr needs a target, such as rvrs mtr 1.1.1.1".to_string());
    };
    if positional.len() > 1 {
        return Err(format!(
            "rvrs mtr takes one target, and \u{201C}{}\u{201D} is a second.",
            positional[1]
        ));
    }
    if options.first_ttl > options.max_ttl {
        return Err(format!(
            "The first hop ({}) is beyond the last ({}).",
            options.first_ttl, options.max_ttl
        ));
    }
    if let Some(size) = packet_size {
        // The whole IP packet, headers included, as mtr counts it.
        let headers = if family == FamilyChoice::V6 || target.contains(':') {
            48
        } else {
            28
        };
        options.payload = size.saturating_sub(headers);
    }
    Ok(Parsed::Run(Box::new(MtrArgs {
        target: normalize_target(target),
        options,
        family,
        report,
        wide,
        numeric,
        show_ips,
        as_lookups,
        no_lookups,
        common,
    })))
}

pub fn run(mut args: MtrArgs, multicall: bool) -> ExitCode {
    if args.common.fleet {
        return fail(trace::NO_FLEET);
    }
    if let Some(node) = &args.common.node {
        return fail(&format!(
            "Nodes aren't available yet, so nothing was sent from \u{201C}{node}\u{201D}. Without --node the run comes from this device."
        ));
    }
    // A report is the original's own text, wherever it goes.
    let look = trace::look(
        args.common.json,
        args.common.classic || args.report,
        multicall,
    );
    let style = Style::detect(args.common.color);
    // On a terminal the table runs until Ctrl-C, as mtr's does. A report or a pipe has to end.
    if look != Look::Pretty && args.options.count.is_none() {
        args.options.count = Some(REPORT_ROUNDS);
    }
    let lookups = Lookups {
        names: !args.numeric && !args.no_lookups,
        networks: !args.no_lookups && (look != Look::Classic || args.as_lookups),
    };
    let report_options = ReportOptions {
        numeric: args.numeric,
        show_ips: args.show_ips,
        wide: args.wide,
        as_lookups: args.as_lookups,
    };

    term::catch_interrupts();
    let started = Instant::now();
    let mut session = Session::start_mtr(&args.target, args.family, args.options.clone(), lookups);
    let mut live = (look == Look::Pretty).then(LiveRegion::start);
    let mut start_line = false;
    let mut frame = 0usize;
    let mut stop_sent = false;

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
        let run = &session.run;
        match look {
            Look::Pretty => {
                if let Some(region) = live.as_mut() {
                    let (width, _) = term::size();
                    region.draw(&live_lines(run, style, width, frame), true);
                }
            }
            Look::Classic => {
                if !start_line && run.destination.is_some() {
                    println!("Start: {}", start_time(run));
                    let _ = std::io::stdout().flush();
                    start_line = true;
                }
            }
            Look::Json => {}
        }
        if !session.run.is_running() {
            break;
        }
    }
    let elapsed = started.elapsed();

    // Names and networks still on their way get a moment. The table is already on screen.
    let budget = if look == Look::Pretty {
        Duration::from_millis(1500)
    } else {
        Duration::from_millis(800)
    };
    let deadline = Instant::now() + budget;
    while session.pending_lookups() > 0 && Instant::now() < deadline && term::interrupts() < 2 {
        session.pump(Duration::from_millis(80));
    }
    session.finish_lookups(Duration::ZERO);
    let run = &session.run;

    if let Some(Outcome::Failed(message)) = &run.outcome {
        if let Some(mut region) = live.take() {
            region.draw(&[], false);
        }
        if look == Look::Json {
            println!("{}", Report::of(run).json());
        }
        return fail(message);
    }

    let keep = std::env::var("RVRS_HISTORY").map_or(true, |value| value != "0");
    let measured = run.path_len() > 0;
    if keep && measured {
        let mut history = History::open();
        if let Err(error) = history.save(run) {
            eprintln!("rvrs: Couldn't save this run to history ({error}).");
        }
    }

    match look {
        Look::Pretty => {
            if let Some(mut region) = live.take() {
                let (width, _) = term::size();
                region.draw(&closing_lines(run, style, width, Some(elapsed)), false);
                region.finish();
            }
        }
        Look::Classic => {
            for line in report_lines(run, report_options) {
                println!("{line}");
            }
        }
        Look::Json => println!("{}", Report::of(run).json()),
    }
    let _ = std::io::stdout().flush();
    if let Some(case) = args.common.case.as_deref().filter(|_| measured) {
        crate::commands::save_after_run(run, case);
    }
    ExitCode::SUCCESS
}

// The terminal look

const FIGURES: usize = 7;
const SPARK: usize = 20;

/// How the table is laid out at a width: which columns fit, and how wide the host is.
struct Layout {
    host: usize,
    /// Last, Best and Worst beside Avg.
    range: bool,
    /// The newest replies of each hop as a sparkline.
    spark: bool,
    /// A terminal too narrow for more than loss and the average.
    tight: bool,
}

fn layout(total: usize) -> Layout {
    let total = total.clamp(40, 180);
    let lead = 6;
    let tight = total < lead + 14 + 7 + 5 + FIGURES * 2;
    let basic = if tight {
        7 + FIGURES
    } else {
        7 + 5 + FIGURES * 2
    };
    let range = !tight && total >= lead + 24 + basic + FIGURES * 3;
    let figures = basic + if range { FIGURES * 3 } else { 0 };
    let spark = total >= lead + 36 + figures + SPARK + 2;
    let host = total
        .saturating_sub(lead + figures + if spark { SPARK + 2 } else { 0 })
        .clamp(12, 64);
    Layout {
        host,
        range,
        spark,
        tight,
    }
}

/// The column names over the figures, and the hop rows under their network headers.
pub fn table(run: &Run, style: Style, total: usize) -> Vec<String> {
    let total = total.clamp(40, 180);
    let layout = layout(total);
    let lead = 6;
    let mut header = format!(
        "{}{}{:>7}",
        " ".repeat(lead),
        " ".repeat(layout.host),
        "Loss"
    );
    if !layout.tight {
        header.push_str(&format!("{:>5}", "Snt"));
    }
    if layout.range {
        header.push_str(&format!("{:>FIGURES$}", "Last"));
    }
    header.push_str(&format!("{:>FIGURES$}", "Avg"));
    if layout.range {
        header.push_str(&format!("{:>FIGURES$}{:>FIGURES$}", "Best", "Worst"));
    }
    if !layout.tight {
        header.push_str(&format!("{:>FIGURES$}", "Jitter"));
    }
    let mut out = vec![style.dim(&header)];

    let slowest = run
        .path()
        .iter()
        .flat_map(|hop| hop.probes.iter().filter_map(Slot::rtt))
        .max()
        .unwrap_or_default();
    for group in groups(run) {
        if let Some(network) = group.network {
            let mut line = format!(
                "{} {}",
                style.bold(&network.label()),
                truncate_end(&network.name, total.saturating_sub(lead + 12))
            );
            if let Some(location) = network.location() {
                if visible_width(&line) + width(&location) + lead + 3 <= total {
                    line.push_str(&style.dim(&format!(" \u{00B7} {location}")));
                }
            }
            out.push(format!("{}{line}", " ".repeat(lead)));
        }
        for hop in group.hops {
            out.push(hop_row(run, hop, style, &layout, slowest));
        }
    }
    if run.is_running() && (run.hops.len() > run.path_len() || run.hops.is_empty()) {
        out.push(style.dim(&format!("{:>3} \u{2502} \u{2026}", run.path_len() + 1)));
    }
    out
}

fn hop_row(run: &Run, hop: &Hop, style: Style, layout: &Layout, slowest: Duration) -> String {
    let ttl = style.dim(&format!("{:>3}", hop.ttl));
    let rail = if hop.is_destination {
        style.bold_green("\u{25CF}")
    } else {
        style.dim("\u{2502}")
    };
    let stats = hop.stats();
    let host = match hop.address() {
        Some(address) => {
            let text_address = address.to_string();
            let others = hop.addresses().len() - 1;
            let more = if others > 0 {
                format!(" +{others}")
            } else {
                String::new()
            };
            match run.name(address) {
                Some(name) if layout.host >= width(&text_address) + width(&more) + 9 => {
                    let name = truncate_middle(
                        name,
                        layout.host - width(&text_address) - width(&more) - 1,
                    );
                    let shown = if hop.is_destination {
                        style.bold(&name)
                    } else {
                        name
                    };
                    format!("{shown} {}{}", style.dim(&text_address), style.dim(&more))
                }
                Some(name) => {
                    truncate_middle(name, layout.host.saturating_sub(width(&more)))
                        + &style.dim(&more)
                }
                None => {
                    let shown =
                        truncate_middle(&text_address, layout.host.saturating_sub(width(&more)));
                    let shown = if hop.is_destination {
                        style.bold(&shown)
                    } else {
                        shown
                    };
                    format!("{shown}{}", style.dim(&more))
                }
            }
        }
        None => style.dim(if stats.sent > 0 || !run.is_running() {
            "no reply"
        } else {
            "\u{2026}"
        }),
    };
    let padding = " ".repeat(layout.host.saturating_sub(visible_width(&host)));

    // Loss is red only when it reaches the end of the path. A middle hop that drops replies
    // while later hops are clean is rate-limiting them, and reads gray.
    let loss_text = format!(
        "{:>7}",
        if stats.sent == 0 {
            String::new()
        } else {
            format::percent(stats.loss_fraction())
        }
    );
    let loss = if stats.lost() == 0 || !hop.did_reply() {
        style.dim(&loss_text)
    } else if run.loss_carries(hop) {
        style.red(&loss_text)
    } else {
        style.dim(&loss_text)
    };
    let figure = |value: Option<Duration>| {
        format!(
            "{:>FIGURES$}",
            value.map_or_else(String::new, format::ms_bare)
        )
    };
    let last = hop.probes.iter().rev().find_map(Slot::rtt);

    let mut row = format!("{ttl} {rail} {host}{padding}{loss}");
    if !layout.tight {
        row.push_str(&style.dim(&format!("{:>5}", stats.sent)));
    }
    if layout.range {
        row.push_str(&figure(last));
    }
    row.push_str(&match stats.mean {
        Some(mean) => style.latency(mean, &figure(Some(mean))),
        None => figure(None),
    });
    if layout.range {
        row.push_str(&style.dim(&format!("{}{}", figure(stats.min), figure(stats.max))));
    }
    if !layout.tight {
        row.push_str(&figure(stats.jitter));
    }
    if layout.spark {
        row.push_str("  ");
        row.push_str(&sparkline(&hop.probes, style, SPARK, slowest));
    }
    row
}

/// The newest probes of a hop, one cell each: the height is the latency against the slowest
/// reply of the run, a cross is a loss.
pub fn sparkline(probes: &[Slot], style: Style, cells: usize, slowest: Duration) -> String {
    const LEVELS: [char; 8] = [
        '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];
    let settled: Vec<&Slot> = probes
        .iter()
        .filter(|slot| **slot != Slot::Pending)
        .collect();
    let window = &settled[settled.len().saturating_sub(cells)..];
    let mut out = String::new();
    for slot in window {
        match slot.rtt() {
            Some(rtt) => {
                let fraction = if slowest.is_zero() {
                    0.0
                } else {
                    rtt.as_secs_f64() / slowest.as_secs_f64()
                };
                let level = ((fraction * 7.0).round() as usize).min(7);
                out.push_str(&style.latency(rtt, &LEVELS[level].to_string()));
            }
            None => out.push_str(&style.red("\u{00D7}")),
        }
    }
    out
}

/// What is redrawn in place while the run is live.
pub fn live_lines(run: &Run, style: Style, total: usize, frame: usize) -> Vec<String> {
    let view = View {
        run,
        style,
        width: total,
        frame,
        elapsed: None,
        looking_up: false,
    };
    let mut out = vec![pretty::title(&view), String::new()];
    out.extend(table(run, style, total));
    out.push(String::new());
    let status = if run.destination.is_none() {
        "Resolving".to_string()
    } else if run.destination_hop().is_some() {
        format!("Running \u{00B7} {}", run.mtr_summary())
    } else {
        match run.path_len() {
            0 => "Running".to_string(),
            1 => "Running \u{00B7} 1 hop so far".to_string(),
            hops => format!("Running \u{00B7} {hops} hops so far"),
        }
    };
    out.push(format!(
        "{} {}",
        style.blue(SPINNER[frame % SPINNER.len()]),
        style.dim(&status)
    ));
    out
}

/// What stays when the run has ended: the table, the destination's figures, and what they
/// likely mean.
pub fn closing_lines(
    run: &Run,
    style: Style,
    total: usize,
    elapsed: Option<Duration>,
) -> Vec<String> {
    let view = View {
        run,
        style,
        width: total,
        frame: 0,
        elapsed: None,
        looking_up: false,
    };
    let mut out = vec![pretty::title(&view), String::new()];
    out.extend(table(run, style, total));
    out.push(String::new());

    let destination = run
        .destination
        .map_or_else(|| run.target.clone(), |address| address.to_string());
    let count = run.path_len();
    let hops = if count == 1 {
        "1 hop".to_string()
    } else {
        format!("{count} hops")
    };
    let mut facts: Vec<String> = Vec::new();
    let lead = match run.destination_hop().map(Hop::stats) {
        Some(stats) => {
            facts.push(hops);
            match run.network_count() {
                0 => {}
                1 => facts.push("1 network".to_string()),
                networks => facts.push(format!("{networks} networks")),
            }
            let summary = run.mtr_summary();
            if stats.lost() == 0 {
                style.bold_green(&summary)
            } else {
                style.yellow(&summary)
            }
        }
        None => style.yellow(&format!("No reply from {destination} after {hops}")),
    };
    if let Some(elapsed) = elapsed {
        facts.push(format!("{:.0} s", elapsed.as_secs_f64()));
    }
    let limit = total.clamp(40, 160);
    let joined = facts.join(" \u{00B7} ");
    if facts.is_empty() {
        out.push(lead);
    } else if visible_width(&lead) + 3 + width(&joined) <= limit {
        out.push(format!(
            "{lead}{}",
            style.dim(&format!(" \u{00B7} {joined}"))
        ));
    } else {
        out.push(lead);
        out.push(style.dim(&joined));
    }

    if let Some(stats) = run.destination_hop().map(Hop::stats) {
        let figure = |name: &str, value: Option<Duration>| {
            value.map(|value| format!("{name} {}", format::ms_bare(value)))
        };
        let figures: Vec<String> = [
            figure("min", stats.min),
            figure("max", stats.max),
            figure("jitter", stats.jitter),
            figure("p50", stats.p50),
            figure("p95", stats.p95),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !figures.is_empty() {
            for line in pretty::wrap(
                &format!("{} ms", figures.join(" \u{00B7} ")),
                total.clamp(40, 160),
            ) {
                out.push(style.dim(&line));
            }
        }
    }
    // The verdict's title is painted. The sentences after it, and the rate-limit note, stay plain.
    let verdict = run
        .destination_hop()
        .map(|hop| Verdict::of(&hop.stats()))
        .filter(|verdict| verdict.grade != Grade::Collecting);
    for sentence in Report::of(run).interpretation {
        let lines = pretty::wrap(&sentence, total.clamp(40, 100));
        for (index, line) in lines.iter().enumerate() {
            let painted = match &verdict {
                Some(verdict) if index == 0 && line.starts_with(&verdict.title) => {
                    let title = if verdict.grade == Grade::Stable {
                        style.bold_green(&verdict.title)
                    } else {
                        style.yellow(&verdict.title)
                    };
                    line.replacen(&verdict.title, &title, 1)
                }
                _ => line.clone(),
            };
            out.push(painted);
        }
    }
    out
}

/// A saved MTR run, for `rvrs show`, `rvrs last` and anything else that prints one.
pub fn saved_lines(run: &Run, style: Style, total: usize) -> Vec<String> {
    closing_lines(run, style, total, None)
}

// The classic look: mtr's report

#[derive(Debug, Clone, Copy, Default)]
pub struct ReportOptions {
    pub numeric: bool,
    pub show_ips: bool,
    pub wide: bool,
    pub as_lookups: bool,
}

/// "2026-11-20T09:30:00+0000", the way mtr stamps a report. rvrs writes it in UTC.
pub fn start_time(run: &Run) -> String {
    format::iso8601(run.started_at).replace('Z', "+0000")
}

/// This device's name, for the report's HOST line.
pub fn local_hostname() -> String {
    let mut buffer = [0 as libc::c_char; 256];
    // SAFETY: the buffer is writable for its whole length, and one byte is kept for the NUL.
    let status = unsafe { libc::gethostname(buffer.as_mut_ptr(), buffer.len() - 1) };
    if status != 0 {
        return "localhost".to_string();
    }
    // SAFETY: gethostname wrote a NUL-terminated string, or the zeroed buffer ends one.
    let name = unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if name.is_empty() {
        "localhost".to_string()
    } else {
        name
    }
}

fn report_host(run: &Run, hop: &Hop, options: ReportOptions) -> String {
    let Some(address) = hop.address() else {
        return "???".to_string();
    };
    let host = match run.name(address) {
        Some(name) if !options.numeric && options.show_ips => format!("{name} ({address})"),
        Some(name) if !options.numeric => name.to_string(),
        _ => address.to_string(),
    };
    if options.as_lookups {
        let network = run
            .network(address)
            .map_or_else(|| "AS???".to_string(), |network| network.label());
        format!("{network:<8} {host}")
    } else {
        host
    }
}

/// The report mtr prints with `-r`: a HOST line with the column names, then a line per hop with
/// Loss%, Snt, Last, Avg, Best, Wrst and StDev. Without `-w` long names are cut to the column.
pub fn report_lines(run: &Run, options: ReportOptions) -> Vec<String> {
    report_lines_from(run, options, &local_hostname())
}

pub fn report_lines_from(run: &Run, options: ReportOptions, hostname: &str) -> Vec<String> {
    let hops = run.path();
    let hosts: Vec<String> = hops
        .iter()
        .map(|hop| report_host(run, hop, options))
        .collect();
    // "  1.|-- " is eight columns, and the figures start at column 34 in mtr's own report.
    let longest = hosts
        .iter()
        .map(|host| width(host))
        .max()
        .unwrap_or(0)
        .max(width(hostname).saturating_sub(2));
    let host_width = if options.wide { longest.max(26) } else { 26 };
    let figures = |loss: &str, rest: &str| format!("{loss:>5}{rest}");
    let mut out = vec![format!(
        "HOST: {:<name_width$}{}",
        truncate_end(hostname, host_width + 2),
        figures("Loss%", "   Snt   Last   Avg  Best  Wrst StDev"),
        name_width = host_width + 2,
    )];
    for (hop, host) in hops.iter().zip(&hosts) {
        let stats = hop.stats();
        let samples: Vec<f64> = hop
            .probes
            .iter()
            .filter_map(Slot::rtt)
            .map(format::ms_value)
            .collect();
        let ms = |value: Option<Duration>| value.map_or(0.0, format::ms_value);
        let last = samples.last().copied().unwrap_or(0.0);
        let deviation = if samples.is_empty() {
            0.0
        } else {
            let count = samples.len() as f64;
            let mean = samples.iter().sum::<f64>() / count;
            (samples.iter().map(|sample| sample * sample).sum::<f64>() / count - mean * mean)
                .max(0.0)
                .sqrt()
        };
        let loss = format!("{:.1}%", stats.loss_fraction() * 100.0);
        let host = if options.wide {
            host.clone()
        } else {
            truncate_end(host, host_width)
        };
        // A loss of 100.0% is a column wider than the rest, and mtr lets it take a space.
        let left = format!("{:>3}.|-- {:<host_width$}", hop.ttl, host);
        let left = if loss.len() > 5 && left.ends_with(' ') {
            left[..left.len() - 1].to_string()
        } else {
            left
        };
        out.push(format!(
            "{left}{}",
            figures(
                &loss,
                &format!(
                    "{:>6}{:>7.1}{:>6.1}{:>6.1}{:>6.1}{:>6.1}",
                    stats.sent,
                    last,
                    ms(stats.mean),
                    ms(stats.min),
                    ms(stats.max),
                    deviation
                )
            )
        ));
    }
    out
}

#[allow(dead_code)]
pub const USAGE_EXIT: u8 = EXIT_USAGE;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rvrs_core::engine::{Ending, Event, ReplyKind};
    use rvrs_core::model::{Network, Tool};

    fn parse_line(line: &str) -> Result<MtrArgs, String> {
        let arguments: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        match parse(&arguments)? {
            Parsed::Run(args) => Ok(*args),
            other => Err(format!("{other:?}")),
        }
    }

    /// Four hops over `rounds` rounds: a private gateway, a named router that drops what
    /// `drops` says, a silent hop, and the destination.
    pub fn sample(rounds: u32, drops: impl Fn(u32) -> bool) -> Run {
        let mut run = Run::new(Tool::Mtr, "example.com", FamilyChoice::Auto, 0, 30);
        run.destination = Some("93.184.216.34".parse().unwrap());
        run.started_at = 1_794_735_000;
        let reply =
            |ttl: u8, round: u32, from: &str, micros: u64, kind: ReplyKind| Event::MtrReply {
                ttl,
                round,
                from: from.parse().unwrap(),
                rtt: Duration::from_micros(micros),
                kind,
            };
        for round in 0..rounds {
            for ttl in 1..=4 {
                run.apply(&Event::MtrSent { ttl, round });
            }
            run.apply(&reply(
                1,
                round,
                "192.168.1.1",
                400 + u64::from(round % 3) * 100,
                ReplyKind::Intermediate,
            ));
            if drops(round) {
                run.apply(&Event::MtrLost { ttl: 2, round });
            } else {
                run.apply(&reply(
                    2,
                    round,
                    "198.51.100.1",
                    8_000,
                    ReplyKind::Intermediate,
                ));
            }
            run.apply(&Event::MtrLost { ttl: 3, round });
            run.apply(&reply(
                4,
                round,
                "93.184.216.34",
                12_000 + u64::from(round % 2) * 400,
                ReplyKind::Destination,
            ));
        }
        run.apply(&Event::Finished(Ending::Reached(4)));
        run.names.insert(
            "198.51.100.1".parse().unwrap(),
            "ae1.core2.fra.example.net".into(),
        );
        run.networks.insert(
            "198.51.100.1".parse().unwrap(),
            Some(Network {
                number: 64500,
                name: "Example Net".into(),
                country: Some("DE".into()),
                city: None,
                prefix: None,
            }),
        );
        run.networks.insert(
            "93.184.216.34".parse().unwrap(),
            Some(Network {
                number: 64501,
                name: "Example Edge".into(),
                country: None,
                city: None,
                prefix: None,
            }),
        );
        run
    }

    #[test]
    fn defaults_are_mtrs() {
        let args = parse_line("example.com").unwrap();
        assert_eq!(args.options.count, None);
        assert_eq!(args.options.interval, Duration::from_secs(1));
        assert_eq!(args.options.max_ttl, 30);
        assert!(!args.report && !args.wide);
    }

    #[test]
    fn mtrs_flags_attached_or_apart_short_or_long() {
        let args = parse_line(
            "-rwbzn4 -c 5 -i0.5 -m 20 --first-ttl=2 --timeout 3 -U 8 -s 100 example.com",
        )
        .unwrap();
        assert!(args.report && args.wide && args.show_ips && args.as_lookups && args.numeric);
        assert_eq!(args.family, FamilyChoice::V4);
        assert_eq!(args.options.count, Some(5));
        assert_eq!(args.options.interval, Duration::from_millis(500));
        assert_eq!(
            (
                args.options.first_ttl,
                args.options.max_ttl,
                args.options.max_unknown
            ),
            (2, 20, 8)
        );
        assert_eq!(args.options.wait, Duration::from_secs(3));
        assert_eq!(args.options.payload, 72);
        assert_eq!(
            parse_line("--report-cycles 3 example.com")
                .unwrap()
                .options
                .count,
            Some(3)
        );
    }

    #[test]
    fn mtrs_json_flag_is_the_report() {
        assert!(parse_line("-j example.com").unwrap().common.json);
    }

    #[test]
    fn what_rvrs_cant_do_is_an_error_that_says_so() {
        assert!(parse_line("-u example.com")
            .unwrap_err()
            .contains("ICMP echo probes"));
        assert!(parse_line("--tcp example.com")
            .unwrap_err()
            .contains("ICMP echo probes"));
        assert!(parse_line("-t example.com")
            .unwrap_err()
            .contains("for the Console"));
        assert!(parse_line("-C example.com").unwrap_err().contains("--json"));
        assert_eq!(
            parse_line("-c 0 example.com").unwrap_err(),
            "-c takes at least 1 round."
        );
        assert_eq!(
            parse_line("-i 0 example.com").unwrap_err(),
            "-i takes 0.1 to 3600 seconds."
        );
        assert!(parse_line("").unwrap_err().contains("needs a target"));
        assert!(parse_line("-f 9 -m 3 example.com")
            .unwrap_err()
            .contains("beyond the last"));
    }

    #[test]
    fn the_report_is_mtrs() {
        let run = sample(10, |_| false);
        let lines = report_lines_from(&run, ReportOptions::default(), "localhost");
        assert_eq!(
            lines[0],
            "HOST: localhost                   Loss%   Snt   Last   Avg  Best  Wrst StDev"
        );
        assert_eq!(
            lines[1],
            "  1.|-- 192.168.1.1                0.0%    10    0.4   0.5   0.4   0.6   0.1"
        );
        assert_eq!(
            lines[2],
            "  2.|-- ae1.core2.fra.example.net  0.0%    10    8.0   8.0   8.0   8.0   0.0"
        );
        assert_eq!(
            lines[3],
            "  3.|-- ???                      100.0%    10    0.0   0.0   0.0   0.0   0.0"
        );
        assert_eq!(
            lines[4],
            "  4.|-- 93.184.216.34              0.0%    10   12.4  12.2  12.0  12.4   0.2"
        );
        assert_eq!(start_time(&run), "2026-11-15T09:30:00+0000");
    }

    #[test]
    fn report_options_change_the_host_column() {
        let run = sample(4, |_| false);
        let numeric = report_lines_from(
            &run,
            ReportOptions {
                numeric: true,
                ..ReportOptions::default()
            },
            "localhost",
        );
        assert!(numeric[2].starts_with("  2.|-- 198.51.100.1 "));
        let both = report_lines_from(
            &run,
            ReportOptions {
                show_ips: true,
                wide: true,
                ..ReportOptions::default()
            },
            "localhost",
        );
        assert!(both[2].starts_with("  2.|-- ae1.core2.fra.example.net (198.51.100.1) "));
        // The wide report keeps its columns in line.
        let ends: Vec<usize> = both.iter().map(|line| width(line)).collect();
        assert!(ends.windows(2).all(|pair| pair[0] == pair[1]), "{ends:?}");
        let networks = report_lines_from(
            &run,
            ReportOptions {
                as_lookups: true,
                wide: true,
                ..ReportOptions::default()
            },
            "localhost",
        );
        assert!(networks[2].contains("AS64500  ae1.core2.fra.example.net"));
        assert!(networks[1].contains("AS???    192.168.1.1"));
        // Without -w a long name is cut to the column.
        let mut long = sample(2, |_| false);
        long.names.insert(
            "198.51.100.1".parse().unwrap(),
            "a-very-long-router-name.core.frankfurt.example.net".into(),
        );
        let cut = report_lines_from(&long, ReportOptions::default(), "localhost");
        assert_eq!(width(&cut[2]), width(&cut[1]));
    }

    #[test]
    fn the_terminal_table_groups_hops_and_names_its_columns() {
        let run = sample(10, |_| false);
        let lines = table(&run, Style::PLAIN, 100);
        assert!(lines[0]
            .trim_start()
            .starts_with("Loss  Snt   Last    Avg   Best  Worst Jitter"));
        assert_eq!(lines[1], "      AS64500 Example Net \u{00B7} DE");
        assert!(lines[2].starts_with("  1 \u{2502} 192.168.1.1"));
        assert!(lines[3].starts_with("  2 \u{2502} ae1.core2.fra.example.net 198.51.100.1"));
        assert!(lines[4].starts_with("  3 \u{2502} no reply"));
        assert!(lines[4].contains("100.0%"));
        assert_eq!(lines[5], "      AS64501 Example Edge");
        assert!(lines[6].starts_with("  4 \u{25CF} 93.184.216.34"));
        assert!(lines[6].contains("  0.0%   10"));
    }

    #[test]
    fn every_row_fits_the_terminal() {
        let run = sample(12, |round| round % 3 == 0);
        for total in [40, 60, 80, 100, 140, 200] {
            for line in closing_lines(&run, Style::PLAIN, total, Some(Duration::from_secs(12))) {
                assert!(
                    visible_width(&line) <= total.clamp(40, 180),
                    "{total}: {line}"
                );
            }
        }
        // The sparkline comes with room, and has a cell per settled probe.
        assert!(table(&run, Style::PLAIN, 160)[2].contains('\u{2581}'));
        assert!(!table(&run, Style::PLAIN, 90)[2].contains('\u{2581}'));
    }

    #[test]
    fn the_closing_block_says_what_was_measured_and_what_it_likely_means() {
        let run = sample(12, |round| round % 3 == 0);
        let lines = closing_lines(&run, Style::PLAIN, 120, Some(Duration::from_secs(12)));
        let text = lines.join("\n");
        assert_eq!(
            lines[0],
            "// mtr example.com  93.184.216.34 \u{00B7} IPv4 \u{00B7} this device"
        );
        assert!(text.contains("12 pkt \u{00B7} 0.0% loss \u{00B7} 12.2 ms avg \u{00B7} 4 hops \u{00B7} 2 networks \u{00B7} 12 s"));
        assert!(text.contains("min 12.0 \u{00B7} max 12.4"));
        assert!(text.contains("Stable. No loss over 12 packets."));
        assert!(text.contains("Hop 2 answers fewer probes than the hops after it."));
    }

    #[test]
    fn a_silent_target_says_so() {
        let mut run = Run::new(Tool::Mtr, "192.0.2.1", FamilyChoice::Auto, 0, 30);
        run.destination = Some("192.0.2.1".parse().unwrap());
        for round in 0..3 {
            run.apply(&Event::MtrSent { ttl: 1, round });
            run.apply(&Event::MtrReply {
                ttl: 1,
                round,
                from: "192.168.1.1".parse().unwrap(),
                rtt: Duration::from_millis(1),
                kind: ReplyKind::Intermediate,
            });
            run.apply(&Event::MtrSent { ttl: 2, round });
            run.apply(&Event::MtrLost { ttl: 2, round });
        }
        run.apply(&Event::Finished(Ending::Exhausted));
        let text = closing_lines(&run, Style::PLAIN, 100, None).join("\n");
        assert!(text.contains("No reply from 192.0.2.1 after 1 hop"));
        assert!(text.contains("It may drop ICMP"));
        assert_eq!(run.summary(), "No reply after 1 hop");
    }

    #[test]
    fn sparklines_scale_to_the_slowest_reply_and_mark_losses() {
        let slot = |micros: u64| Slot::Reply {
            from: "192.0.2.1".parse().unwrap(),
            micros,
            unreachable: None,
            ttl: None,
        };
        let probes = vec![slot(1_000), Slot::Lost, slot(8_000), Slot::Pending];
        assert_eq!(
            sparkline(&probes, Style::PLAIN, 20, Duration::from_millis(8)),
            "\u{2582}\u{00D7}\u{2588}"
        );
        assert_eq!(
            sparkline(&probes, Style::PLAIN, 1, Duration::from_millis(8)),
            "\u{2588}"
        );
    }
}
