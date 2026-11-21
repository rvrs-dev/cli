//! A traceroute on a terminal, the rvrs way: hops grouped under the network they belong to, as
//! the app's result screen groups them, names beside addresses, every probe's latency in its
//! own column, and one line of facts at the end.

use std::time::Duration;

use rvrs_core::format;
use rvrs_core::model::{groups, Hop, Outcome, Run, Slot};
use rvrs_core::report::Report;

use crate::classic::unreachable_note;
use crate::style::{truncate_end, truncate_middle, visible_width, width, Style, SPINNER};

pub struct View<'a> {
    pub run: &'a Run,
    pub style: Style,
    pub width: usize,
    /// Advances the spinner while the run is live.
    pub frame: usize,
    /// How long the run took, once it has ended.
    pub elapsed: Option<Duration>,
    /// Lookups still due after the probes are done.
    pub looking_up: bool,
}

const CELL: usize = 7;
const BAR: usize = 14;

/// The whole view, top to bottom, one string per line.
pub fn lines(view: &View) -> Vec<String> {
    let mut out = vec![title(view), String::new()];
    out.extend(table(view.run, view.style, view.width));
    out.push(String::new());
    out.extend(footer(view));
    out
}

/// "// traceroute one.one.one.one  1.1.1.1 · IPv4 · this device"
pub fn title(view: &View) -> String {
    let run = view.run;
    let style = view.style;
    let mut facts: Vec<String> = Vec::new();
    match run.destination {
        Some(address) => {
            if !run.has_literal_target() {
                facts.push(address.to_string());
            }
            if let Some(family) = run.resolved_family() {
                facts.push(family.label().to_string());
            }
        }
        None if run.is_running() => facts.push("resolving".to_string()),
        None => {}
    }
    if run.tool == rvrs_core::model::Tool::Ping {
        if let Some(network) = run.destination.and_then(|address| run.network(address)) {
            facts.push(format!("{} {}", network.label(), network.name));
        }
    }
    if let Some(tcp) = &run.tcp {
        facts.push(format!("TCP port {}", tcp.port));
    }
    facts.push("this device".to_string());
    let lead = format!("{} {} ", style.bold("//"), style.dim(run.tool.command()));
    let room = view
        .width
        .clamp(40, 160)
        .saturating_sub(visible_width(&lead));
    let target = truncate_middle(&run.target, room);
    // Facts that don't fit go, the last one first.
    while !facts.is_empty() && width(&target) + 2 + width(&facts.join(" \u{00B7} ")) > room {
        facts.pop();
    }
    let tail = if facts.is_empty() {
        String::new()
    } else {
        format!("  {}", style.dim(&facts.join(" \u{00B7} ")))
    };
    format!("{lead}{}{tail}", style.bold(&target))
}

/// The hop rows under their network headers.
pub fn table(run: &Run, style: Style, total_width: usize) -> Vec<String> {
    table_rows(run, style, total_width)
        .into_iter()
        .map(|(_, line)| line)
        .collect()
}

