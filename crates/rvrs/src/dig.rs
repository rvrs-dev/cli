//! `rvrs dig`: a name alone gets the lookup the app runs, with every source compared, the
//! delegation walked and the verdict on top. Anything dig-specific on the command line (a
//! server, a type, a `+option`), a pipe or the `dig` name gets one question and dig's own output.

use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use rvrs_core::dns::client::{self, ClientError, Question, Reply};
use rvrs_core::dns::lookup::device_resolvers;
use rvrs_core::dns::message::{rcode_name, reverse_name, Presentation, QueryFlags, Response};
use rvrs_core::dns::result::{changes, SourceStatus, VerdictGrade};
use rvrs_core::dns::{
    type_code, type_name, types, Diff, DnsResult, Options, Record, Resolver, Section, SourceResult,
};
use rvrs_core::format;
use rvrs_core::history::History;
use rvrs_core::model::{Outcome, Run};
use rvrs_core::report::Report;
use rvrs_core::resolve;
use rvrs_core::session::{Lookups, Session};
use rvrs_core::FamilyChoice;

use crate::args::{normalize_target, Common};
use crate::pretty;
use crate::style::{truncate_end, truncate_middle, visible_width, ColorChoice, Style, SPINNER};
use crate::term::{self, LiveRegion};
use crate::trace::{self, Look};
use crate::EXIT_USAGE;

/// dig's code for "no server answered".
const EXIT_NO_REPLY: u8 = 9;

/// Which parts of an answer dig prints, switched by `+noall`, `+answer` and the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
    pub cmd: bool,
    pub comments: bool,
    pub question: bool,
    pub answer: bool,
    pub authority: bool,
    pub additional: bool,
    pub stats: bool,
}

impl Shown {
    const ALL: Shown = Shown {
        cmd: true,
        comments: true,
        question: true,
        answer: true,
        authority: true,
        additional: true,
        stats: true,
    };
    const NONE: Shown = Shown {
        cmd: false,
        comments: false,
        question: false,
        answer: false,
        authority: false,
        additional: false,
        stats: false,
    };
}

#[derive(Debug, Clone, PartialEq)]
pub struct DigArgs {
    /// The name as typed. Empty asks for the root, as dig does.
    pub name: String,
    pub type_code: Option<u16>,
    /// `@server`: an address or a name.
    pub server: Option<String>,
    pub port: u16,
    pub family: FamilyChoice,
    pub short: bool,
    pub trace: bool,
    pub tcp: bool,
    pub dnssec: bool,
    pub recursive: bool,
    pub edns: bool,
    pub shown: Shown,
    pub timeout: Duration,
    pub tries: u32,
    /// `--resolver`: resolvers of the person's own, asked beside the public ones in the lookup.
    pub resolvers: Vec<IpAddr>,
    pub no_lookups: bool,
    /// The command line asks one question the way dig does.
    pub one_question: bool,
    /// The arguments as typed, for dig's first line.
    pub line: String,
    pub common: Common,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Run(Box<DigArgs>),
    Help,
    Version,
}

