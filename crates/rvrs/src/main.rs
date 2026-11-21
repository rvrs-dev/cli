//! rvrs: see what the network is doing. One binary that takes the place of the network tools
//! engineers already type, with their flags, and shows results that explain themselves.

mod account;
mod args;
mod bgp;
mod classic;
mod commands;
mod completions;
mod console;
mod dig;
mod help;
mod mtr;
mod ping;
mod pretty;
mod pulse;
mod style;
mod term;
mod trace;
mod update;

use std::io::IsTerminal;
use std::path::Path;
use std::process::ExitCode;

use style::{ColorChoice, Style};

/// The run could not be made or the command could not do what was asked.
pub const EXIT_FAILED: u8 = 1;
/// The command line was wrong.
pub const EXIT_USAGE: u8 = 2;

pub fn fail(message: &str) -> ExitCode {
    eprintln!("{}", trace::style_error(Style::detect_stderr(), message));
    ExitCode::from(EXIT_FAILED)
}

pub fn usage(message: &str) -> ExitCode {
    eprintln!("{}", trace::style_error(Style::detect_stderr(), message));
    ExitCode::from(EXIT_USAGE)
}

const COMMANDS: [&str; 28] = [
    "update",
    "favorite",
    "favorites",
    "save",
    "saved",
    "case",
    "cases",
    "traceroute",
    "mtr",
    "ping",
    "dig",
    "dns",
    "iperf3",
    "last",
    "show",
    "history",
    "status",
    "completions",
    "login",
    "logout",
    "share",
    "node",
    "nodes",
    "bgp",
    "schedule",
    "schedules",
    "alerts",
    "help",
];

/// A reader that has what it wanted closes the pipe, as `head` does. That ends rvrs quietly with
/// the code a shell gives a process the pipe signal ended, never with a panic message.
fn end_quietly_on_a_closed_pipe() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or_default();
        if message.contains("failed printing to stdout") {
            std::process::exit(141);
        }
        default(info);
    }));
}

fn main() -> ExitCode {
    end_quietly_on_a_closed_pipe();
    let mut arguments: Vec<String> = std::env::args().collect();
    let invoked_as = arguments
        .first()
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    arguments.remove(0);
    // Every start counts as an open, as an app's launch does, except rvrs's own hidden helpers.
    if !arguments
        .first()
        .is_some_and(|command| command.starts_with("__"))
    {
        pulse::count_open();
    }

    // Invoked under the original's name (a `traceroute` symlink to rvrs), it is the original.
    if invoked_as == "traceroute" {
        return traceroute(&arguments, true);
    }
    if invoked_as == "ping" {
        return ping(&arguments, true);
    }
    if invoked_as == "mtr" {
        return mtr(&arguments, true);
    }
    if invoked_as == "dig" {
        return dig(&arguments, true);
    }

    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let Some(command) = arguments.first().cloned() else {
        if interactive {
            return console::open(None);
        }
        print!("{}", help::main(Style::detect(ColorChoice::Auto)));
        return ExitCode::SUCCESS;
    };
    let rest = &arguments[1..];

    match command.as_str() {
        // After a tool has printed its result, one line may say that a newer rvrs is out.
        "traceroute" => noticed(traceroute(rest, false)),
        "ping" => noticed(ping(rest, false)),
        "mtr" => noticed(mtr(rest, false)),
        "dig" | "dns" => noticed(dig(rest, false)),
        "bgp" => noticed(bgp(rest)),
        "iperf3" => fail(
            "rvrs iperf3 isn't in the CLI yet. The rvrs app for iPhone and iPad runs it today.",
        ),
        "last" => commands::last(rest),
        "show" => commands::show(rest),
        "history" => commands::history(rest),
        "save" => commands::save(rest),
        "saved" => commands::saved(rest),
        "case" | "cases" => commands::case(rest),
        "favorite" => commands::favorite(rest),
        "favorites" => commands::favorites(rest),
        "status" => commands::status(),
        "update" => update::run(rest),
        "__update-check" => update::check_now(),
        "__pulse" => pulse::send_now(),
        "completions" => commands::completions(rest),
        "login" => account::login(rest),
        "logout" => account::logout(rest),
        "share" => fail("Share links aren't available yet. To send a run, print it as a report with rvrs last --json."),
        "node" | "nodes" => fail("Nodes aren't available yet, so runs come from this device. The waitlist is at rvrs.dev."),
        "schedule" | "schedules" | "alerts" => fail("Schedules and alerts come with nodes, which aren't available yet. The waitlist is at rvrs.dev."),
        "help" | "--help" | "-h" => {
            let style = Style::detect(ColorChoice::Auto);
            match rest.first().map(String::as_str) {
                Some("traceroute") => print!("{}", help::traceroute(style)),
                Some("ping") => print!("{}", help::ping(style)),
                Some("mtr") => print!("{}", help::mtr(style)),
                Some("dig" | "dns") => print!("{}", help::dig(style)),
                Some("bgp") => print!("{}", help::bgp(style)),
                _ => print!("{}", help::main(style)),
            }
            ExitCode::SUCCESS
        }
        "--version" | "-V" | "version" => {
            println!("{}", help::version());
            ExitCode::SUCCESS
        }
        "__targets" => commands::targets(),
        "__runs" => commands::runs(),
        "__cases" => commands::case_names(),
        other if looks_like_target(other) => {
            if interactive && rest.is_empty() {
                console::open(Some(args::normalize_target(other)))
            } else {
                traceroute(&arguments, false)
            }
        }
        other => {
            let closest = COMMANDS.iter().map(|known| (distance(other, known), *known)).min().filter(|(score, _)| *score <= 3);
            match closest {
                Some((_, known)) => usage(&format!("rvrs has no command \u{201C}{other}\u{201D}. The closest is {known}.")),
                None => usage(&format!("rvrs has no command \u{201C}{other}\u{201D}. List them with rvrs help.")),
            }
        }
    }
}