/// The same rows, each with the TTL of the hop it shows. Headers and the closing line have none.
pub fn table_rows(run: &Run, style: Style, total_width: usize) -> Vec<(Option<u8>, String)> {
    let total_width = total_width.clamp(40, 160);
    let queries = usize::from(run.queries);
    // ttl (3) + space + rail + space, then the host column, then the probe cells and " ms".
    let lead = 6;
    let mut cells = queries * CELL + 3;
    let compact = total_width < lead + 22 + cells;
    let narrow = compact && total_width < 58;
    if compact {
        cells = if narrow { 11 } else { 26 };
    }
    let show_bar = !compact && total_width >= lead + 44 + cells + BAR + 2;
    let host_width = total_width
        .saturating_sub(lead + cells + if show_bar { BAR + 2 } else { 0 })
        .clamp(12, 64);
    let slowest = run
        .path()
        .iter()
        .filter_map(|hop| hop.stats().mean)
        .max()
        .unwrap_or(Duration::ZERO);

    let mut out = Vec::new();
    for group in groups(run) {
        if let Some(network) = group.network {
            let mut header = format!(
                "{} {}",
                style.bold(&network.label()),
                truncate_end(&network.name, total_width.saturating_sub(lead + 12))
            );
            if let Some(location) = network.location() {
                if visible_width(&header) + width(&location) + lead + 3 <= total_width {
                    header.push_str(&style.dim(&format!(" \u{00B7} {location}")));
                }
            }
            out.push((None, format!("{}{header}", " ".repeat(lead))));
        }
        for hop in group.hops {
            out.push((
                Some(hop.ttl),
                hop_row(
                    run,
                    hop,
                    style,
                    host_width,
                    compact,
                    narrow,
                    show_bar.then_some(slowest),
                ),
            ));
        }
    }

    // What lies behind the farthest hop that answered.
    let answered = run.path_len();
    let sent = run.hops.len();
    if run.is_running() {
        if sent > answered || run.hops.is_empty() {
            out.push((
                None,
                style.dim(&format!("{:>3} \u{2502} \u{2026}", answered + 1)),
            ));
        }
    } else if run.outcome == Some(Outcome::Exhausted) && sent > answered {
        let range = if sent == answered + 1 {
            format!("{sent}")
        } else {
            format!("{}\u{2013}{sent}", answered + 1)
        };
        out.push((None, style.dim(&format!("{range:>3} \u{2502} no reply"))));
    }
    out
}

fn hop_row(
    run: &Run,
    hop: &Hop,
    style: Style,
    host_width: usize,
    compact: bool,
    narrow: bool,
    bar_scale: Option<Duration>,
) -> String {
    let ttl = style.dim(&format!("{:>3}", hop.ttl));
    let rail = if hop.is_destination {
        style.bold_green("\u{25CF}")
    } else {
        style.dim("\u{2502}")
    };
    let Some(address) = hop.address() else {
        let word = if hop.is_settled() || !run.is_running() {
            "no reply"
        } else {
            "\u{2026}"
        };
        return format!("{ttl} {rail} {}", style.dim(word));
    };

    // The host: its name, then the address dimmed. Without a name the address stands alone.
    let others = hop.addresses().len() - 1;
    let more = if others > 0 {
        format!(" +{others}")
    } else {
        String::new()
    };
    let text_address = address.to_string();
    let host = match run.name(address) {
        Some(name) => {
            let room = host_width.saturating_sub(width(&text_address) + width(&more) + 1);
            if room >= 8 {
                let name = truncate_middle(name, room);
                let shown = if hop.is_destination {
                    style.bold(&name)
                } else {
                    name
                };
                format!("{shown} {}{}", style.dim(&text_address), style.dim(&more))
            } else {
                let name = truncate_middle(name, host_width.saturating_sub(width(&more)));
                format!("{name}{}", style.dim(&more))
            }
        }
        None => {
            let shown = truncate_middle(&text_address, host_width.saturating_sub(width(&more)));
            let shown = if hop.is_destination {
                style.bold(&shown)
            } else {
                shown
            };
            format!("{shown}{}", style.dim(&more))
        }
    };
    let padding = " ".repeat(host_width.saturating_sub(visible_width(&host)));

    let mut row = format!("{ttl} {rail} {host}{padding}");
    if compact {
        let stats = hop.stats();
        let figures = match (stats.mean, stats.min, stats.max) {
            (Some(mean), _, _) if narrow => format!("{:>9}", format::ms(mean)),
            (Some(mean), Some(min), Some(max)) => format!(
                "{:>9}  {}",
                format::ms(mean),
                style.dim(&format!(
                    "{}\u{2013}{}",
                    format::ms_bare(min),
                    format::ms_bare(max)
                ))
            ),
            _ => String::new(),
        };
        row.push_str(&format!("  {figures}"));
    } else {
        for slot in &hop.probes {
            let cell = match slot {
                Slot::Reply { micros, .. } => style.latency(
                    Duration::from_micros(*micros),
                    &format!("{:>CELL$.1}", *micros as f64 / 1000.0),
                ),
                Slot::Lost | Slot::Refused { .. } => style.dim(&format!("{:>CELL$}", "*")),
                Slot::Pending if run.is_running() => style.dim(&format!("{:>CELL$}", "\u{00B7}")),
                Slot::Pending => " ".repeat(CELL),
            };
            row.push_str(&cell);
        }
        row.push_str(&style.dim(" ms"));
    }
    if let Some(code) = hop.unreachable_code() {
        let note = unreachable_note(code, address.is_ipv6());
        if !note.is_empty() {
            row.push_str(&format!(" {}", style.yellow(&note)));
        }
    }
    if let (Some(scale), Some(mean)) = (bar_scale, hop.stats().mean) {
        row.push_str("  ");
        row.push_str(&style.latency(mean, &bar(mean, scale, BAR)));
    }
    row
}