pub fn parse(arguments: &[String], multicall: bool) -> Result<Parsed, String> {
    let mut args = DigArgs {
        name: String::new(),
        type_code: None,
        server: None,
        port: 53,
        family: FamilyChoice::Auto,
        short: false,
        trace: false,
        tcp: false,
        dnssec: false,
        recursive: true,
        edns: true,
        shown: Shown::ALL,
        timeout: Duration::from_secs(5),
        tries: 3,
        resolvers: Vec::new(),
        no_lookups: false,
        one_question: multicall,
        line: arguments.join(" "),
        common: Common::default(),
    };
    let mut name: Option<String> = None;
    let mut reverse = false;

    let mut index = 0;
    let value_after = |index: &mut usize, flag: &str| -> Result<String, String> {
        *index += 1;
        arguments
            .get(*index - 1)
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value."))
    };
    let known_type = |flag: &str, value: &str| {
        type_code(value).ok_or_else(|| {
            format!("{flag} takes a record type such as A or MX, and \u{201C}{value}\u{201D} isn't one.")
        })
    };
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        index += 1;
        if let Some(server) = argument.strip_prefix('@') {
            if server.is_empty() {
                return Err("@ needs a server, as in @1.1.1.1".to_string());
            }
            args.server = Some(server.to_string());
            args.one_question = true;
            continue;
        }
        if let Some(option) = argument.strip_prefix('+') {
            query_option(&mut args, option)?;
            args.one_question = true;
            continue;
        }
        if let Some(long) = argument.strip_prefix("--") {
            let (flag, inline) = match long.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_string())),
                None => (long, None),
            };
            let shown = format!("--{flag}");
            let value = |index: &mut usize| match inline.clone() {
                Some(value) => Ok(value),
                None => value_after(index, &shown),
            };
            match flag {
                "help" => return Ok(Parsed::Help),
                "version" => return Ok(Parsed::Version),
                "json" => args.common.json = true,
                "classic" => args.common.classic = true,
                "fleet" => args.common.fleet = true,
                "no-lookups" => args.no_lookups = true,
                "color" => {
                    let value = value(&mut index)?;
                    args.common.color = ColorChoice::parse(&value).ok_or_else(|| {
                        format!("--color takes auto, always or never, and \u{201C}{value}\u{201D} is none of them.")
                    })?;
                }
                "node" => args.common.node = Some(value(&mut index)?),
                "case" => args.common.case = Some(crate::args::case_name(&value(&mut index)?)?),
                "resolver" => {
                    let value = value(&mut index)?;
                    let address = value.parse().map_err(|_| {
                        format!("--resolver takes an address such as 10.0.0.53, and \u{201C}{value}\u{201D} isn't one.")
                    })?;
                    args.resolvers.push(address);
                }
                other => {
                    return Err(format!(
                        "rvrs dig has no flag --{other}. Supported flags: rvrs dig --help"
                    ))
                }
            }
            continue;
        }
        if argument.len() > 1 && argument.starts_with('-') {
            let flag = &argument[..2];
            let attached = &argument[2..];
            let value = |index: &mut usize| {
                if attached.is_empty() {
                    value_after(index, flag)
                } else {
                    Ok(attached.to_string())
                }
            };
            match flag {
                "-h" => return Ok(Parsed::Help),
                "-v" | "-V" => return Ok(Parsed::Version),
                "-4" => args.family = FamilyChoice::V4,
                "-6" => args.family = FamilyChoice::V6,
                "-t" => args.type_code = Some(known_type("-t", &value(&mut index)?)?),
                "-q" => name = Some(value(&mut index)?),
                "-x" => {
                    let value = value(&mut index)?;
                    let address: IpAddr = value.parse().map_err(|_| {
                        format!("-x takes an address, and \u{201C}{value}\u{201D} isn't one.")
                    })?;
                    name = Some(reverse_name(address));
                    reverse = true;
                }
                "-p" => {
                    let value = value(&mut index)?;
                    args.port = value.parse().map_err(|_| {
                        format!("-p takes a port, and \u{201C}{value}\u{201D} isn't one.")
                    })?;
                }
                "-c" => {
                    let value = value(&mut index)?;
                    if !value.eq_ignore_ascii_case("IN") {
                        return Err(format!(
                            "rvrs dig asks in class IN only, and -c names {value}."
                        ));
                    }
                }
                other => {
                    return Err(format!(
                        "{other} isn't supported yet. Supported flags: rvrs dig --help"
                    ))
                }
            }
            args.one_question = true;
            continue;
        }
        // dig reads a word that names a record type as the type, wherever it stands.
        if let Some(code) = type_code(argument).filter(|_| !argument.contains('.')) {
            args.type_code = Some(code);
            args.one_question = true;
        } else if argument.eq_ignore_ascii_case("IN") && name.is_some() {
            args.one_question = true;
        } else if let Some(first) = &name {
            return Err(format!(
                "rvrs dig takes one name, and \u{201C}{argument}\u{201D} comes after {first}."
            ));
        } else {
            name = Some(argument.to_string());
        }
    }

    if reverse && args.type_code.is_none() {
        args.type_code = Some(types::PTR);
    }
    if args.common.json && args.one_question && !multicall {
        return Err(
            "--json prints the whole lookup, so it takes a name and nothing of dig's.".to_string(),
        );
    }
    match name {
        Some(name) if args.one_question => args.name = name.trim_end_matches('.').to_string(),
        Some(name) => args.name = rvrs_core::dns::lookup::normalize(&normalize_target(&name)),
        // dig alone asks for the root's nameservers.
        None if args.one_question => {
            args.type_code = args.type_code.or(Some(types::NS));
        }
        None => return Err("rvrs dig needs a name, such as rvrs dig example.com".to_string()),
    }
    Ok(Parsed::Run(Box::new(args)))
}

/// One of dig's `+options`. The ones rvrs doesn't have are named, never ignored.
fn query_option(args: &mut DigArgs, option: &str) -> Result<(), String> {
    let (word, value) = match option.split_once('=') {
        Some((word, value)) => (word, Some(value)),
        None => (option, None),
    };
    let (on, word) = match word.strip_prefix("no") {
        Some(rest) => (false, rest),
        None => (true, word),
    };
    let number = |what: &str| -> Result<u32, String> {
        value
            .and_then(|value| value.parse().ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("+{word} takes {what}, as in +{word}=2"))
    };
    match word {
        "short" => args.short = on,
        "trace" => args.trace = on,
        "tcp" | "vc" => args.tcp = on,
        "dnssec" | "do" => args.dnssec = on,
        "recurse" | "rec" | "recursive" => args.recursive = on,
        "edns" => args.edns = on,
        "all" => args.shown = if on { Shown::ALL } else { Shown::NONE },
        "cmd" => args.shown.cmd = on,
        "comments" => args.shown.comments = on,
        "question" => args.shown.question = on,
        "answer" => args.shown.answer = on,
        "authority" => args.shown.authority = on,
        "additional" => args.shown.additional = on,
        "stats" => args.shown.stats = on,
        "time" | "timeout" => {
            args.timeout = Duration::from_secs(u64::from(number("a number of seconds")?))
        }
        "tries" => args.tries = number("a number of tries")?,
        "retry" => args.tries = number("a number of retries")? + 1,
        // rvrs sends no cookie, so switching it off changes nothing.
        "cookie" if !on => {}
        _ => {
            return Err(format!(
                "+{option} isn't supported yet. Supported options: rvrs dig --help"
            ))
        }
    }
    Ok(())
}

pub fn run(args: DigArgs, multicall: bool) -> ExitCode {
    if args.common.fleet {
        return crate::usage(trace::NO_FLEET);
    }
    if let Some(node) = &args.common.node {
        return match crate::remote::dig(&args) {
            Ok(options) => crate::remote::run(node, "dns", &args.name, options, &args.common),
            Err(message) => crate::usage(&message),
        };
    }
    let look = trace::look(args.common.json, args.common.classic, multicall);
    if args.one_question || look == Look::Classic {
        return if args.trace {
            trace_question(&args)
        } else {
            ask(&args)
        };
    }
    lookup(&args, look)
}

// One question, the way dig asks it

/// The server a question goes to, with the name dig prints in brackets.
fn server_of(args: &DigArgs) -> Result<(SocketAddr, String), String> {
    if let Some(server) = &args.server {
        let address = match server.parse::<IpAddr>() {
            Ok(address) => address,
            Err(_) => resolve::resolve(server, args.family)
                .map_err(|_| format!("Couldn't resolve the server {server}."))?,
        };
        return Ok((SocketAddr::new(address, args.port), server.clone()));
    }
    let wanted = |address: &IpAddr| match args.family {
        FamilyChoice::V4 => address.is_ipv4(),
        FamilyChoice::V6 => address.is_ipv6(),
        FamilyChoice::Auto => true,
    };
    device_resolvers()
        .into_iter()
        .find(wanted)
        .map(|address| (SocketAddr::new(address, args.port), address.to_string()))
        .ok_or_else(|| "No resolver is configured on this device. Name one, as in rvrs dig @1.1.1.1 example.com".to_string())
}

