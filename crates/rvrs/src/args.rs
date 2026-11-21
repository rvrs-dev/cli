//! `rvrs traceroute` takes the flags of the traceroute it replaces. This is a hand-written
//! parser because the promise is exact: `-q3`, `-q 3`, `--queries=3`, a trailing packet length,
//! and a plain error for every flag rvrs can't honor yet. A flag is never silently ignored when
//! ignoring it would change the result.

use std::time::Duration;

use rvrs_core::engine::Options;
use rvrs_core::FamilyChoice;

use crate::style::ColorChoice;

#[derive(Debug, Clone, PartialEq)]
pub struct TraceArgs {
    pub target: String,
    pub options: Options,
    pub family: FamilyChoice,
    /// `-n`: addresses only, no reverse DNS.
    pub numeric: bool,
    /// `-A`: show each hop's AS number in the classic output.
    pub as_lookups: bool,
    /// `--no-lookups`: no reverse DNS and no network lookups.
    pub no_lookups: bool,
    /// The total packet length the classic header reports.
    pub packet_len: usize,
    /// `-T`: probe with TCP toward this port (`-p`, 80 when it isn't given). None is ICMP.
    pub tcp: Option<u16>,
    pub common: Common,
}

/// rvrs's own flags. Long only, so they never collide with a tool's short flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Common {
    pub json: bool,
    pub classic: bool,
    pub color: ColorChoice,
    pub node: Option<String>,
    /// `--fleet`: run from every node in the fleet. Refused until nodes exist.
    pub fleet: bool,
    /// `--case <name>`: save the run into this case once it is made.
    pub case: Option<String>,
}

impl Default for Common {
    fn default() -> Self {
        Self {
            json: false,
            classic: false,
            color: ColorChoice::Auto,
            node: None,
            fleet: false,
            case: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Run(Box<TraceArgs>),
    Help,
    Version,
}

/// A case name from `--case`: trimmed, never blank.
pub fn case_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("--case takes the name of a case, such as --case vpn-drop".into());
    }
    Ok(name.to_string())
}

/// Flags that take a value, by short name.
const VALUED: &str = "mqwfzNpsitgPMl";

fn long_to_short(name: &str) -> Option<char> {
    Some(match name {
        "max-hops" => 'm',
        "queries" => 'q',
        "wait" => 'w',
        "first" => 'f',
        "sendwait" => 'z',
        "sim-queries" => 'N',
        "port" => 'p',
        "source" => 's',
        "interface" => 'i',
        "tos" => 't',
        "gateway" => 'g',
        "protocol" => 'P',
        "module" => 'M',
        "flowlabel" => 'l',
        "icmp" => 'I',
        "tcp" => 'T',
        "udp" => 'U',
        "as-path-lookups" => 'A',
        "dont-fragment" => 'F',
        "debug" => 'd',
        "extensions" => 'e',
        _ => return None,
    })
}

fn number<T: std::str::FromStr>(flag: &str, value: &str, what: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} takes {what}, and \u{201C}{value}\u{201D} isn't one."))
}