/// A horizontal bar in eighths of a cell, so small differences between hops still show.
pub fn bar(value: Duration, scale: Duration, cells: usize) -> String {
    const PARTIAL: [char; 8] = [
        ' ', '\u{258F}', '\u{258E}', '\u{258D}', '\u{258C}', '\u{258B}', '\u{258A}', '\u{2589}',
    ];
    if scale.is_zero() {
        return String::new();
    }
    let fraction = (value.as_secs_f64() / scale.as_secs_f64()).clamp(0.0, 1.0);
    let eighths = ((fraction * cells as f64 * 8.0).round() as usize).max(1);
    let mut out = "\u{2588}".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        out.push(PARTIAL[eighths % 8]);
    }
    out
}

/// The status while it runs; the outcome, the facts and any interpretation once it has ended.
pub fn footer(view: &View) -> Vec<String> {
    let run = view.run;
    let style = view.style;
    let spinner = SPINNER[view.frame % SPINNER.len()];
    let Some(outcome) = &run.outcome else {
        let count = run.path_len();
        let progress = match (run.destination, count) {
            (None, _) => "Resolving".to_string(),
            (Some(_), 0) => "Running".to_string(),
            (Some(_), 1) => "Running \u{00B7} 1 hop so far".to_string(),
            (Some(_), count) => format!("Running \u{00B7} {count} hops so far"),
        };
        return vec![format!("{} {}", style.blue(spinner), style.dim(&progress))];
    };

    let mut facts: Vec<String> = Vec::new();
    let destination = run
        .destination
        .map_or_else(|| run.target.clone(), |address| address.to_string());
    let count = run.path_len();
    let hops = if count == 1 {
        "1 hop".to_string()
    } else {
        format!("{count} hops")
    };
    let lead = match outcome {
        Outcome::Reached => {
            facts.push(hops);
            if let Some(rtt) = run.total_rtt() {
                facts.push(format::ms(rtt));
            }
            if let Some((port, state)) = run
                .tcp
                .as_ref()
                .and_then(|tcp| Some((tcp.port, tcp.state?)))
            {
                facts.push(format!("port {port} {}", state.word()));
            }
            style.bold_green(&format!("Reached {destination}"))
        }
        Outcome::Exhausted => match run.unreachable() {
            Some((ttl, _)) => style.yellow(&format!("Hop {ttl} reports {destination} unreachable")),
            None => match &run.tcp {
                Some(tcp) => style.yellow(&format!(
                    "No reply from {destination} on port {} after {hops}",
                    tcp.port
                )),
                None => style.yellow(&format!("No reply from {destination} after {hops}")),
            },
        },
        Outcome::Stopped => style.bold(&format!("Stopped after {hops}")),
        Outcome::Failed(message) => style.red(message),
    };
    if *outcome != Outcome::Stopped && !matches!(outcome, Outcome::Failed(_)) {
        match run.network_count() {
            0 => {}
            1 => facts.push("1 network".to_string()),
            networks => facts.push(format!("{networks} networks")),
        }
    }
    if let Some(elapsed) = view.elapsed {
        facts.push(format!("{:.1} s", elapsed.as_secs_f64()));
    }
    let limit = view.width.clamp(40, 160);
    let mut out = Vec::new();
    let mut line = lead;
    if !facts.is_empty() {
        let joined = facts.join(" \u{00B7} ");
        if visible_width(&line) + 3 + width(&joined) <= limit {
            line.push_str(&style.dim(&format!(" \u{00B7} {joined}")));
        } else {
            // A narrow terminal gets the facts on their own line.
            out.push(std::mem::take(&mut line));
            line = style.dim(&joined);
        }
    }
    if view.looking_up && visible_width(&line) + 24 <= limit {
        line.push_str(&format!(
            "  {} {}",
            style.blue(spinner),
            style.dim("looking up networks")
        ));
    }
    out.push(line);
    for sentence in Report::of(run).interpretation {
        out.extend(wrap(&sentence, view.width.clamp(40, 100)));
    }
    out
}