fn question<'a>(
    args: &'a DigArgs,
    name: &'a str,
    type_code: u16,
    server: SocketAddr,
    recursive: bool,
) -> Question<'a> {
    Question {
        name,
        type_code,
        server,
        flags: QueryFlags {
            recursive,
            edns: args.edns || args.dnssec,
            dnssec_ok: args.dnssec,
            authentic_data: true,
        },
        timeout: args.timeout,
        tcp: args.tcp,
        presentation: Presentation::Dig,
    }
}

/// Asks again after a timeout, as often as `+tries` says.
fn send(question: &Question, tries: u32) -> Result<Reply, ClientError> {
    let mut last = ClientError::Timeout(question.server.ip().to_string());
    for _ in 0..tries.max(1) {
        match client::query(question) {
            Ok(reply) => return Ok(reply),
            Err(error @ ClientError::Timeout(_)) => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

fn ask(args: &DigArgs) -> ExitCode {
    let (server, server_name) = match server_of(args) {
        Ok(found) => found,
        Err(message) => return crate::fail(&message),
    };
    let asked = args.type_code.unwrap_or(types::A);
    let mut stdout = std::io::stdout().lock();
    if args.shown.cmd && !args.short {
        for line in command_lines(args) {
            let _ = writeln!(stdout, "{line}");
        }
    }
    let reply = match send(
        &question(args, &args.name, asked, server, args.recursive),
        args.tries,
    ) {
        Ok(reply) => reply,
        Err(ClientError::Timeout(_)) => {
            // dig's own line and code, which scripts look for.
            let _ = writeln!(
                stdout,
                ";; connection timed out; no servers could be reached"
            );
            return ExitCode::from(EXIT_NO_REPLY);
        }
        Err(error) => {
            drop(stdout);
            eprintln!(
                "{}",
                trace::style_error(Style::detect_stderr(), &error.to_string())
            );
            return ExitCode::from(EXIT_NO_REPLY);
        }
    };
    if args.short {
        for record in reply.response.answers() {
            let _ = writeln!(stdout, "{}", record.value);
        }
    } else {
        let stats = Stats {
            millis: reply.rtt.as_millis(),
            server,
            server_name: &server_name,
            when: when(),
            size: reply.size,
        };
        for line in reply_lines(args, &reply.response, Some(&stats)) {
            let _ = writeln!(stdout, "{line}");
        }
    }
    let _ = stdout.flush();
    ExitCode::SUCCESS
}

/// "; <<>> rvrs 0.4 <<>> example.com MX"
fn command_lines(args: &DigArgs) -> Vec<String> {
    let mut lines = vec![
        String::new(),
        format!("; <<>> {} <<>> {}", crate::help::version(), args.line),
    ];
    if args.server.is_some() {
        lines.push("; (1 server found)".to_string());
    }
    lines.push(";; global options: +cmd".to_string());
    lines
}

struct Stats<'a> {
    millis: u128,
    server: SocketAddr,
    server_name: &'a str,
    when: String,
    size: usize,
}

/// A name with its closing dot, as a zone file writes it.
fn dotted(name: &str) -> String {
    if name.ends_with('.') {
        name.to_string()
    } else {
        format!("{name}.")
    }
}

/// dig pads a name with tabs to the column the TTL starts in.
fn padded(name: &str) -> String {
    let tabs = match name.len() {
        0..=7 => 3,
        8..=15 => 2,
        _ => 1,
    };
    format!("{name}{}", "\t".repeat(tabs))
}

/// "example.com. 300 IN A 192.0.2.1", with dig's tabs between the fields.
pub fn record_line(record: &Record) -> String {
    format!(
        "{}{}\tIN\t{}\t{}",
        padded(&dotted(&record.name)),
        record.ttl,
        record.type_name(),
        record.value
    )
}

/// An answer the way dig prints it, section by section.
fn reply_lines(args: &DigArgs, response: &Response, stats: Option<&Stats>) -> Vec<String> {
    let shown = args.shown;
    let mut out = Vec::new();
    if shown.comments {
        out.push(";; Got answer:".to_string());
        out.push(format!(
            ";; ->>HEADER<<- opcode: QUERY, status: {}, id: {}",
            rcode_name(response.rcode),
            response.id
        ));
        let mut flags = vec!["qr"];
        for (set, flag) in [
            (response.is_authoritative(), "aa"),
            (response.is_truncated(), "tc"),
            (response.recursion_desired(), "rd"),
            (response.recursion_available(), "ra"),
            (response.authentic_data(), "ad"),
            (response.checking_disabled(), "cd"),
        ] {
            if set {
                flags.push(flag);
            }
        }
        out.push(format!(
            ";; flags: {}; QUERY: {}, ANSWER: {}, AUTHORITY: {}, ADDITIONAL: {}",
            flags.join(" "),
            response.counts[0],
            response.counts[1],
            response.counts[2],
            response.counts[3]
        ));
        if response.recursion_desired() && !response.recursion_available() {
            out.push(";; WARNING: recursion requested but not available".to_string());
        }
        out.push(String::new());
        if let Some(edns) = &response.edns {
            out.push(";; OPT PSEUDOSECTION:".to_string());
            out.push(format!(
                "; EDNS: version: {}, flags:{}; udp: {}",
                edns.version,
                if edns.dnssec_ok { " do" } else { "" },
                edns.udp_size
            ));
        }
    }
    if shown.question {
        if let Some((name, asked)) = &response.question {
            if shown.comments {
                out.push(";; QUESTION SECTION:".to_string());
            }
            out.push(format!(
                ";{}\tIN\t{}",
                padded(&dotted(name)),
                type_name(*asked)
            ));
            if shown.comments {
                out.push(String::new());
            }
        }
    }
    for (wanted, section, title) in [
        (shown.answer, Section::Answer, "ANSWER"),
        (shown.authority, Section::Authority, "AUTHORITY"),
        (shown.additional, Section::Additional, "ADDITIONAL"),
    ] {
        let records: Vec<&Record> = response.in_section(section).collect();
        if !wanted || records.is_empty() {
            continue;
        }
        if shown.comments {
            out.push(format!(";; {title} SECTION:"));
        }
        out.extend(records.into_iter().map(record_line));
        if shown.comments {
            out.push(String::new());
        }
    }
    if let Some(stats) = stats.filter(|_| shown.stats) {
        out.push(format!(";; Query time: {} msec", stats.millis));
        out.push(format!(
            ";; SERVER: {}#{}({})",
            stats.server.ip(),
            stats.server.port(),
            stats.server_name
        ));
        out.push(format!(";; WHEN: {}", stats.when));
        out.push(format!(";; MSG SIZE  rcvd: {}", stats.size));
        out.push(String::new());
    }
    out
}

/// "Fri Nov 20 10:00:00 CET 2026", in this device's time zone, as dig stamps an answer.
fn when() -> String {
    let now = format::now() as libc::time_t;
    // SAFETY: tm is plain data, and localtime_r fills the one passed to it.
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&now, &mut parts) };
    let mut buffer = [0 as libc::c_char; 64];
    let pattern = b"%a %b %d %H:%M:%S %Z %Y\0";
    // SAFETY: the buffer is writable for its length, the pattern ends in NUL, and strftime
    // writes a NUL-terminated string or nothing.
    unsafe {
        libc::strftime(
            buffer.as_mut_ptr(),
            buffer.len(),
            pattern.as_ptr().cast(),
            &parts,
        );
        std::ffi::CStr::from_ptr(buffer.as_ptr())
    }
    .to_string_lossy()
    .into_owned()
}

