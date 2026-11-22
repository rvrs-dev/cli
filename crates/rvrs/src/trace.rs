//! `rvrs traceroute`: run it, show it in the right look for where the output goes, save it.

use std::io::{IsTerminal, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use rvrs_core::history::History;
use rvrs_core::model::{Outcome, Run};
use rvrs_core::report::Report;
use rvrs_core::session::{Lookups, Session};

use crate::args::TraceArgs;
use crate::classic::{self, ClassicOptions};
use crate::pretty::{self, View};
use crate::style::Style;
use crate::term::{self, LiveRegion};
use crate::{fail, EXIT_FAILED};

/// How long a finished run waits for names and networks still on their way.
const LOOKUP_BUDGET: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    /// The grouped view, redrawn in place while the run is live.
    Pretty,
    /// The original tool's lines.
    Classic,
    /// The report, and nothing else.
    Json,
}

pub fn look(json: bool, classic: bool, multicall: bool) -> Look {
    if json {
        Look::Json
    } else if classic || multicall || !std::io::stdout().is_terminal() {
        Look::Classic
    } else {
        Look::Pretty
    }
}

/// What --fleet says until nodes exist.
pub const NO_FLEET: &str =
    "Nodes aren't available yet, so there is no fleet to run from. Without --fleet the run comes from this device.";

pub fn run(args: TraceArgs, multicall: bool) -> ExitCode {
    if args.common.fleet {
        return fail(NO_FLEET);
    }
    if let Some(node) = &args.common.node {
        return fail(&format!(
            "Nodes aren't available yet, so nothing was sent from \u{201C}{node}\u{201D}. Without --node the run comes from this device."
        ));
    }
    // The TCP probe and the path MTU check are modules of their own. cfg! is a constant, so a
    // build without them never links their code.
    if args.tcp.is_some() && !cfg!(feature = "tcp") {
        return crate::modules::refuse("tcp");
    }
    if args.mtu && !cfg!(feature = "mtu") {
        return crate::modules::refuse("mtu");
    }
    let look = look(args.common.json, args.common.classic, multicall);
    let style = Style::detect(args.common.color);
    let lookups = Lookups {
        names: !args.numeric && !args.no_lookups,
        // The classic look makes network lookups only when asked, as the original does.
        networks: !args.no_lookups && (look != Look::Classic || args.as_lookups),
    };
    let classic_options = ClassicOptions {
        numeric: args.numeric,
        as_lookups: args.as_lookups,
        packet_len: args.packet_len,
    };

    term::catch_interrupts();
    let started = Instant::now();
    let mut session = match args.tcp.filter(|_| cfg!(feature = "tcp")) {
        Some(port) => Session::start_tcp(
            &args.target,
            args.family,
            args.options.clone(),
            port,
            lookups,
        ),
        None => Session::start(&args.target, args.family, args.options.clone(), lookups),
    };
    let mut live = (look == Look::Pretty).then(LiveRegion::start);
    let mut classic_printed = 0usize;
    let mut classic_header = false;
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
        match look {
            Look::Pretty => {
                if let Some(region) = live.as_mut() {
                    let (width, _) = term::size();
                    let view = View {
                        run: &session.run,
                        style,
                        width,
                        frame,
                        elapsed: None,
                        looking_up: false,
                    };
                    region.draw(&pretty::lines(&view), true);
                }
            }
            Look::Classic => print_classic(
                &session,
                classic_options,
                &mut classic_header,
                &mut classic_printed,
                false,
            ),
            Look::Json => {}
        }
        if !session.run.is_running() {
            break;
        }
    }
    let elapsed = started.elapsed();

    // Names and networks still on their way get a moment. The result is already on screen.
    // On a terminal the result is on screen while they land, so they get a little longer.
    let deadline = Instant::now()
        + if look == Look::Pretty {
            LOOKUP_BUDGET + Duration::from_secs(1)
        } else {
            LOOKUP_BUDGET
        };
    while session.pending_lookups() > 0 && Instant::now() < deadline && term::interrupts() < 2 {
        session.pump(Duration::from_millis(80));
        frame += 1;
        if let Some(region) = live.as_mut() {
            let (width, _) = term::size();
            let view = View {
                run: &session.run,
                style,
                width,
                frame,
                elapsed: Some(elapsed),
                looking_up: true,
            };
            region.draw(&pretty::lines(&view), true);
        }
    }
    session.finish_lookups(Duration::ZERO);
    // The path MTU, asked for with --mtu: a few don't-fragment pings once the trace is done.
    if cfg!(feature = "mtu") && args.mtu && term::interrupts() == 0 {
        if let Some(destination) = session.run.destination {
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            match rvrs_core::mtu::discover(destination, Duration::from_secs(1), &stop) {
                Ok(found) => session.run.mtu = Some(found),
                Err(message) => eprintln!("{}", style_error(style, &message)),
            }
        }
    }
    let run = &session.run;

    let mut history = History::open();
    let keep = std::env::var("RVRS_HISTORY").map_or(true, |value| value != "0");
    if keep && !matches!(run.outcome, Some(Outcome::Failed(_))) {
        crate::pulse::count_run(run.tool);
        crate::sync::queue(run);
        if let Err(error) = history.save(run) {
            eprintln!("rvrs: Couldn't save this run to history ({error}).");
        }
    }

    if let Some(Outcome::Failed(message)) = &run.outcome {
        if let Some(region) = live.take() {
            let mut region = region;
            region.draw(&[], false);
        }
        if look == Look::Json {
            println!("{}", Report::of(run).json());
        }
        eprintln!("{}", style_error(style, message));
        return ExitCode::from(EXIT_FAILED);
    }

    match look {
        Look::Pretty => {
            let (width, _) = term::size();
            let view = View {
                run,
                style,
                width,
                frame,
                elapsed: Some(elapsed),
                looking_up: false,
            };
            if let Some(mut region) = live.take() {
                region.draw(&pretty::lines(&view), false);
                region.finish();
            }
        }
        Look::Classic => print_classic(
            &session,
            classic_options,
            &mut classic_header,
            &mut classic_printed,
            true,
        ),
        Look::Json => println!("{}", Report::of(run).json()),
    }
    let _ = std::io::stdout().flush();
    if let Some(case) = &args.common.case {
        crate::commands::save_after_run(run, case);
    }
    ExitCode::SUCCESS
}