/// Breaks a sentence at spaces so no line passes `max` columns.
pub fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if !current.is_empty() && width(&current) + 1 + width(word) > max {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rvrs_core::engine::{Ending, Event, ReplyKind};
    use rvrs_core::model::{Network, Tool};
    use rvrs_core::FamilyChoice;

    fn reply(ttl: u8, round: u8, from: &str, micros: u64, kind: ReplyKind) -> Event {
        Event::Reply {
            ttl,
            round,
            from: from.parse().unwrap(),
            rtt: Duration::from_micros(micros),
            kind,
        }
    }

    pub fn sample() -> Run {
        let mut run = Run::new(
            Tool::Traceroute,
            "one.one.one.one",
            FamilyChoice::Auto,
            3,
            30,
        );
        run.destination = Some("1.1.1.1".parse().unwrap());
        for round in 0..3u8 {
            run.apply(&reply(
                1,
                round,
                "192.168.178.1",
                800 + u64::from(round) * 100,
                ReplyKind::Intermediate,
            ));
            run.apply(&Event::Lost { ttl: 2, round });
            run.apply(&reply(
                3,
                round,
                "198.51.100.7",
                11_200,
                ReplyKind::Intermediate,
            ));
            run.apply(&reply(4, round, "1.1.1.1", 12_400, ReplyKind::Destination));
        }
        run.apply(&Event::Finished(Ending::Reached(4)));
        run.names
            .insert("192.168.178.1".parse().unwrap(), "fritz.box".into());
        run.names
            .insert("1.1.1.1".parse().unwrap(), "one.one.one.one".into());
        run.networks.insert(
            "198.51.100.7".parse().unwrap(),
            Some(Network {
                number: 64500,
                name: "Example Net".into(),
                country: Some("DE".into()),
                city: Some("Frankfurt".into()),
                prefix: None,
            }),
        );
        run.networks.insert(
            "1.1.1.1".parse().unwrap(),
            Some(Network {
                number: 13335,
                name: "Cloudflare, Inc.".into(),
                country: None,
                city: None,
                prefix: None,
            }),
        );
        run
    }

    fn plain(run: &Run, width: usize) -> Vec<String> {
        lines(&View {
            run,
            style: Style::PLAIN,
            width,
            frame: 0,
            elapsed: Some(Duration::from_millis(1240)),
            looking_up: false,
        })
    }

    #[test]
    fn hops_sit_under_their_network() {
        let run = sample();
        let out = plain(&run, 80);
        assert_eq!(
            out[0],
            "// traceroute one.one.one.one  1.1.1.1 \u{00B7} IPv4 \u{00B7} this device"
        );
        assert_eq!(out[2], "      AS64500 Example Net \u{00B7} Frankfurt, DE");
        assert!(out[3].starts_with("  1 \u{2502} fritz.box 192.168.178.1"));
        assert!(out[3].ends_with("    0.8    0.9    1.0 ms"));
        assert_eq!(out[4], "  2 \u{2502} no reply");
        assert_eq!(out[6], "      AS13335 Cloudflare, Inc.");
        assert!(out[7].starts_with("  4 \u{25CF} one.one.one.one 1.1.1.1"));
        assert_eq!(
            out[9],
            "Reached 1.1.1.1 \u{00B7} 4 hops \u{00B7} 12.4 ms \u{00B7} 2 networks \u{00B7} 1.2 s"
        );
    }

    #[test]
    fn every_row_fits_the_terminal() {
        let run = sample();
        for width in [40, 60, 80, 120, 200] {
            for line in plain(&run, width) {
                assert!(
                    visible_width(&line) <= width.clamp(40, 160),
                    "{width}: {line}"
                );
            }
        }
    }

    #[test]
    fn wide_terminals_get_a_latency_bar() {
        let run = sample();
        assert!(plain(&run, 130)[7].contains('\u{2588}'));
        assert!(!plain(&run, 80)[7].contains('\u{2588}'));
    }

    #[test]
    fn a_silent_target_says_what_it_can_and_cant_mean() {
        let mut run = Run::new(Tool::Traceroute, "192.0.2.1", FamilyChoice::Auto, 1, 8);
        run.destination = Some("192.0.2.1".parse().unwrap());
        for ttl in 1..=8 {
            run.apply(&Event::Sent { ttl, round: 0 });
        }
        run.apply(&reply(1, 0, "10.0.0.1", 900, ReplyKind::Intermediate));
        run.apply(&Event::Finished(Ending::Exhausted));
        let out = plain(&run, 80);
        assert_eq!(out[3], "2\u{2013}8 \u{2502} no reply");
        assert_eq!(out[5], "No reply from 192.0.2.1 after 1 hop \u{00B7} 1.2 s");
        assert_eq!(out[6..].join(" "), "The target sent no reply. It may drop ICMP, so a silent target isn't necessarily down.");
    }

    #[test]
    fn a_live_run_shows_what_is_still_out() {
        let mut run = Run::new(Tool::Traceroute, "example.com", FamilyChoice::Auto, 3, 30);
        run.destination = Some("93.184.216.34".parse().unwrap());
        run.apply(&Event::Sent { ttl: 1, round: 0 });
        run.apply(&Event::Sent { ttl: 2, round: 0 });
        run.apply(&reply(1, 0, "10.0.0.1", 900, ReplyKind::Intermediate));
        let view = View {
            run: &run,
            style: Style::PLAIN,
            width: 80,
            frame: 0,
            elapsed: None,
            looking_up: false,
        };
        let out = lines(&view);
        assert!(out[2].ends_with("    0.9      \u{00B7}      \u{00B7} ms"));
        assert_eq!(out[3], "  2 \u{2502} \u{2026}");
        assert!(out[5].ends_with("Running \u{00B7} 1 hop so far"));
    }

    #[test]
    fn bars_scale_in_eighths() {
        assert_eq!(
            bar(Duration::from_millis(10), Duration::from_millis(10), 4),
            "\u{2588}\u{2588}\u{2588}\u{2588}"
        );
        assert_eq!(
            bar(Duration::from_millis(5), Duration::from_millis(10), 4),
            "\u{2588}\u{2588}"
        );
        assert_eq!(
            bar(Duration::from_micros(10), Duration::from_millis(10), 4),
            "\u{258F}"
        );
    }

    #[test]
    fn sentences_wrap_at_spaces() {
        assert_eq!(
            wrap("one two three four", 9),
            vec!["one two", "three", "four"]
        );
    }
}