/// `+trace`: from the root down, one server per zone, each answer with where it came from.
fn trace_question(args: &DigArgs) -> ExitCode {
    let asked = args.type_code.unwrap_or(types::A);
    let mut stdout = std::io::stdout().lock();
    if args.shown.cmd {
        for line in command_lines(args) {
            let _ = writeln!(stdout, "{line}");
        }
    }
    // The root's nameservers come from the device's resolver, as dig asks for them. The ones
    // rvrs carries stand in when it has none.
    let mut candidates: Vec<(String, Option<IpAddr>)> = Vec::new();
    if let Ok((server, _)) = server_of(args) {
        if let Ok(reply) = send(&question(args, "", types::NS, server, true), args.tries) {
            let roots: Vec<&Record> = reply
                .response
                .answers()
                .filter(|record| record.type_code == types::NS)
                .collect();
            for record in &roots {
                let _ = writeln!(stdout, "{}", record_line(record));
                candidates.push((record.value.trim_end_matches('.').to_string(), None));
            }
            if !roots.is_empty() {
                let _ = writeln!(
                    stdout,
                    "{}\n",
                    received(&reply, server, &server.ip().to_string())
                );
            }
        }
    }
    if candidates.is_empty() {
        candidates = Options::default()
            .roots
            .into_iter()
            .map(|(name, address)| (name, Some(address)))
            .collect();
    }

    for _ in 0..24 {
        let mut answered = None;
        for (name, known) in candidates.iter().take(4) {
            let Some(address) = known.or_else(|| resolve::resolve(name, args.family).ok()) else {
                continue;
            };
            let server = SocketAddr::new(address, 53);
            if let Ok(reply) = send(&question(args, &args.name, asked, server, false), 1) {
                answered = Some((reply, server, name.clone()));
                break;
            }
        }
        let Some((reply, server, name)) = answered else {
            let _ = writeln!(
                stdout,
                ";; connection timed out; no servers could be reached"
            );
            return ExitCode::from(EXIT_NO_REPLY);
        };
        let response = &reply.response;
        for record in response.answers().chain(response.authority()) {
            let _ = writeln!(stdout, "{}", record_line(record));
        }
        let _ = writeln!(stdout, "{}\n", received(&reply, server, &name));
        let _ = stdout.flush();

        let referral: Vec<&Record> = response
            .authority()
            .filter(|record| record.type_code == types::NS)
            .collect();
        if response.rcode != 0 || response.answers().next().is_some() || referral.is_empty() {
            break;
        }
        // The next zone's servers, with the addresses the referral carried along.
        let wanted = |address: &IpAddr| {
            args.family != FamilyChoice::V6 && address.is_ipv4()
                || args.family == FamilyChoice::V6 && address.is_ipv6()
        };
        candidates = referral
            .iter()
            .map(|record| {
                let glue = response
                    .additional()
                    .filter(|extra| extra.name.eq_ignore_ascii_case(&record.value))
                    .filter_map(|extra| extra.value.parse::<IpAddr>().ok())
                    .find(wanted);
                (record.value.trim_end_matches('.').to_string(), glue)
            })
            .collect();
    }
    ExitCode::SUCCESS
}

/// ";; Received 239 bytes from 192.0.2.53#53(a.root-servers.net) in 14 ms"
fn received(reply: &Reply, server: SocketAddr, name: &str) -> String {
    format!(
        ";; Received {} bytes from {}#{}({}) in {} ms",
        reply.size,
        server.ip(),
        server.port(),
        name,
        reply.rtt.as_millis()
    )
}

// The lookup, in the rvrs look