fn traceroute(arguments: &[String], multicall: bool) -> ExitCode {
    match args::parse(arguments) {
        Ok(args::Parsed::Run(parsed)) => trace::run(*parsed, multicall),
        Ok(args::Parsed::Help) => {
            print!("{}", help::traceroute(Style::detect(ColorChoice::Auto)));
            ExitCode::SUCCESS
        }
        Ok(args::Parsed::Version) => {
            println!("{}", help::version());
            ExitCode::SUCCESS
        }
        Err(message) => usage(&message),
    }
}

fn noticed(code: ExitCode) -> ExitCode {
    update::after_command();
    code
}

fn mtr(arguments: &[String], multicall: bool) -> ExitCode {
    match mtr::parse(arguments) {
        Ok(mtr::Parsed::Run(parsed)) => mtr::run(*parsed, multicall),
        Ok(mtr::Parsed::Help) => {
            print!("{}", help::mtr(Style::detect(ColorChoice::Auto)));
            ExitCode::SUCCESS
        }
        Ok(mtr::Parsed::Version) => {
            println!("{}", help::version());
            ExitCode::SUCCESS
        }
        Err(message) => usage(&message),
    }
}

fn dig(arguments: &[String], multicall: bool) -> ExitCode {
    match dig::parse(arguments, multicall) {
        Ok(dig::Parsed::Run(parsed)) => dig::run(*parsed, multicall),
        Ok(dig::Parsed::Help) => {
            print!("{}", help::dig(Style::detect(ColorChoice::Auto)));
            ExitCode::SUCCESS
        }
        Ok(dig::Parsed::Version) => {
            println!("{}", help::version());
            ExitCode::SUCCESS
        }
        Err(message) => usage(&message),
    }
}

fn bgp(arguments: &[String]) -> ExitCode {
    match bgp::parse(arguments) {
        Ok(bgp::Parsed::Run(parsed)) => bgp::run(*parsed),
        Ok(bgp::Parsed::Check {
            prefix,
            asn,
            json,
            color,
        }) => bgp::check(&prefix, asn, json, color),
        Ok(bgp::Parsed::Glass(command, color)) => bgp::glass(command, color),
        Ok(bgp::Parsed::Help) => {
            print!("{}", help::bgp(Style::detect(ColorChoice::Auto)));
            ExitCode::SUCCESS
        }
        Ok(bgp::Parsed::Version) => {
            println!("{}", help::version());
            ExitCode::SUCCESS
        }
        Err(message) => usage(&message),
    }
}

fn ping(arguments: &[String], multicall: bool) -> ExitCode {
    match ping::parse(arguments) {
        Ok(ping::Parsed::Run(parsed)) => ping::run(*parsed, multicall),
        Ok(ping::Parsed::Help) => {
            print!("{}", help::ping(Style::detect(ColorChoice::Auto)));
            ExitCode::SUCCESS
        }
        Ok(ping::Parsed::Version) => {
            println!("{}", help::version());
            ExitCode::SUCCESS
        }
        Err(message) => usage(&message),
    }
}

/// An address, or a name with a dot in it. `rvrs example.com` needs no command.
fn looks_like_target(word: &str) -> bool {
    let host = args::normalize_target(word);
    !word.starts_with('-')
        && (host.parse::<std::net::IpAddr>().is_ok() || host.contains('.') || host == "localhost")
}

/// Edit distance, for "the closest is".
fn distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (previous + usize::from(a != *b))
                .min(row[j] + 1)
                .min(current + 1);
            previous = current;
        }
    }
    row[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_need_no_command() {
        for word in [
            "1.1.1.1",
            "example.com",
            "::1",
            "2606:4700:4700::1111",
            "https://example.com/x",
            "localhost",
        ] {
            assert!(looks_like_target(word), "{word}");
        }
        for word in ["tracerout", "--json", "pign"] {
            assert!(!looks_like_target(word), "{word}");
        }
    }

    #[test]
    fn typos_find_their_command() {
        assert_eq!(distance("tracerout", "traceroute"), 1);
        assert_eq!(distance("histroy", "history"), 2);
        assert_eq!(distance("", "abc"), 3);
    }
}