pub fn parse(arguments: &[String]) -> Result<Parsed, String> {
    let mut options = Options::default();
    let mut family = FamilyChoice::Auto;
    let mut numeric = false;
    let mut as_lookups = false;
    let mut no_lookups = false;
    let mut common = Common::default();
    let mut positional: Vec<&str> = Vec::new();
    let tcp = std::cell::Cell::new(false);
    let port = std::cell::Cell::new(None::<u16>);

    let mut apply = |flag: char,
                     shown: &str,
                     value: Option<&str>,
                     options: &mut Options,
                     family: &mut FamilyChoice|
     -> Result<(), String> {
        let value = value.unwrap_or_default();
        match flag {
            'm' => {
                let hops: u32 = number(shown, value, "a number of hops")?;
                if !(1..=255).contains(&hops) {
                    return Err(format!("{shown} takes 1 to 255 hops."));
                }
                options.max_ttl = hops as u8;
            }
            'q' => {
                let queries: u32 = number(shown, value, "a number of probes")?;
                if !(1..=10).contains(&queries) {
                    return Err(format!("{shown} takes 1 to 10 probes per hop."));
                }
                options.queries = queries as u8;
            }
            'w' => {
                // Linux also accepts "max,here,near". rvrs adapts the wait itself, so the first number is the one that counts.
                let first = value.split(',').next().unwrap_or_default();
                let seconds: f64 = number(shown, first, "a number of seconds")?;
                if !(seconds > 0.0 && seconds <= 60.0) {
                    return Err(format!("{shown} takes more than 0 and at most 60 seconds."));
                }
                options.wait = Duration::from_secs_f64(seconds);
            }
            'f' => {
                let first: u32 = number(shown, value, "a hop number")?;
                if !(1..=255).contains(&first) {
                    return Err(format!("{shown} takes a hop number from 1 to 255."));
                }
                options.first_ttl = first as u8;
            }
            'z' => {
                // As Linux reads it: up to 10 is seconds, anything larger is milliseconds.
                let pause: f64 = number(shown, value, "a pause")?;
                if pause < 0.0 {
                    return Err(format!("{shown} can't be negative."));
                }
                options.spacing = if pause <= 10.0 { Duration::from_secs_f64(pause) } else { Duration::from_secs_f64(pause / 1000.0) };
            }
            // rvrs always sends its probes in parallel, so there is nothing to tune.
            'N' => {
                let _: u32 = number(shown, value, "a number of probes")?;
            }
            'I' => {}
            '4' => *family = FamilyChoice::V4,
            '6' => *family = FamilyChoice::V6,
            'n' => numeric = true,
            'A' | 'a' => as_lookups = true,
            'T' => tcp.set(true),
            'U' | 'D' | 'P' | 'M' => {
                return Err(format!(
                    "{shown} asks for a probe type rvrs doesn't send yet. rvrs traceroute sends ICMP echo probes, or TCP with -T."
                ));
            }
            'p' => {
                let number: u32 = number(shown, value, "a port")?;
                if !(1..=65535).contains(&number) {
                    return Err(format!("{shown} takes a port from 1 to 65535."));
                }
                port.set(Some(number as u16));
            }
            's' | 'i' => return Err(format!("{shown} isn't supported yet. rvrs sends from the address and interface the system routes through.")),
            other => return Err(format!("-{other} isn't supported yet. Supported flags: rvrs traceroute --help")),
        }
        Ok(())
    };

    let mut index = 0;
    let mut only_positional = false;
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
                Some((name, value)) => (name, Some(value)),
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
                        None => {
                            index += 1;
                            arguments
                                .get(index - 1)
                                .map(String::as_str)
                                .ok_or("--color takes auto, always or never.")?
                        }
                    };
                    common.color = ColorChoice::parse(value).ok_or_else(|| format!("--color takes auto, always or never, and \u{201C}{value}\u{201D} is none of them."))?;
                }
                "node" => {
                    let value = match inline {
                        Some(value) => value,
                        None => {
                            index += 1;
                            arguments
                                .get(index - 1)
                                .map(String::as_str)
                                .ok_or("--node takes the name of a node.")?
                        }
                    };
                    common.node = Some(value.to_string());
                }
                "case" => {
                    let value = match inline {
                        Some(value) => value,
                        None => {
                            index += 1;
                            arguments
                                .get(index - 1)
                                .map(String::as_str)
                                .ok_or("--case takes the name of a case.")?
                        }
                    };
                    common.case = Some(case_name(value)?);
                }
                other => {
                    let Some(short) = long_to_short(other) else {
                        return Err(format!("rvrs traceroute has no flag --{other}. Supported flags: rvrs traceroute --help"));
                    };
                    let shown = format!("--{other}");
                    let value = if VALUED.contains(short) {
                        match inline {
                            Some(value) => Some(value),
                            None => {
                                index += 1;
                                Some(
                                    arguments
                                        .get(index - 1)
                                        .map(String::as_str)
                                        .ok_or_else(|| format!("{shown} needs a value."))?,
                                )
                            }
                        }
                    } else {
                        None
                    };
                    apply(short, &shown, value, &mut options, &mut family)?;
                }
            }
            continue;
        }
        // A cluster of short flags: -nI, -q3, -q 3.
        let cluster: Vec<char> = argument[1..].chars().collect();
        let mut position = 0;
        while position < cluster.len() {
            let flag = cluster[position];
            position += 1;
            let shown = format!("-{flag}");
            if flag == 'h' {
                return Ok(Parsed::Help);
            }
            if flag == 'V' {
                return Ok(Parsed::Version);
            }
            if VALUED.contains(flag) {
                let rest: String = cluster[position..].iter().collect();
                position = cluster.len();
                let value = if rest.is_empty() {
                    index += 1;
                    arguments
                        .get(index - 1)
                        .cloned()
                        .ok_or_else(|| format!("{shown} needs a value."))?
                } else {
                    rest
                };
                apply(flag, &shown, Some(&value), &mut options, &mut family)?;
            } else {
                apply(flag, &shown, None, &mut options, &mut family)?;
            }
        }
    }

    let Some(target) = positional.first() else {
        return Err("rvrs traceroute needs a target, such as rvrs traceroute 1.1.1.1".to_string());
    };
    if positional.len() > 2 {
        return Err(format!(
            "rvrs traceroute takes one target, and \u{201C}{}\u{201D} is a second.",
            positional[1]
        ));
    }
    // The original's optional packet length: the whole IP packet, headers included.
    let header = |family: FamilyChoice, target: &str| {
        if family == FamilyChoice::V6 || target.contains(':') {
            48
        } else {
            28
        }
    };
    let overhead = header(family, target);
    let mut packet_len = overhead + options.payload;
    if let Some(length) = positional.get(1) {
        let length: usize = number("The packet length", length, "a number of bytes")?;
        if length < overhead || length > 1428 {
            return Err(format!("The packet length takes {overhead} to 1428 bytes."));
        }
        packet_len = length;
        options.payload = length - overhead;
    }
    if options.first_ttl > options.max_ttl {
        return Err(format!(
            "The first hop ({}) is beyond the last ({}).",
            options.first_ttl, options.max_ttl
        ));
    }

    if port.get().is_some() && !tcp.get() {
        return Err(
            "-p sets a port, and ICMP echo probes have none. Add -T to probe with TCP.".to_string(),
        );
    }
    // Linux traceroute's default port for -T.
    let tcp = tcp.get().then(|| port.get().unwrap_or(80));

    Ok(Parsed::Run(Box::new(TraceArgs {
        target: normalize_target(target),
        options,
        family,
        numeric,
        as_lookups,
        no_lookups,
        packet_len,
        tcp,
        common,
    })))
}