fn lookup(args: &DigArgs, look: Look) -> ExitCode {
    let style = Style::detect(args.common.color);
    let options = Options {
        own_resolvers: args
            .resolvers
            .iter()
            .map(|address| Resolver {
                label: address.to_string(),
                address: *address,
            })
            .collect(),
        ..Options::default()
    };
    let lookups = Lookups {
        names: false,
        networks: !args.no_lookups && look == Look::Pretty,
    };

    term::catch_interrupts();
    let mut session = Session::start_dns(&args.name, options, lookups);
    let mut live = (look == Look::Pretty).then(LiveRegion::start);
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
        if let Some(region) = live.as_mut() {
            let (width, _) = term::size();
            region.draw(
                &result_lines(&session.run, Compared::Later, style, width, frame),
                true,
            );
        }
        if !session.run.is_running() {
            break;
        }
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
        eprintln!("{}", trace::style_error(Style::detect_stderr(), message));
        return ExitCode::from(EXIT_USAGE);
    }

    let found = run.dns.as_ref().is_some_and(|dns| !dns.is_empty());
    let keep = std::env::var("RVRS_HISTORY").map_or(true, |value| value != "0");
    let mut history = History::open();
    let previous = history.previous_dns(run).cloned();
    let mut held = false;
    if keep && found {
        crate::pulse::count_run(run.tool);
        held = crate::sync::queue_for(run, look == Look::Pretty);
        if let Err(error) = history.save(run) {
            eprintln!("rvrs: Couldn't save this run to history ({error}).");
        }
    }

    match look {
        Look::Pretty => {
            if let Some(mut region) = live.take() {
                let (width, _) = term::size();
                let compared = if keep {
                    Compared::With(previous.as_ref())
                } else {
                    Compared::Never
                };
                region.draw(&result_lines(run, compared, style, width, 0), false);
                region.finish();
            }
        }
        Look::Json => println!("{}", Report::of(run).json()),
        Look::Classic => {}
    }
    let _ = std::io::stdout().flush();
    if let Some(case) = args.common.case.as_deref().filter(|_| found) {
        crate::commands::save_after_run(run, case);
    }
    if held {
        crate::sync::print_run_page(run);
    }
    if run.outcome == Some(Outcome::Reached) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// What "Recent changes" has to go on.
#[derive(Clone, Copy)]
pub enum Compared<'a> {
    /// The lookup is still running.
    Later,
    /// The lookup just ended, and this is the lookup of the same name before it, if any.
    With(Option<&'a Run>),
    /// A stored run on its own, or history is off.
    Never,
}

fn header(style: Style, text: &str) -> String {
    style.bold(text)
}

fn title_line(run: &Run, style: Style, width: usize) -> String {
    let lead = format!("{} {} ", style.bold("//"), style.dim("dig"));
    let room = width
        .clamp(40, 160)
        .saturating_sub(visible_width(&lead) + 13);
    format!(
        "{lead}{}  {}",
        style.bold(&truncate_middle(&run.target, room)),
        style.dim("this device")
    )
}

/// The whole result, top to bottom. While the lookup runs, the verdict's place is taken by
/// what is still due.
pub fn result_lines(
    run: &Run,
    compared: Compared,
    style: Style,
    width: usize,
    frame: usize,
) -> Vec<String> {
    let width = width.clamp(40, 120);
    let mut out = vec![String::new(), title_line(run, style, width), String::new()];
    let empty = DnsResult::new(&run.target);
    let dns = run.dns.as_ref().unwrap_or(&empty);
    let diff = Diff::of(dns);

    if run.is_running() {
        let spinner = style.blue(SPINNER[frame % SPINNER.len()]);
        let waiting = match dns.pending_sources {
            0 if dns.sources.is_empty() => "Looking up\u{2026}".to_string(),
            0 => "Walking the delegation from the root\u{2026}".to_string(),
            1 => "1 source still answering".to_string(),
            count => format!("{count} sources still answering"),
        };
        out.push(format!("{spinner} {waiting}"));
    } else if dns.is_empty() {
        out.push(style.yellow(if run.outcome == Some(Outcome::Stopped) {
            "Stopped"
        } else {
            "No source answered"
        }));
        return out;
    } else {
        let verdict = dns.verdict();
        out.push(match verdict.grade {
            VerdictGrade::Good => style.bold_green(&verdict.title),
            VerdictGrade::Attention => style.yellow(&verdict.title),
            VerdictGrade::Problem => style.red(&verdict.title),
        });
        out.extend(pretty::wrap(&verdict.detail, width));
        let addresses = dns.addresses();
        let v4 = addresses
            .iter()
            .filter(|record| record.type_code == types::A)
            .count();
        let mut tiles = vec![format!(
            "{} {v4} / {}",
            style.dim("A / AAAA"),
            addresses.len() - v4
        )];
        if let Some(ttl) = dns.address_ttl() {
            tiles.push(format!("{} {}", style.dim("TTL"), format::ttl(ttl)));
        }
        tiles.push(format!("{} {}", style.dim("DNSSEC"), dns.dnssec.short()));
        out.push(tiles.join(&style.dim(" \u{00B7} ")));
    }

    let keys = dns.key_records();
    if !keys.is_empty() {
        out.push(String::new());
        out.push(header(style, "Key records"));
        out.extend(
            keys.into_iter()
                .map(|record| record_row(record, style, width)),
        );
    }

    let sources = dns.ordered_sources();
    if !sources.is_empty() {
        out.push(String::new());
        out.push(header(style, "Compare"));
        out.extend(compare_rows(&sources, &diff, style, width));
    }
    if !run.is_running() && !diff.notes.is_empty() {
        out.push(String::new());
        out.push(header(style, "Differences"));
        for note in &diff.notes {
            out.push(format!(
                "  {}",
                if note.is_difference {
                    style.yellow(&note.title)
                } else {
                    note.title.clone()
                }
            ));
            out.extend(
                pretty::wrap(&note.detail, width - 2)
                    .into_iter()
                    .map(|line| format!("  {}", style.dim(&line))),
            );
        }
    }

    if !dns.path.is_empty() || (!run.is_running() && !dns.dnssec_detail.is_empty()) {
        out.push(String::new());
        out.push(header(style, "Delegation path"));
        let zones = dns
            .path
            .iter()
            .map(|step| step.zone.len())
            .max()
            .unwrap_or(1)
            .min(28);
        for (index, step) in dns.path.iter().enumerate() {
            let role = step.role(index + 1 == dns.path.len());
            let mut detail = step.detail();
            if step.ds_present == Some(true) {
                detail.push_str(" \u{00B7} DS");
            }
            if step.signed {
                detail.push_str(" \u{00B7} RRSIG");
            }
            let zone = truncate_middle(&step.zone, zones);
            let room = width.saturating_sub(2 + 15 + zones + 2);
            let detail = truncate_end(&detail, room.max(16));
            let detail = if step.error.is_some() {
                style.yellow(&detail)
            } else {
                style.dim(&detail)
            };
            out.push(format!("  {role:<15}{zone:<zones$}  {detail}"));
        }
        if !run.is_running() && !dns.dnssec_detail.is_empty() {
            out.extend(
                pretty::wrap(&dns.dnssec_detail, width - 2)
                    .into_iter()
                    .map(|line| format!("  {}", style.dim(&line))),
            );
        }
    }

    if let Compared::With(previous) = compared {
        out.push(String::new());
        out.push(header(style, "Recent changes"));
        match previous.and_then(|previous| previous.dns.as_ref().map(|then| (previous, then))) {
            None => out.extend(note_lines(
                "First saved lookup of this name. Later lookups are compared with it.",
                style,
                width,
            )),
            Some((previous, then)) => {
                let when = relative(previous.started_at);
                let changed = changes(dns, Some(then));
                if changed.is_empty() {
                    out.extend(note_lines(
                        &format!("Same addresses, names and nameservers as the lookup {when}."),
                        style,
                        width,
                    ));
                } else {
                    out.extend(changed.iter().map(|change| {
                        format!(
                            "  {}",
                            style.yellow(&truncate_end(&change.line(), width - 2))
                        )
                    }));
                    out.push(format!(
                        "  {}",
                        style.dim(&format!("Compared with the lookup {when}."))
                    ));
                }
            }
        }
    }
    out
}

