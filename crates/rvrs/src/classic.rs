//! The output of the traceroute rvrs replaces, line for line in shape: what a pipe, `--classic`
//! and a `traceroute` symlink get. Modeled on the Linux traceroute.

use std::net::IpAddr;

use rvrs_core::model::{Hop, Run, Slot};

#[derive(Debug, Clone, Copy)]
pub struct ClassicOptions {
    pub numeric: bool,
    pub as_lookups: bool,
    pub packet_len: usize,
}

/// "traceroute to example.com (93.184.216.34), 30 hops max, 60 byte packets"
pub fn header(run: &Run, options: ClassicOptions) -> String {
    let address = run
        .destination
        .map_or_else(|| run.target.clone(), |address| address.to_string());
    format!(
        "traceroute to {} ({address}), {} hops max, {} byte packets",
        run.target, run.max_ttl, options.packet_len
    )
}

/// " 3  ae1.example.net (203.0.113.30)  11.212 ms  11.004 ms  11.398 ms"
pub fn hop_line(run: &Run, hop: &Hop, options: ClassicOptions) -> String {
    let mut line = format!("{:>2} ", hop.ttl);
    let mut previous: Option<IpAddr> = None;
    for slot in &hop.probes {
        match slot {
            Slot::Reply {
                from,
                micros,
                unreachable,
                ..
            } => {
                if previous != Some(*from) {
                    line.push(' ');
                    line.push_str(&host(run, *from, options));
                    previous = Some(*from);
                }
                line.push_str(&format!("  {:.3} ms", *micros as f64 / 1000.0));
                if let Some(code) = unreachable {
                    let note = unreachable_note(*code, from.is_ipv6());
                    if !note.is_empty() {
                        line.push(' ');
                        line.push_str(&note);
                    }
                }
            }
            Slot::Lost => line.push_str(" *"),
            Slot::Pending | Slot::Refused { .. } => {}
        }
    }
    line
}

fn host(run: &Run, address: IpAddr, options: ClassicOptions) -> String {
    let mut text = match run.name(address) {
        Some(name) if !options.numeric => format!("{name} ({address})"),
        _ if options.numeric => address.to_string(),
        _ => format!("{address} ({address})"),
    };
    if options.as_lookups {
        match run.network(address) {
            Some(network) => text.push_str(&format!(" [{}]", network.label())),
            None => text.push_str(" [*]"),
        }
    }
    text
}