/// A pasted URL is reduced to its host, as the app's target field does.
pub fn normalize_target(input: &str) -> String {
    let trimmed = input.trim();
    let without_scheme = trimmed.split_once("://").map_or(trimmed, |(_, rest)| rest);
    if trimmed.contains("://") {
        let host = without_scheme
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(without_scheme);
        let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
        if let Some(bracketed) = host.strip_prefix('[') {
            return bracketed.split(']').next().unwrap_or(bracketed).to_string();
        }
        return host.split(':').next().unwrap_or(host).to_string();
    }
    trimmed
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(line: &str) -> Result<TraceArgs, String> {
        let arguments: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        match parse(&arguments)? {
            Parsed::Run(args) => Ok(*args),
            other => Err(format!("{other:?}")),
        }
    }

    #[test]
    fn defaults_are_the_originals() {
        let args = parse_line("example.com").unwrap();
        assert_eq!(args.options.max_ttl, 30);
        assert_eq!(args.options.queries, 3);
        assert_eq!(args.options.wait, Duration::from_secs(5));
        assert_eq!(args.packet_len, 60);
        assert_eq!(args.family, FamilyChoice::Auto);
    }

    #[test]
    fn short_flags_take_values_attached_or_apart() {
        let args = parse_line("-m 20 -q5 -w 1.5 -f 2 -n example.com").unwrap();
        assert_eq!(
            (
                args.options.max_ttl,
                args.options.queries,
                args.options.first_ttl
            ),
            (20, 5, 2)
        );
        assert_eq!(args.options.wait, Duration::from_millis(1500));
        assert!(args.numeric);
    }

    #[test]
    fn clusters_and_long_forms() {
        let args = parse_line("-nI4 --max-hops=12 --queries 2 example.com").unwrap();
        assert!(args.numeric);
        assert_eq!(args.family, FamilyChoice::V4);
        assert_eq!((args.options.max_ttl, args.options.queries), (12, 2));
    }

    #[test]
    fn flags_may_follow_the_target() {
        let args = parse_line("example.com -m 5 --json").unwrap();
        assert_eq!(args.options.max_ttl, 5);
        assert!(args.common.json);
    }

    #[test]
    fn a_packet_length_sets_the_payload() {
        let args = parse_line("example.com 100").unwrap();
        assert_eq!(args.packet_len, 100);
        assert_eq!(args.options.payload, 72);
        assert!(parse_line("example.com 10").is_err());
    }

    #[test]
    fn linux_wait_triplets_use_their_first_number() {
        assert_eq!(
            parse_line("-w 2,3,10 example.com").unwrap().options.wait,
            Duration::from_secs(2)
        );
    }

    #[test]
    fn sendwait_reads_seconds_up_to_ten_and_milliseconds_beyond() {
        assert_eq!(
            parse_line("-z 0.5 example.com").unwrap().options.spacing,
            Duration::from_millis(500)
        );
        assert_eq!(
            parse_line("-z 50 example.com").unwrap().options.spacing,
            Duration::from_millis(50)
        );
    }

    #[test]
    fn probe_types_rvrs_cant_send_are_errors() {
        for flag in ["-U", "--udp", "-p 80", "-s 10.0.0.1", "-i en0"] {
            assert!(
                parse_line(&format!("{flag} example.com")).is_err(),
                "{flag}"
            );
        }
    }

    #[test]
    fn tcp_probes_take_a_port_and_default_to_80() {
        assert_eq!(parse_line("example.com").unwrap().tcp, None);
        assert_eq!(parse_line("-T example.com").unwrap().tcp, Some(80));
        assert_eq!(
            parse_line("--tcp -p 443 example.com").unwrap().tcp,
            Some(443)
        );
        assert_eq!(
            parse_line("-T --port=22 example.com").unwrap().tcp,
            Some(22)
        );
        // A port without -T has nothing to apply to, and the error says what to add.
        let error = parse_line("-p 443 example.com").unwrap_err();
        assert!(error.contains("-T"), "{error}");
        assert!(parse_line("-T -p 0 example.com").is_err());
        assert!(parse_line("-T -p 70000 example.com").is_err());
    }

    #[test]
    fn errors_name_the_flag_and_the_rule() {
        assert_eq!(
            parse_line("-q 0 example.com").unwrap_err(),
            "-q takes 1 to 10 probes per hop."
        );
        assert_eq!(
            parse_line("-m x example.com").unwrap_err(),
            "-m takes a number of hops, and \u{201C}x\u{201D} isn't one."
        );
        assert_eq!(parse_line("-m").unwrap_err(), "-m needs a value.");
        assert!(parse_line("").unwrap_err().contains("needs a target"));
    }

    #[test]
    fn rvrs_flags_are_long_only() {
        let args = parse_line("--classic --color=never --node fra-home-1 --no-lookups example.com")
            .unwrap();
        assert!(args.common.classic && args.no_lookups);
        assert_eq!(args.common.color, ColorChoice::Never);
        assert_eq!(args.common.node.as_deref(), Some("fra-home-1"));
        let args = parse_line("--case vpn-drop example.com").unwrap();
        assert_eq!(args.common.case.as_deref(), Some("vpn-drop"));
        let args = parse_line("--case=home example.com").unwrap();
        assert_eq!(args.common.case.as_deref(), Some("home"));
        assert!(parse_line("--case= example.com").is_err());
    }

    #[test]
    fn help_and_version() {
        assert_eq!(parse(&["--help".to_string()]), Ok(Parsed::Help));
        assert_eq!(parse(&["-V".to_string()]), Ok(Parsed::Version));
    }

    #[test]
    fn a_pasted_url_is_reduced_to_its_host() {
        assert_eq!(
            normalize_target("https://example.com/path?q=1"),
            "example.com"
        );
        assert_eq!(
            normalize_target("https://user@example.com:8443/"),
            "example.com"
        );
        assert_eq!(
            normalize_target("[2606:4700:4700::1111]"),
            "2606:4700:4700::1111"
        );
        assert_eq!(normalize_target("http://[::1]:8080/x"), "::1");
        assert_eq!(normalize_target(" 1.1.1.1 "), "1.1.1.1");
    }
}