/// A sentence under a header: indented, dim, wrapped to the width.
fn note_lines(text: &str, style: Style, width: usize) -> Vec<String> {
    pretty::wrap(text, width - 2)
        .into_iter()
        .map(|line| format!("  {}", style.dim(&line)))
        .collect()
}

/// "5m ago", in the middle of a sentence.
fn relative(then: u64) -> String {
    let text = format::relative(then, format::now());
    if text == "Just now" {
        "just now".to_string()
    } else {
        text
    }
}

/// "  A      192.0.2.10                    5m"
pub fn record_row(record: &Record, style: Style, width: usize) -> String {
    let room = width.saturating_sub(2 + 7 + 2 + 4).clamp(12, 48);
    let value = truncate_end(&record.value, room);
    let pad = " ".repeat(room.saturating_sub(visible_width(&value)));
    format!(
        "  {:<7}{value}{pad}  {}",
        record.type_name(),
        style.dim(&format::ttl(record.ttl))
    )
}

fn status_word(status: SourceStatus, style: Style) -> String {
    match status {
        SourceStatus::Match => style.green(status.title()),
        SourceStatus::Differs => style.yellow(status.title()),
        SourceStatus::Error => style.red(status.title()),
    }
}

/// One row per source: who answered, the first IPv4 address, its TTL, the time and how the
/// answer compares with the reference.
pub fn compare_rows(
    sources: &[&SourceResult],
    diff: &Diff,
    style: Style,
    width: usize,
) -> Vec<String> {
    let wide = width >= 86;
    let name_width = if width >= 72 { 22 } else { 14 };
    let (address_width, server_width) = (18, 17);
    let mut head = format!("  {:<name_width$}", "Source");
    if wide {
        head.push_str(&format!("{:<server_width$}", "Server"));
    }
    head.push_str(&format!(
        "{:<address_width$}{:<6}{:>7}  Status",
        "A", "TTL", "Time"
    ));
    let mut out = vec![style.dim(&head)];
    for source in sources {
        let mut row = format!(
            "  {:<name_width$}",
            truncate_end(&source.label, name_width - 2)
        );
        if wide {
            let server = if source.server == source.label {
                String::new()
            } else {
                truncate_middle(&source.server, server_width - 2)
            };
            let pad = " ".repeat(server_width - visible_width(&server));
            row.push_str(&format!("{}{pad}", style.dim(&server)));
        }
        let time = source
            .rtt_ms
            .map_or_else(String::new, |rtt| format!("{rtt:.0} ms"));
        let status = diff.status(source);
        if let Some(error) = &source.error {
            let room = address_width + 6 + 7;
            row.push_str(&format!("{:<room$}", truncate_end(error, room - 1)));
        } else {
            let values = source.values(types::A);
            let first = match values.first() {
                Some(first) if values.len() > 1 => format!("{first} +{}", values.len() - 1),
                Some(first) => first.clone(),
                None => source
                    .failures
                    .get("A")
                    .cloned()
                    .unwrap_or_else(|| "\u{2013}".to_string()),
            };
            let ttl = source
                .answers()
                .find(|record| record.type_code == types::A)
                .map_or_else(String::new, |record| format::ttl(record.ttl));
            row.push_str(&format!(
                "{:<address_width$}{ttl:<6}{time:>7}",
                truncate_end(&first, address_width - 1)
            ));
        }
        out.push(format!("{row}  {}", status_word(status, style)));
    }
    out
}

/// A stored lookup on a terminal.
pub fn saved_lines(run: &Run, style: Style, width: usize) -> Vec<String> {
    let mut lines = result_lines(run, Compared::Never, style, width, 0);
    lines.remove(0);
    lines
}