/// Prints the classic lines that are ready: a hop once all its probes have settled and no
/// lookup for it is still out, in order, each line once.
fn print_classic(
    session: &Session,
    options: ClassicOptions,
    header: &mut bool,
    printed: &mut usize,
    finished: bool,
) {
    let run = &session.run;
    if run.destination.is_none() {
        return;
    }
    let mut stdout = std::io::stdout().lock();
    if !*header {
        let _ = writeln!(stdout, "{}", classic::header(run, options));
        *header = true;
    }
    let ready = finished || session.pending_lookups() == 0;
    // A silent target prints its stars up to the last hop, as the original does. Otherwise the
    // lines end at the hop that ended the path.
    let end = if finished || !run.is_running() {
        if run.outcome == Some(Outcome::Exhausted) && run.unreachable().is_none() {
            run.hops.len()
        } else {
            run.path_len()
        }
    } else {
        run.path_len()
    };
    while *printed < end {
        let hop = &run.hops[*printed];
        if !(finished || (hop.is_settled() && ready)) {
            break;
        }
        let _ = writeln!(stdout, "{}", classic::hop_line(run, hop, options));
        *printed += 1;
    }
    let _ = stdout.flush();
}

pub fn style_error(style: Style, message: &str) -> String {
    format!("{} {message}", style.red("rvrs:"))
}

/// A saved run in the look that fits where the output goes.
pub fn show(run: &Run, json: bool, classic: bool, style: Style) {
    if run.is_report_only() {
        // A run another surface made, whose record a report doesn't carry.
        if json {
            println!("{}", Report::of(run).json());
            return;
        }
        let (width, _) = term::size();
        let view = pretty::View {
            run,
            style,
            width,
            frame: 0,
            elapsed: None,
            looking_up: false,
        };
        println!("{}", pretty::title(&view));
        println!();
        for line in pretty::report_lines(run, style, width) {
            println!("{line}");
        }
        return;
    }
    if run.tool == rvrs_core::model::Tool::Mtr {
        match look(json, classic, false) {
            Look::Json => println!("{}", Report::of(run).json()),
            Look::Classic => {
                println!("Start: {}", crate::mtr::start_time(run));
                for line in crate::mtr::report_lines(run, crate::mtr::ReportOptions::default()) {
                    println!("{line}");
                }
            }
            Look::Pretty => {
                let (width, _) = term::size();
                for line in crate::mtr::saved_lines(run, style, width) {
                    println!("{line}");
                }
            }
        }
        return;
    }
    if run.tool == rvrs_core::model::Tool::Dns {
        match look(json, classic, false) {
            Look::Json => println!("{}", Report::of(run).json()),
            Look::Classic => {
                for line in crate::dig::zone_lines(run) {
                    println!("{line}");
                }
            }
            Look::Pretty => {
                let (width, _) = term::size();
                for line in crate::dig::saved_lines(run, style, width) {
                    println!("{line}");
                }
            }
        }
        return;
    }
    if run.tool == rvrs_core::model::Tool::Bgp {
        match look(json, classic, false) {
            Look::Json => println!("{}", Report::of(run).json()),
            Look::Classic => {
                if let Some(bgp) = &run.bgp {
                    println!("{}", bgp.export(&run.target, run.started_at));
                }
            }
            Look::Pretty => {
                let (width, _) = term::size();
                for line in crate::bgp::saved_lines(run, style, width) {
                    println!("{line}");
                }
            }
        }
        return;
    }
    if run.tool == rvrs_core::model::Tool::Ping {
        match look(json, classic, false) {
            Look::Json => println!("{}", Report::of(run).json()),
            Look::Classic => {
                println!("{}", crate::ping::classic_header(run, 56));
                for seq in 0..run.packets().len() {
                    if let Some(line) = crate::ping::classic_line(run, seq, 56, false) {
                        println!("{line}");
                    }
                }
                // A saved run doesn't keep how long it took, so the statistics leave the time at 0.
                for line in crate::ping::classic_statistics(run, Duration::ZERO) {
                    println!("{line}");
                }
            }
            Look::Pretty => {
                let (width, _) = term::size();
                for line in crate::ping::saved_lines(run, style, width) {
                    println!("{line}");
                }
            }
        }
        return;
    }
    match look(json, classic, false) {
        Look::Json => println!("{}", Report::of(run).json()),
        Look::Classic => {
            let options = ClassicOptions {
                numeric: false,
                as_lookups: false,
                packet_len: 60,
            };
            println!("{}", classic::header(run, options));
            let end = if run.outcome == Some(Outcome::Exhausted) && run.unreachable().is_none() {
                run.hops.len()
            } else {
                run.path_len()
            };
            for hop in &run.hops[..end] {
                println!("{}", classic::hop_line(run, hop, options));
            }
        }
        Look::Pretty => {
            let (width, _) = term::size();
            let view = View {
                run,
                style,
                width,
                frame: 0,
                elapsed: None,
                looking_up: false,
            };
            for line in pretty::lines(&view) {
                println!("{line}");
            }
        }
    }
}
