//! `rvrs ping`: ping's flags, its lines in a pipe, and on a terminal one line per packet over
//! a live sparkline, ending in the figures and the app's verdict. It prints and returns; it
//! never opens the Console.

use std::io::Write;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use rvrs_core::engine::PingOptions;
use rvrs_core::format;
use rvrs_core::history::History;
use rvrs_core::model::{Grade, Outcome, Run, Slot, Verdict};
use rvrs_core::report::{Report, SILENT_TARGET};
use rvrs_core::session::{Lookups, Session};
use rvrs_core::FamilyChoice;

use crate::args::{normalize_target, Common};
use crate::classic::unreachable_note;
use crate::pretty::{self, View};
use crate::style::{ColorChoice, Style, SPINNER};
use crate::term::{self, LiveRegion};
use crate::trace::{self, Look};
use crate::EXIT_USAGE;

#[derive(Debug, Clone, PartialEq)]
pub struct PingArgs {
    pub target: String,
    pub options: PingOptions,
    pub family: FamilyChoice,
    /// `-n`: addresses only, no reverse DNS.
    pub numeric: bool,
    /// `-q`: nothing but the opening line and the statistics.
    pub quiet: bool,
    pub no_lookups: bool,
    pub common: Common,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Run(Box<PingArgs>),
    Help,
    Version,
}

/// ping's short flags that take a value, supported or not, so the parser reads past them right.
const VALUED: &str = "ciWstwIQmMpSlTe";

fn number<T: std::str::FromStr>(flag: &str, value: &str, what: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} takes {what}, and \u{201C}{value}\u{201D} isn't one."))
}

pub fn parse(arguments: &[String]) -> Result<Parsed, String> {
    let mut options = PingOptions::default();
    let mut family = FamilyChoice::Auto;
    let (mut numeric, mut quiet, mut no_lookups) = (false, false, false);
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
                    return Err(format!(
                        "rvrs ping has no flag --{other}. Supported flags: rvrs ping --help"
                    ))
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
            match flag {
                'h' => return Ok(Parsed::Help),
                'V' => return Ok(Parsed::Version),
                'c' => {
                    let count: u32 = number(&shown, &value, "a number of packets")?;
                    if count == 0 {
                        return Err("-c takes at least 1 packet.".to_string());
                    }
                    options.count = Some(count);
                }
                'i' => {
                    let seconds: f64 = number(&shown, &value, "a number of seconds")?;
                    if !(0.01..=3600.0).contains(&seconds) {
                        return Err("-i takes 0.01 to 3600 seconds.".to_string());
                    }
                    options.interval = Duration::from_secs_f64(seconds);
                }
                'W' => {
                    let seconds: f64 = number(&shown, &value, "a number of seconds")?;
                    if !(seconds > 0.0 && seconds <= 60.0) {
                        return Err("-W takes more than 0 and at most 60 seconds.".to_string());
                    }
                    options.wait = Duration::from_secs_f64(seconds);
                }
                'w' => {
                    let seconds: f64 = number(&shown, &value, "a number of seconds")?;
                    if seconds <= 0.0 {
                        return Err("-w takes more than 0 seconds.".to_string());
                    }
                    options.deadline = Some(Duration::from_secs_f64(seconds));
                }
                's' => {
                    let size: usize = number(&shown, &value, "a number of bytes")?;
                    if size > 65_507 {
                        return Err("-s takes 0 to 65507 bytes.".to_string());
                    }
                    options.payload = size;
                }
                't' => {
                    let ttl: u32 = number(&shown, &value, "a TTL")?;
                    if !(1..=255).contains(&ttl) {
                        return Err("-t takes a TTL from 1 to 255.".to_string());
                    }
                    options.ttl = ttl as u8;
                }
                'n' => numeric = true,
                'q' => quiet = true,
                '4' => family = FamilyChoice::V4,
                '6' => family = FamilyChoice::V6,
                'f' => return Err("-f floods the target, which rvrs doesn't do.".to_string()),
                'I' => return Err("-I isn't supported yet. rvrs sends from the address and interface the system routes through.".to_string()),
                other => return Err(format!("-{other} isn't supported yet. Supported flags: rvrs ping --help")),
            }
        }
    }

    let Some(target) = positional.first() else {
        return Err("rvrs ping needs a target, such as rvrs ping 1.1.1.1".to_string());
    };
    if positional.len() > 1 {
        return Err(format!(
            "rvrs ping takes one target, and \u{201C}{}\u{201D} is a second.",
            positional[1]
        ));
    }
    Ok(Parsed::Run(Box::new(PingArgs {
        target: normalize_target(target),
        options,
        family,
        numeric,
        quiet,
        no_lookups,
        common,
    })))
}