/// A stored lookup in a pipe: the reference answer as lines of a zone file.
pub fn zone_lines(run: &Run) -> Vec<String> {
    run.dns
        .as_ref()
        .and_then(DnsResult::reference)
        .map(|reference| reference.answers().map(record_line).collect())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn parsed(words: &[&str], multicall: bool) -> DigArgs {
        let arguments: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
        match parse(&arguments, multicall) {
            Ok(Parsed::Run(args)) => *args,
            other => panic!("{other:?}"),
        }
    }

    fn refused(words: &[&str]) -> String {
        let arguments: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
        parse(&arguments, false).unwrap_err()
    }

    #[test]
    fn a_name_alone_is_the_lookup_and_anything_of_digs_is_one_question() {
        let lookup = parsed(&["https://Example.com/path"], false);
        assert!(!lookup.one_question);
        assert_eq!(lookup.name, "example.com");

        for words in [
            &["example.com", "MX"][..],
            &["@1.1.1.1", "example.com"],
            &["example.com", "+short"],
            &["-t", "AAAA", "example.com"],
            &["mx", "example.com"],
        ] {
            assert!(parsed(words, false).one_question, "{words:?}");
        }
        assert!(parsed(&["example.com"], true).one_question);
        assert_eq!(
            parsed(&["mx", "example.com"], false).type_code,
            Some(types::MX)
        );
        assert_eq!(
            parsed(&["example.com", "in", "TYPE65"], false).type_code,
            Some(types::HTTPS)
        );
    }

    #[test]
    fn digs_flags_and_options_are_read() {
        let args = parsed(
            &[
                "@9.9.9.9",
                "-p",
                "5353",
                "-tNS",
                "example.com.",
                "+tcp",
                "+dnssec",
                "+norecurse",
                "+time=2",
                "+retry=1",
                "-4",
            ],
            false,
        );
        assert_eq!(args.server.as_deref(), Some("9.9.9.9"));
        assert_eq!((args.port, args.type_code), (5353, Some(types::NS)));
        assert_eq!(args.name, "example.com");
        assert!(args.tcp && args.dnssec && !args.recursive);
        assert_eq!((args.timeout, args.tries), (Duration::from_secs(2), 2));
        assert_eq!(args.family, FamilyChoice::V4);

        let only = parsed(&["example.com", "+noall", "+answer"], false);
        assert_eq!(
            only.shown,
            Shown {
                answer: true,
                ..Shown::NONE
            }
        );

        let reverse = parsed(&["-x", "192.0.2.7"], false);
        assert_eq!(reverse.name, "7.2.0.192.in-addr.arpa");
        assert_eq!(reverse.type_code, Some(types::PTR));

        // dig alone asks for the root's nameservers.
        let root = parsed(&[], true);
        assert_eq!((root.name.as_str(), root.type_code), ("", Some(types::NS)));

        let own = parsed(
            &[
                "example.com",
                "--resolver",
                "10.0.0.53",
                "--resolver=10.0.0.54",
            ],
            false,
        );
        assert_eq!(own.resolvers.len(), 2);
        assert!(!own.one_question);
    }

    #[test]
    fn what_rvrs_doesnt_take_is_named() {
        assert_eq!(
            refused(&[]),
            "rvrs dig needs a name, such as rvrs dig example.com"
        );
        assert_eq!(
            refused(&["example.com", "+multiline"]),
            "+multiline isn't supported yet. Supported options: rvrs dig --help"
        );
        assert_eq!(
            refused(&["a.example", "b.example"]),
            "rvrs dig takes one name, and \u{201C}b.example\u{201D} comes after a.example."
        );
        assert_eq!(
            refused(&["-t", "nope", "example.com"]),
            "-t takes a record type such as A or MX, and \u{201C}nope\u{201D} isn't one."
        );
        assert_eq!(
            refused(&["example.com", "-c", "CH"]),
            "rvrs dig asks in class IN only, and -c names CH."
        );
        assert_eq!(
            refused(&["example.com", "MX", "--json"]),
            "--json prints the whole lookup, so it takes a name and nothing of dig's."
        );
        assert_eq!(
            refused(&["example.com", "+time"]),
            "+time takes a number of seconds, as in +time=2"
        );
        assert_eq!(
            refused(&["example.com", "--resolver", "ns1"]),
            "--resolver takes an address such as 10.0.0.53, and \u{201C}ns1\u{201D} isn't one."
        );
    }

    fn answer() -> Response {
        Response {
            id: 4242,
            flags: 0x8180,
            rcode: 0,
            counts: [1, 2, 0, 1],
            question: Some(("example.com.".to_string(), types::A)),
            records: vec![
                Record {
                    name: "example.com.".into(),
                    type_code: types::A,
                    ttl: 300,
                    value: "192.0.2.10".into(),
                    section: Section::Answer,
                },
                Record {
                    name: "a-rather-long-name.example.com.".into(),
                    type_code: types::A,
                    ttl: 60,
                    value: "192.0.2.11".into(),
                    section: Section::Answer,
                },
            ],
            edns: Some(rvrs_core::dns::message::Edns {
                udp_size: 1232,
                version: 0,
                dnssec_ok: false,
            }),
        }
    }

    #[test]
    fn an_answer_prints_as_dig_prints_it() {
        let args = parsed(&["example.com", "+nocmd"], false);
        let stats = Stats {
            millis: 12,
            server: "192.0.2.53:53".parse().unwrap(),
            server_name: "192.0.2.53",
            when: "Fri Nov 20 10:00:00 UTC 2026".into(),
            size: 72,
        };
        let lines = reply_lines(&args, &answer(), Some(&stats));
        assert_eq!(
            lines,
            [
                ";; Got answer:",
                ";; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 4242",
                ";; flags: qr rd ra; QUERY: 1, ANSWER: 2, AUTHORITY: 0, ADDITIONAL: 1",
                "",
                ";; OPT PSEUDOSECTION:",
                "; EDNS: version: 0, flags:; udp: 1232",
                ";; QUESTION SECTION:",
                ";example.com.\t\t\tIN\tA",
                "",
                ";; ANSWER SECTION:",
                "example.com.\t\t300\tIN\tA\t192.0.2.10",
                "a-rather-long-name.example.com.\t60\tIN\tA\t192.0.2.11",
                "",
                ";; Query time: 12 msec",
                ";; SERVER: 192.0.2.53#53(192.0.2.53)",
                ";; WHEN: Fri Nov 20 10:00:00 UTC 2026",
                ";; MSG SIZE  rcvd: 72",
                "",
            ]
        );

        let only = parsed(&["example.com", "+noall", "+answer"], false);
        assert_eq!(reply_lines(&only, &answer(), Some(&stats)).len(), 2);
    }

    /// A finished lookup with three sources that agree.
    pub(crate) fn finished() -> Run {
        use rvrs_core::dns::{DelegationStep, DnsEvent, Dnssec, SourceKind};
        let mut run = Run::new(
            rvrs_core::model::Tool::Dns,
            "example.com",
            FamilyChoice::Auto,
            0,
            0,
        );
        run.apply_dns(&DnsEvent::Plan(3));
        for (kind, label, server, rtt) in [
            (SourceKind::Device, "Router", "192.168.1.1", 4.0),
            (SourceKind::Public, "Cloudflare", "1.1.1.1", 9.0),
            (
                SourceKind::Authoritative,
                "ns1.example-dns.net",
                "198.51.100.53",
                21.0,
            ),
        ] {
            let mut source = SourceResult::new(kind, label, server);
            source.rtt_ms = Some(rtt);
            source.is_authoritative = kind == SourceKind::Authoritative;
            for (type_code, value, ttl) in [
                (types::A, "192.0.2.10", 300),
                (types::AAAA, "2001:db8::10", 300),
                (types::NS, "ns1.example-dns.net", 86_400),
                (types::MX, "10 mail.example.com", 3600),
            ] {
                source.records.push(Record {
                    name: "example.com".into(),
                    type_code,
                    ttl,
                    value: value.into(),
                    section: Section::Answer,
                });
            }
            run.apply_dns(&DnsEvent::Source(source));
        }
        let mut root = DelegationStep::new(".", "a.root-servers.net", "198.41.0.4");
        root.rtt_ms = Some(14.0);
        root.next_zone = Some("com".into());
        root.nameservers = vec!["a.gtld-servers.net".into(), "b.gtld-servers.net".into()];
        root.ds_present = Some(true);
        run.apply_dns(&DnsEvent::Step(root));
        run.apply_dns(&DnsEvent::Dnssec(
            Dnssec::Unsigned,
            "com publishes no DS for example.com, so the chain stops there.".into(),
        ));
        run.apply_dns(&DnsEvent::Finished);
        run
    }

    #[test]
    fn the_lookup_reads_top_to_bottom() {
        let run = finished();
        let text = result_lines(&run, Compared::With(None), Style::PLAIN, 100, 0).join("\n");
        for part in [
            "// dig example.com  this device",
            "All sources agree",
            "A / AAAA 1 / 1 \u{00B7} TTL 5m \u{00B7} DNSSEC None",
            "Key records",
            "  A      192.0.2.10",
            "  MX     10 mail.example.com",
            "Compare",
            "Router                192.168.1.1      192.0.2.10        5m       4 ms  Match",
            "Delegation path",
            "  Root           .  a.root-servers.net \u{00B7} 14 ms \u{00B7} delegates com to 2 servers \u{00B7} DS",
            "com publishes no DS for example.com, so the chain stops there.",
            "Recent changes",
            "First saved lookup of this name. Later lookups are compared with it.",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        assert!(!text.contains("Differences"));

        // A later lookup with another address names the change.
        let mut later = finished();
        later.started_at = 600;
        for source in &mut later.dns.as_mut().unwrap().sources {
            source.records[0].value = "192.0.2.77".into();
        }
        let text =
            result_lines(&later, Compared::With(Some(&run)), Style::PLAIN, 100, 0).join("\n");
        assert!(
            text.contains("  A: 192.0.2.10 \u{2192} 192.0.2.77"),
            "{text}"
        );
        assert!(text.contains("Compared with the lookup "), "{text}");
    }

    #[test]
    fn every_line_fits_a_narrow_terminal() {
        let mut run = finished();
        let dns = run.dns.as_mut().unwrap();
        dns.sources[1].error = Some("1.1.1.1 didn't answer in time.".into());
        dns.sources[1].records.clear();
        for width in [40, 60, 80, 100] {
            for line in result_lines(&run, Compared::With(None), Style::PLAIN, width, 0) {
                assert!(visible_width(&line) <= width.max(64), "{width}: {line}");
            }
        }
        assert_eq!(
            zone_lines(&run)[0],
            "example.com.\t\t300\tIN\tA\t192.0.2.10"
        );
    }

    #[test]
    fn a_running_lookup_says_what_is_still_due() {
        use rvrs_core::dns::DnsEvent;
        let mut run = Run::new(
            rvrs_core::model::Tool::Dns,
            "example.com",
            FamilyChoice::Auto,
            0,
            0,
        );
        assert!(result_lines(&run, Compared::Later, Style::PLAIN, 80, 0)
            .join("\n")
            .contains("Looking up\u{2026}"));
        run.apply_dns(&DnsEvent::Plan(5));
        assert!(result_lines(&run, Compared::Later, Style::PLAIN, 80, 0)
            .join("\n")
            .contains("5 sources still answering"));
        run.apply_dns(&DnsEvent::Finished);
        assert!(result_lines(&run, Compared::Never, Style::PLAIN, 80, 0)
            .join("\n")
            .contains("No source answered"));
    }
}