/// The original's annotation for an unreachable reply: !H host, !N network, !P protocol,
/// !X prohibited, and the bare code for the rest.
pub fn unreachable_note(code: u8, v6: bool) -> String {
    if v6 {
        return match code {
            0 => "!N".into(),
            1 => "!X".into(),
            3 => "!H".into(),
            4 => String::new(),
            other => format!("!<{other}>"),
        };
    }
    match code {
        0 => "!N".into(),
        1 => "!H".into(),
        2 => "!P".into(),
        3 => String::new(),
        4 => "!F".into(),
        5 => "!S".into(),
        13 => "!X".into(),
        14 => "!V".into(),
        15 => "!C".into(),
        other => format!("!<{other}>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvrs_core::engine::{Ending, Event, ReplyKind};
    use rvrs_core::model::{Network, Tool};
    use rvrs_core::FamilyChoice;
    use std::time::Duration;

    const OPTIONS: ClassicOptions = ClassicOptions {
        numeric: false,
        as_lookups: false,
        packet_len: 60,
    };

    fn reply(ttl: u8, round: u8, from: &str, micros: u64, kind: ReplyKind) -> Event {
        Event::Reply {
            ttl,
            round,
            from: from.parse().unwrap(),
            rtt: Duration::from_micros(micros),
            kind,
        }
    }

    fn sample() -> Run {
        let mut run = Run::new(Tool::Traceroute, "example.com", FamilyChoice::Auto, 3, 30);
        run.destination = Some("93.184.216.34".parse().unwrap());
        for round in 0..3 {
            run.apply(&reply(
                1,
                round,
                "192.168.1.1",
                812 + u64::from(round),
                ReplyKind::Intermediate,
            ));
            run.apply(&Event::Lost { ttl: 2, round });
        }
        run.apply(&reply(
            3,
            0,
            "203.0.113.30",
            11_212,
            ReplyKind::Intermediate,
        ));
        run.apply(&reply(
            3,
            1,
            "203.0.113.39",
            11_004,
            ReplyKind::Intermediate,
        ));
        run.apply(&Event::Lost { ttl: 3, round: 2 });
        run.apply(&reply(
            4,
            0,
            "93.184.216.34",
            12_400,
            ReplyKind::Destination,
        ));
        run.apply(&reply(
            4,
            1,
            "93.184.216.34",
            12_100,
            ReplyKind::Destination,
        ));
        run.apply(&reply(
            4,
            2,
            "93.184.216.34",
            12_600,
            ReplyKind::Destination,
        ));
        run.apply(&Event::Finished(Ending::Reached(4)));
        run.names
            .insert("192.168.1.1".parse().unwrap(), "fritz.box".into());
        run
    }

    #[test]
    fn the_header_is_the_originals() {
        assert_eq!(
            header(&sample(), OPTIONS),
            "traceroute to example.com (93.184.216.34), 30 hops max, 60 byte packets"
        );
    }

    #[test]
    fn hop_lines_keep_the_originals_shape() {
        let run = sample();
        assert_eq!(
            hop_line(&run, &run.hops[0], OPTIONS),
            " 1  fritz.box (192.168.1.1)  0.812 ms  0.813 ms  0.814 ms"
        );
        assert_eq!(hop_line(&run, &run.hops[1], OPTIONS), " 2  * * *");
        assert_eq!(
            hop_line(&run, &run.hops[2], OPTIONS),
            " 3  203.0.113.30 (203.0.113.30)  11.212 ms 203.0.113.39 (203.0.113.39)  11.004 ms *"
        );
        assert_eq!(
            hop_line(&run, &run.hops[3], OPTIONS),
            " 4  93.184.216.34 (93.184.216.34)  12.400 ms  12.100 ms  12.600 ms"
        );
    }

    #[test]
    fn numeric_drops_the_names() {
        let run = sample();
        let options = ClassicOptions {
            numeric: true,
            ..OPTIONS
        };
        assert_eq!(
            hop_line(&run, &run.hops[0], options),
            " 1  192.168.1.1  0.812 ms  0.813 ms  0.814 ms"
        );
    }

    #[test]
    fn as_lookups_follow_the_address() {
        let mut run = sample();
        run.networks.insert(
            "93.184.216.34".parse().unwrap(),
            Some(Network {
                number: 15133,
                name: "Edgecast".into(),
                country: None,
                city: None,
                prefix: None,
            }),
        );
        let options = ClassicOptions {
            numeric: true,
            as_lookups: true,
            ..OPTIONS
        };
        assert!(hop_line(&run, &run.hops[3], options)
            .starts_with(" 4  93.184.216.34 [AS15133]  12.400 ms"));
        assert!(hop_line(&run, &run.hops[0], options).starts_with(" 1  192.168.1.1 [*]"));
    }

    #[test]
    fn unreachable_replies_carry_the_originals_marks() {
        let mut run = Run::new(Tool::Traceroute, "example.com", FamilyChoice::Auto, 1, 30);
        run.apply(&reply(1, 0, "10.0.0.1", 1000, ReplyKind::Unreachable(1)));
        assert_eq!(
            hop_line(
                &run,
                &run.hops[0],
                ClassicOptions {
                    numeric: true,
                    ..OPTIONS
                }
            ),
            " 1  10.0.0.1  1.000 ms !H"
        );
        assert_eq!(unreachable_note(13, false), "!X");
        assert_eq!(unreachable_note(3, false), "");
        assert_eq!(unreachable_note(9, false), "!<9>");
    }
}