pub fn run(args: PingArgs, multicall: bool) -> ExitCode {
    let style = Style::detect(args.common.color);
    if args.common.fleet {
        eprintln!(
            "{}",
            trace::style_error(Style::detect_stderr(), trace::NO_FLEET)
        );
        return ExitCode::from(EXIT_USAGE);
    }
    if let Some(node) = &args.common.node {
        eprintln!(
            "{}",
            trace::style_error(
                Style::detect_stderr(),
                &format!("Nodes aren't available yet, so nothing was sent from \u{201C}{node}\u{201D}. Without --node the run comes from this device.")
            )
        );
        return ExitCode::from(EXIT_USAGE);
    }
    let look = trace::look(args.common.json, args.common.classic, multicall);
    let lookups = Lookups {
        names: !args.numeric && !args.no_lookups,
        // The classic look makes no network lookups, as the original makes none.
        networks: !args.no_lookups && look != Look::Classic,
    };

    term::catch_interrupts();
    let started = Instant::now();
    let mut session = Session::start_ping(&args.target, args.family, args.options.clone(), lookups);
    let mut live = (look == Look::Pretty).then(LiveRegion::start);
    let mut printed: Vec<bool> = Vec::new();
    let mut header = false;
    let mut first_reply: Option<Instant> = None;
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
        if first_reply.is_none() && run.ping_stats().received > 0 {
            first_reply = Some(Instant::now());
        }
        // Lines wait for the target's name for a moment, so the first reads like the rest.
        let names_ready = session.pending_lookups() == 0
            || first_reply.is_some_and(|at| at.elapsed() > Duration::from_millis(500))
            || !run.is_running();
        match look {
            Look::Pretty => {
                if let Some(region) = live.as_mut() {
                    let (width, _) = term::size();
                    if !header && run.destination.is_some() {
                        header = true;
                        let view = View {
                            run,
                            style,
                            width,
                            frame,
                            elapsed: None,
                            looking_up: false,
                        };
                        region.print_above(&[pretty::title(&view), String::new()]);
                    }
                    if !args.quiet {
                        let fresh = fresh_packets(run, &mut printed);
                        let lines: Vec<String> = fresh
                            .into_iter()
                            .map(|seq| packet_line(run, seq, style))
                            .collect();
                        region.print_above(&lines);
                    }
                    region.draw(&live_lines(run, style, width, frame), true);
                }
            }
            Look::Classic => {
                if run.destination.is_some() && !header {
                    println!("{}", classic_header(run, args.options.payload));
                    header = true;
                }
                if !args.quiet && names_ready {
                    let mut stdout = std::io::stdout().lock();
                    for seq in fresh_packets(run, &mut printed) {
                        if let Some(line) =
                            classic_line(run, seq, args.options.payload, args.numeric)
                        {
                            let _ = writeln!(stdout, "{line}");
                        }
                    }
                    let _ = stdout.flush();
                }
            }
            Look::Json => {}
        }
        if !session.run.is_running() {
            break;
        }
    }
    let elapsed = started.elapsed();
    session.finish_lookups(if look == Look::Pretty {
        Duration::from_millis(600)
    } else {
        Duration::ZERO
    });
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

    let keep = std::env::var("RVRS_HISTORY").map_or(true, |value| value != "0");
    if keep && !run.packets().is_empty() {
        let mut history = History::open();
        crate::pulse::count_run(run.tool);
        crate::sync::queue(run);
        if let Err(error) = history.save(run) {
            eprintln!("rvrs: Couldn't save this run to history ({error}).");
        }
    }
    let case = args
        .common
        .case
        .as_deref()
        .filter(|_| !run.packets().is_empty());

    match look {
        Look::Pretty => {
            if let Some(mut region) = live.take() {
                let (width, _) = term::size();
                // The title is already above the packets.
                let lines = closing_lines(run, style, width);
                region.draw(if header { &lines[2..] } else { &lines[1..] }, false);
                region.finish();
            }
        }
        Look::Classic => {
            for line in classic_statistics(run, elapsed) {
                println!("{line}");
            }
        }
        Look::Json => println!("{}", Report::of(run).json()),
    }
    let _ = std::io::stdout().flush();
    if let Some(case) = case {
        crate::commands::save_after_run(run, case);
    }

    // ping's own codes: 0 when the target answered, 1 when it didn't. With a count and a
    // deadline, every packet must have been answered.
    let stats = run.ping_stats();
    let answered = match (args.options.count, args.options.deadline) {
        (Some(count), Some(_)) => stats.received as u32 >= count,
        _ => stats.received > 0,
    };
    if answered {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Packets that have settled since the last call, in the order they were sent.
fn fresh_packets(run: &Run, printed: &mut Vec<bool>) -> Vec<usize> {
    let packets = run.packets();
    printed.resize(packets.len().max(printed.len()), false);
    let mut fresh = Vec::new();
    for (seq, slot) in packets.iter().enumerate() {
        if !printed[seq] && *slot != Slot::Pending {
            printed[seq] = true;
            fresh.push(seq);
        }
    }
    fresh
}

// The terminal look

/// "    3   11.2 ms  ttl 57"
pub fn packet_line(run: &Run, seq: usize, style: Style) -> String {
    let number = style.dim(&format!("{:>5}", seq + 1));
    match run.packets().get(seq) {
        Some(Slot::Reply { micros, ttl, .. }) => {
            let time = style.latency(
                Duration::from_micros(*micros),
                &format!("{:>9}", format::ms(Duration::from_micros(*micros))),
            );
            match ttl {
                Some(ttl) => format!("{number}  {time}  {}", style.dim(&format!("ttl {ttl}"))),
                None => format!("{number}  {time}"),
            }
        }
        Some(Slot::Refused {
            from,
            time_exceeded: true,
            ..
        }) => {
            format!(
                "{number}  {}",
                style.yellow(&format!("TTL ran out at {from}"))
            )
        }
        Some(Slot::Refused { from, code, .. }) => {
            let note = unreachable_note(*code, from.is_ipv6());
            let note = if note.is_empty() {
                String::new()
            } else {
                format!(" ({note})")
            };
            format!(
                "{number}  {}",
                style.yellow(&format!("{from} reports the target unreachable{note}"))
            )
        }
        _ => format!("{number}  {}", style.yellow("no reply")),
    }
}

/// One cell per packet, newest on the right: the height is the latency, a cross is a loss.
pub fn sparkline(run: &Run, style: Style, cells: usize) -> String {
    const LEVELS: [char; 8] = [
        '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];
    let packets = run.packets();
    let window = &packets[packets.len().saturating_sub(cells)..];
    let slowest = window
        .iter()
        .filter_map(Slot::rtt)
        .max()
        .unwrap_or_default();
    let mut out = String::new();
    for slot in window {
        match slot {
            Slot::Reply { micros, .. } => {
                let fraction = if slowest.is_zero() {
                    0.0
                } else {
                    *micros as f64 / slowest.as_micros() as f64
                };
                let level = ((fraction * 7.0).round() as usize).min(7);
                out.push_str(
                    &style.latency(Duration::from_micros(*micros), &LEVELS[level].to_string()),
                );
            }
            Slot::Pending => out.push_str(&style.dim("\u{00B7}")),
            _ => out.push_str(&style.red("\u{00D7}")),
        }
    }
    out
}

fn chart_cells(width: usize) -> usize {
    width.clamp(40, 160).saturating_sub(4).min(72)
}

/// What is redrawn in place while the run is live: the title, the sparkline and the status.
pub fn live_lines(run: &Run, style: Style, width: usize, frame: usize) -> Vec<String> {
    let spinner = style.blue(SPINNER[frame % SPINNER.len()]);
    let status = if run.destination.is_none() {
        "Resolving".to_string()
    } else if run.ping_stats().sent == 0 {
        "Running".to_string()
    } else {
        format!("Running \u{00B7} {}", run.packet_summary())
    };
    vec![
        String::new(),
        format!("  {}", sparkline(run, style, chart_cells(width))),
        format!("{spinner} {}", style.dim(&status)),
    ]
}

/// What stays when the run has ended: the sparkline, the figures and the verdict.
pub fn closing_lines(run: &Run, style: Style, width: usize) -> Vec<String> {
    let stats = run.ping_stats();
    let view = View {
        run,
        style,
        width,
        frame: 0,
        elapsed: None,
        looking_up: false,
    };
    let destination = run
        .destination
        .map_or_else(|| run.target.clone(), |address| address.to_string());
    let mut out = vec![String::new(), pretty::title(&view), String::new()];
    if !run.packets().is_empty() {
        out.push(format!("  {}", sparkline(run, style, chart_cells(width))));
        out.push(String::new());
    }
    if stats.received == 0 {
        let lead = style.yellow(&format!("No reply from {destination}"));
        out.push(format!(
            "{lead}{}",
            style.dim(&format!(" \u{00B7} {}", run.packet_summary()))
        ));
        match run.reading() {
            Some(reading) => {
                for sentence in reading {
                    out.extend(pretty::wrap(sentence, width.clamp(40, 100)));
                }
            }
            None if stats.sent > 0 => {
                out.extend(pretty::wrap(SILENT_TARGET, width.clamp(40, 100)));
            }
            None => {}
        }
        return out;
    }
    let summary = run.packet_summary();
    out.push(if stats.lost() == 0 {
        style.bold_green(&summary)
    } else {
        style.yellow(&summary)
    });
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
        out.push(style.dim(&format!("{} ms", figures.join(" \u{00B7} "))));
    }
    // A run that came as a report says it in its maker's words.
    if let Some(reading) = run.reading() {
        for sentence in reading {
            out.extend(pretty::wrap(sentence, width.clamp(40, 100)));
        }
        return out;
    }
    let verdict = Verdict::of(&stats);
    if verdict.grade != Grade::Collecting {
        let title = match verdict.grade {
            Grade::Stable => style.bold_green(&verdict.title),
            _ => style.yellow(&verdict.title),
        };
        let lines = pretty::wrap(
            &format!("{}. {}", verdict.title, verdict.detail),
            width.clamp(40, 100),
        );
        for (index, line) in lines.iter().enumerate() {
            // The title is painted; the sentence after it stays plain.
            out.push(if index == 0 {
                line.replacen(&verdict.title, &title, 1)
            } else {
                line.clone()
            });
        }
    }
    out
}

/// A saved ping, for `rvrs show`, `rvrs last` and the Console.
pub fn saved_lines(run: &Run, style: Style, width: usize) -> Vec<String> {
    closing_lines(run, style, width)
        .into_iter()
        .skip(1)
        .collect()
}

// The classic look, modeled on iputils ping

/// "PING one.one.one.one (1.1.1.1) 56(84) bytes of data."
pub fn classic_header(run: &Run, payload: usize) -> String {
    match run.destination {
        Some(address) if address.is_ipv6() => {
            format!("PING {}({address}) {payload} data bytes", run.target)
        }
        Some(address) => format!(
            "PING {} ({address}) {payload}({}) bytes of data.",
            run.target,
            payload + 28
        ),
        None => format!("PING {}", run.target),
    }
}

/// "64 bytes from one.one.one.one (1.1.1.1): icmp_seq=1 ttl=57 time=11.2 ms". A lost packet has
/// no line, as in the original.
pub fn classic_line(run: &Run, seq: usize, payload: usize, numeric: bool) -> Option<String> {
    let host = |address: std::net::IpAddr| match run.name(address) {
        Some(name) if !numeric => format!("{name} ({address})"),
        _ => address.to_string(),
    };
    match run.packets().get(seq)? {
        Slot::Reply {
            from, micros, ttl, ..
        } => {
            let ttl = ttl.map_or_else(String::new, |ttl| format!(" ttl={ttl}"));
            Some(format!(
                "{} bytes from {}: icmp_seq={}{ttl} time={} ms",
                payload + 8,
                host(*from),
                seq + 1,
                classic_time(*micros as f64 / 1000.0)
            ))
        }
        Slot::Refused {
            from,
            time_exceeded: true,
            ..
        } => Some(format!(
            "From {} icmp_seq={} Time to live exceeded",
            host(*from),
            seq + 1
        )),
        Slot::Refused { from, code, .. } => {
            let what = match code {
                0 => "Destination Net Unreachable".to_string(),
                1 => "Destination Host Unreachable".to_string(),
                2 => "Destination Protocol Unreachable".to_string(),
                3 => "Destination Port Unreachable".to_string(),
                13 => "Packet filtered".to_string(),
                other => format!("Destination unreachable, code {other}"),
            };
            Some(format!("From {} icmp_seq={} {what}", host(*from), seq + 1))
        }
        _ => None,
    }
}

/// Three significant digits, as the original prints a time.
pub fn classic_time(ms: f64) -> String {
    if ms < 1.0 {
        format!("{ms:.3}")
    } else if ms < 10.0 {
        format!("{ms:.2}")
    } else if ms < 100.0 {
        format!("{ms:.1}")
    } else {
        format!("{ms:.0}")
    }
}

/// The closing block: the dashes, the counts and the rtt line.
pub fn classic_statistics(run: &Run, elapsed: Duration) -> Vec<String> {
    let packets = run.packets();
    let stats = run.ping_stats();
    let errors = packets
        .iter()
        .filter(|slot| matches!(slot, Slot::Refused { .. }))
        .count();
    let loss = if stats.sent == 0 {
        0.0
    } else {
        stats.lost() as f64 * 100.0 / stats.sent as f64
    };
    let errors = if errors > 0 {
        format!("+{errors} errors, ")
    } else {
        String::new()
    };
    let mut out = vec![
        String::new(),
        format!("--- {} ping statistics ---", run.target),
        format!(
            "{} packets transmitted, {} received, {errors}{}% packet loss, time {}ms",
            stats.sent,
            stats.received,
            shortest(loss),
            elapsed.as_millis()
        ),
    ];
    let samples: Vec<f64> = packets
        .iter()
        .filter_map(Slot::rtt)
        .map(format::ms_value)
        .collect();
    if !samples.is_empty() {
        let count = samples.len() as f64;
        let mean = samples.iter().sum::<f64>() / count;
        let deviation = (samples.iter().map(|sample| sample * sample).sum::<f64>() / count
            - mean * mean)
            .max(0.0)
            .sqrt();
        let min = samples.iter().copied().fold(f64::INFINITY, f64::min);
        let max = samples.iter().copied().fold(0.0, f64::max);
        out.push(format!(
            "rtt min/avg/max/mdev = {min:.3}/{mean:.3}/{max:.3}/{deviation:.3} ms"
        ));
    }
    out
}

/// A number the way C's %g prints it: up to six significant digits, no trailing zeros.
fn shortest(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() {
        "0".to_string()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rvrs_core::engine::{Ending, Event};
    use rvrs_core::model::Tool;

    fn parse_line(line: &str) -> Result<PingArgs, String> {
        let arguments: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        match parse(&arguments)? {
            Parsed::Run(args) => Ok(*args),
            other => Err(format!("{other:?}")),
        }
    }

    pub fn sample(times: &[Option<f64>]) -> Run {
        let mut run = Run::new(Tool::Ping, "one.one.one.one", FamilyChoice::Auto, 1, 64);
        run.destination = Some("1.1.1.1".parse().unwrap());
        for (seq, time) in times.iter().enumerate() {
            let seq = seq as u32;
            run.apply(&Event::PingSent { seq });
            match time {
                Some(ms) => run.apply(&Event::PingReply {
                    seq,
                    from: "1.1.1.1".parse().unwrap(),
                    rtt: Duration::from_secs_f64(ms / 1000.0),
                    ttl: Some(57),
                }),
                None => run.apply(&Event::PingLost { seq }),
            };
        }
        run.apply(&Event::Finished(if times.iter().any(Option::is_some) {
            Ending::Reached(64)
        } else {
            Ending::Exhausted
        }));
        run
    }

    #[test]
    fn defaults_are_pings() {
        let args = parse_line("example.com").unwrap();
        assert_eq!(args.options.count, None);
        assert_eq!(args.options.interval, Duration::from_secs(1));
        assert_eq!(args.options.payload, 56);
        assert_eq!(args.options.ttl, 64);
    }

    #[test]
    fn pings_flags_attached_or_apart() {
        let args = parse_line("-c 5 -i0.2 -W 1 -s 100 -t 12 -w 30 -nq4 example.com").unwrap();
        assert_eq!(args.options.count, Some(5));
        assert_eq!(args.options.interval, Duration::from_millis(200));
        assert_eq!(args.options.wait, Duration::from_secs(1));
        assert_eq!((args.options.payload, args.options.ttl), (100, 12));
        assert_eq!(args.options.deadline, Some(Duration::from_secs(30)));
        assert!(args.numeric && args.quiet);
        assert_eq!(args.family, FamilyChoice::V4);
        let args = parse_line("-c 2 --case vpn-drop example.com").unwrap();
        assert_eq!(args.common.case.as_deref(), Some("vpn-drop"));
        assert!(parse_line("--case= example.com").is_err());
    }

    #[test]
    fn errors_name_the_flag_and_the_rule() {
        assert_eq!(
            parse_line("-c 0 example.com").unwrap_err(),
            "-c takes at least 1 packet."
        );
        assert_eq!(
            parse_line("-i 0 example.com").unwrap_err(),
            "-i takes 0.01 to 3600 seconds."
        );
        assert_eq!(
            parse_line("-f example.com").unwrap_err(),
            "-f floods the target, which rvrs doesn't do."
        );
        assert!(parse_line("-c").unwrap_err().contains("needs a value"));
        assert!(parse_line("").unwrap_err().contains("needs a target"));
        assert!(parse_line("a.example b.example")
            .unwrap_err()
            .contains("is a second"));
    }

    #[test]
    fn classic_lines_are_the_originals() {
        let mut run = sample(&[Some(11.234), None, Some(0.512), Some(123.4)]);
        assert_eq!(
            classic_header(&run, 56),
            "PING one.one.one.one (1.1.1.1) 56(84) bytes of data."
        );
        assert_eq!(
            classic_line(&run, 0, 56, false).unwrap(),
            "64 bytes from 1.1.1.1: icmp_seq=1 ttl=57 time=11.2 ms"
        );
        assert_eq!(classic_line(&run, 1, 56, false), None);
        run.names
            .insert("1.1.1.1".parse().unwrap(), "one.one.one.one".into());
        assert_eq!(
            classic_line(&run, 2, 56, false).unwrap(),
            "64 bytes from one.one.one.one (1.1.1.1): icmp_seq=3 ttl=57 time=0.512 ms"
        );
        assert_eq!(
            classic_line(&run, 3, 56, true).unwrap(),
            "64 bytes from 1.1.1.1: icmp_seq=4 ttl=57 time=123 ms"
        );
    }

    #[test]
    fn classic_statistics_are_the_originals() {
        let run = sample(&[Some(10.0), None, Some(12.0)]);
        let lines = classic_statistics(&run, Duration::from_millis(2003));
        assert_eq!(lines[1], "--- one.one.one.one ping statistics ---");
        assert_eq!(
            lines[2],
            "3 packets transmitted, 2 received, 33.3333% packet loss, time 2003ms"
        );
        assert_eq!(
            lines[3],
            "rtt min/avg/max/mdev = 10.000/11.000/12.000/1.000 ms"
        );
        let silent = classic_statistics(&sample(&[None, None]), Duration::from_secs(1));
        assert_eq!(
            silent[2],
            "2 packets transmitted, 0 received, 100% packet loss, time 1000ms"
        );
        assert_eq!(silent.len(), 3);
        let clean = classic_statistics(&sample(&[Some(1.0)]), Duration::from_secs(1));
        assert!(clean[2].contains(", 0% packet loss"));
    }

    #[test]
    fn a_router_that_refuses_is_reported_as_the_original_does() {
        let mut run = sample(&[Some(10.0)]);
        run.apply(&Event::PingSent { seq: 1 });
        run.apply(&Event::PingRefused {
            seq: 1,
            from: "10.0.0.1".parse().unwrap(),
            time_exceeded: true,
            code: 0,
        });
        assert_eq!(
            classic_line(&run, 1, 56, true).unwrap(),
            "From 10.0.0.1 icmp_seq=2 Time to live exceeded"
        );
        assert!(classic_statistics(&run, Duration::from_secs(1))[2]
            .contains("+1 errors, 50% packet loss"));
        assert!(packet_line(&run, 1, Style::PLAIN).ends_with("TTL ran out at 10.0.0.1"));
    }

    #[test]
    fn terminal_lines_are_seq_time_and_ttl() {
        let run = sample(&[Some(11.234), None]);
        assert_eq!(
            packet_line(&run, 0, Style::PLAIN),
            "    1    11.2 ms  ttl 57"
        );
        assert_eq!(packet_line(&run, 1, Style::PLAIN), "    2  no reply");
    }

    #[test]
    fn the_sparkline_has_a_cell_per_packet_and_a_cross_per_loss() {
        let run = sample(&[Some(1.0), Some(8.0), None, Some(4.0)]);
        assert_eq!(
            sparkline(&run, Style::PLAIN, 60),
            "\u{2582}\u{2588}\u{00D7}\u{2585}"
        );
        assert_eq!(sparkline(&run, Style::PLAIN, 2), "\u{00D7}\u{2588}");
    }

    #[test]
    fn the_closing_block_states_figures_then_the_verdict() {
        let times: Vec<Option<f64>> = (0..12)
            .map(|index| Some(10.0 + f64::from(index % 2) * 0.4))
            .collect();
        let lines = closing_lines(&sample(&times), Style::PLAIN, 80);
        assert_eq!(
            lines[1],
            "// ping one.one.one.one  1.1.1.1 \u{00B7} IPv4 \u{00B7} this device"
        );
        assert_eq!(lines[5], "12 pkt \u{00B7} 0.0% loss \u{00B7} 10.2 ms avg");
        assert_eq!(
            lines[6],
            "min 10.0 \u{00B7} max 10.4 \u{00B7} jitter 0.4 \u{00B7} p50 10.0 \u{00B7} p95 10.4 ms"
        );
        assert_eq!(
            lines[7],
            "Stable. No loss over 12 packets. p95 10.4 ms, jitter 0.4 ms."
        );
    }

    #[test]
    fn a_silent_target_says_what_it_can_and_cant_mean() {
        let lines = closing_lines(&sample(&[None, None, None]), Style::PLAIN, 100);
        assert_eq!(
            lines[5],
            "No reply from 1.1.1.1 \u{00B7} 3 pkt \u{00B7} 100.0% loss"
        );
        assert_eq!(lines[6], SILENT_TARGET);
    }

    #[test]
    fn percentages_print_as_the_original_does() {
        assert_eq!(shortest(0.0), "0");
        assert_eq!(shortest(100.0), "100");
        assert_eq!(shortest(33.333_333), "33.3333");
        assert_eq!(shortest(2.5), "2.5");
        assert_eq!(classic_time(0.0512), "0.051");
        assert_eq!(classic_time(5.678), "5.68");
    }
}
