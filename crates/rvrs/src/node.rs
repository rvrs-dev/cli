//! `rvrs node`: this binary as a node, a machine that makes runs on request.
//!
//! It reads what the node declares about itself, its tags and its location, from the flags, the
//! environment and the node's file (`schema/node-config.md`), and what its owner set on the
//! device: where it keeps its state, its limits and the ranges it may probe
//! (`docs/node-safety.md`). Then it takes its token and runs the loop in [`service`].
//!
//! A node has a token of its own, given as `RVRS_TOKEN` or kept in its state folder. It never
//! uses the credential of a person who signed in on the same machine, and until it has a token
//! it makes no call at all.

mod api;
mod kept;
mod outbox;
mod runner;
mod service;

pub use service::word;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rvrs_core::node::address::Allowed;
use rvrs_core::node::setup::{Credential, Found, Secret, Setup};
use rvrs_core::node::{Guard, Limits};
use rvrs_core::node_config::{Flags, NodeDeclaration};
use rvrs_core::paths;

/// Where the node's file is when nothing says otherwise.
pub const FILE_NAME: &str = "node.json";
/// The environment variable that names the node's file.
pub const ENV_CONFIG: &str = "RVRS_NODE_CONFIG";

/// Where the node keeps its state when nothing says otherwise: its token, its outbox and its
/// copy of its schedules.
pub const ENV_STATE: &str = "RVRS_NODE_STATE";
/// Ranges of private or shared address space the node's owner lets it probe.
pub const ENV_ALLOW: &str = "RVRS_NODE_ALLOW";
/// Limits the node's owner set, as `runsPerMinute=6,secondsPerRun=120`.
pub const ENV_LIMITS: &str = "RVRS_NODE_LIMITS";
/// An enrollment token. Enrolling isn't served by the API yet.
pub const ENV_ENROLLMENT: &str = "RVRS_ENROLLMENT_TOKEN";

/// What follows `rvrs node`, as typed.
#[derive(Debug, Clone, Default, PartialEq)]
struct Typed {
    flags: Flags,
    config: Option<PathBuf>,
    /// `--state <folder>`.
    state: Option<PathBuf>,
    /// Each `--allow <ranges>`.
    allow: Vec<String>,
    /// Each `--limit <name>=<value>`.
    limits: Vec<String>,
}

/// Reads the flags of `rvrs node`. One it doesn't know is refused: a node that ignored a flag
/// would run with other settings than its owner wrote.
fn parse(arguments: &[String]) -> Result<Typed, String> {
    let mut typed = Typed::default();
    let mut at = 0;
    while at < arguments.len() {
        let argument = &arguments[at];
        at += 1;
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
            _ => (argument.as_str(), None),
        };
        let wants = match name {
            "--tag" => "a tag, as in --tag isp=telia",
            "--location" => "a latitude and a longitude, as in --location 55.5,8.5",
            "--place" => "a place, as in --place \"Esbjerg, DK\"",
            "--location-precision" => "exact, area or city",
            "--config" => "the path of a file",
            "--state" => "the path of a folder",
            "--allow" => "a range, as in --allow 192.168.1.0/24",
            "--limit" => "a limit and its value, as in --limit runsPerMinute=6",
            _ => {
                return Err(format!(
                "rvrs node has no flag \u{201C}{argument}\u{201D}. List them with rvrs help node."
            ))
            }
        };
        let value = match inline {
            Some(value) => value,
            None => {
                let next = arguments.get(at).filter(|next| !next.starts_with("--"));
                let Some(next) = next else {
                    return Err(format!("{name} takes {wants}."));
                };
                at += 1;
                next.clone()
            }
        };
        match name {
            "--tag" => typed.flags.tags.push(value),
            "--location" => typed.flags.location = Some(value),
            "--place" => typed.flags.place = Some(value),
            "--location-precision" => typed.flags.precision = Some(value),
            "--state" => typed.state = Some(PathBuf::from(value)),
            "--allow" => typed.allow.push(value),
            "--limit" => typed.limits.push(value),
            _ => typed.config = Some(PathBuf::from(value)),
        }
    }
    Ok(typed)
}

/// The ranges the owner allowed, from the environment and every `--allow`.
fn allowed(typed: &Typed, env: &dyn Fn(&str) -> Option<String>) -> Result<Allowed, String> {
    let mut ranges: Vec<String> = env(ENV_ALLOW).into_iter().collect();
    ranges.extend(typed.allow.iter().cloned());
    let ranges: Vec<&str> = ranges
        .iter()
        .map(|range| range.trim())
        .filter(|range| !range.is_empty())
        .collect();
    if ranges.is_empty() {
        return Ok(Allowed::none());
    }
    Allowed::parse(&ranges.join(","))
}

/// The limits the owner set, the environment's first and each `--limit` over them. A limit past
/// its ceiling is refused, so nobody believes a node keeps a limit it doesn't.
fn limits(typed: &Typed, env: &dyn Fn(&str) -> Option<String>) -> Result<Limits, String> {
    let mut limits = Limits::default();
    let from_env = env(ENV_LIMITS).unwrap_or_default();
    let written = from_env
        .split(',')
        .map(str::to_string)
        .chain(typed.limits.iter().cloned());
    for limit in written {
        let limit = limit.trim();
        if limit.is_empty() {
            continue;
        }
        let Some((name, value)) = limit.split_once('=') else {
            return Err(format!(
                "A limit is written as its name and its value, as in runsPerMinute=6. \u{201C}{limit}\u{201D} has no value."
            ));
        };
        limits.set(name.trim(), value)?;
    }
    Ok(limits)
}

/// What the node declares, from the flags, the environment and the file. `read` is how a file is
/// read, so a test needs no disk.
fn declaration(
    arguments: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    default_file: &Path,
    read: &dyn Fn(&Path) -> std::io::Result<String>,
) -> Result<NodeDeclaration, String> {
    let typed = parse(arguments)?;
    let flags = typed.flags;
    let named = typed.config.or_else(|| {
        env(ENV_CONFIG)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    });
    let explicit = named.is_some();
    let path = named.unwrap_or_else(|| default_file.to_path_buf());
    let file = match read(&path) {
        Ok(text) => {
            let shown = paths::display(&path);
            Some(NodeDeclaration::from_file(&text, &shown)?)
        }
        // The file in the config folder is optional. One the person named has to be there.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit => None,
        Err(error) => {
            return Err(format!(
                "Couldn't read the node's file {}. {}.",
                paths::display(&path),
                capitalized(
                    error
                        .to_string()
                        .split(" (os error")
                        .next()
                        .unwrap_or_default()
                )
            ))
        }
    };
    NodeDeclaration::from_sources(&flags, env, file.as_ref())
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
        .unwrap_or_default()
}

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_stop(_signal: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// A node stops on Ctrl-C and when the system asks it to, as a container does on `docker stop`.
fn catch_stops() {
    // SAFETY: the handler only touches an atomic.
    unsafe {
        let handler = on_stop as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}

/// The token the node keeps in its state folder, when there is one.
fn kept_token(state: &Path) -> Option<String> {
    std::fs::read_to_string(state.join("token"))
        .ok()
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

/// What the machine calls itself, as a name for the node until the API has said one.
fn host_name() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: gethostname writes at most the length it is given into the buffer.
    let named = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
    let name = String::from_utf8_lossy(&buffer[..end]).trim().to_string();
    if named && !name.is_empty() {
        name
    } else {
        "this node".to_string()
    }
}

pub fn run(arguments: &[String]) -> ExitCode {
    if arguments
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        print!(
            "{}",
            crate::help::node(crate::style::Style::detect(crate::style::ColorChoice::Auto))
        );
        return ExitCode::SUCCESS;
    }
    // The pulse of a node says so, and is kept apart from a CLI's on the same machine. Set
    // before any thread starts, and inherited by the process that sends it.
    std::env::set_var(crate::pulse::ENV_PRODUCT, "node");
    let env = |name: &str| std::env::var(name).ok();
    let default_file = paths::config_dir().join(FILE_NAME);
    let read = |path: &Path| std::fs::read_to_string(path);
    let declaration = match declaration(arguments, &env, &default_file, &read) {
        Ok(declaration) => declaration,
        Err(message) => return crate::usage(&message),
    };
    // The flags were read once already, so this can't fail.
    let typed = parse(arguments).unwrap_or_default();
    let (allowed, limits) = match (allowed(&typed, &env), limits(&typed, &env)) {
        (Ok(allowed), Ok(limits)) => (allowed, limits),
        (Err(message), _) | (_, Err(message)) => return crate::usage(&message),
    };
    let state = typed
        .state
        .or_else(|| {
            env(ENV_STATE)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| paths::data_dir().join("node"));
    let given = |name: &str| env(name).filter(|value| !value.trim().is_empty());

    catch_stops();
    let stop = Arc::new(AtomicBool::new(false));
    {
        // The signal sets a static, and the loop reads an Arc it shares with its run threads.
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || loop {
            if STOP.load(Ordering::Relaxed) {
                stop.store(true, Ordering::Relaxed);
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        });
    }

    // A token the API refused. The node waits for another one and makes no call meanwhile.
    let mut refused: Option<String> = None;
    loop {
        let found = Found {
            off: false,
            kept: kept_token(&state),
            token: given("RVRS_TOKEN"),
            enrollment: given(ENV_ENROLLMENT),
        };
        let setup = Setup::of(found);
        let token = match setup.credential() {
            Some(Credential::Token(token)) => token.clone(),
            Some(Credential::Enrollment(_)) => {
                return crate::fail(
                    "Enrolling isn't available yet. Give this node a token made in the web app, as RVRS_TOKEN.",
                )
            }
            None => return crate::fail(setup.notice().unwrap_or_default()),
        };
        if refused.as_deref() == Some(token.expose()) {
            if stop.load(Ordering::Relaxed) {
                return ExitCode::SUCCESS;
            }
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let ended = service::run(service::Settings {
            api: api::Api::new(&crate::account::api_base(), Secret::new(token.expose())),
            guard: Guard::new(limits, &crate::features::included(), allowed.clone()),
            outbox: outbox::Outbox::open(&state.join("outbox")),
            schedules: kept::KeptCopy::at(&state),
            declaration: declaration.clone(),
            name: host_name(),
            hands: service::Hands::default(),
            stop: Arc::clone(&stop),
            log: Box::new(|line| println!("{line}")),
            // Counted only once the API took a heartbeat: a node without a working token makes
            // no call, the pulse included.
            count: Box::new(|counted| match counted {
                service::Counted::Heartbeat => {
                    static OPENED: AtomicBool = AtomicBool::new(false);
                    if !OPENED.swap(true, Ordering::Relaxed) {
                        crate::pulse::count_open();
                    } else {
                        crate::pulse::when_due();
                    }
                }
                service::Counted::Run(tool) => crate::pulse::count_run(tool),
            }),
        });
        match ended {
            service::Ended::Stopped => return ExitCode::SUCCESS,
            service::Ended::NoToken => refused = Some(token.expose().to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    fn missing(_: &Path) -> std::io::Result<String> {
        Err(Error::new(ErrorKind::NotFound, "no such file"))
    }

    fn default_file() -> PathBuf {
        PathBuf::from("/nowhere/node.json")
    }

    #[test]
    fn the_flags_of_a_node_are_read_in_both_spellings() {
        let typed = parse(&words(
            "--tag isp=telia --tag=line=ftth --location 55.5,8.5 --place=Esbjerg --location-precision area --config /etc/rvrs/node.json",
        ))
        .unwrap();
        assert_eq!(typed.flags.tags, ["isp=telia", "line=ftth"]);
        assert_eq!(typed.flags.location.as_deref(), Some("55.5,8.5"));
        assert_eq!(typed.flags.place.as_deref(), Some("Esbjerg"));
        assert_eq!(typed.flags.precision.as_deref(), Some("area"));
        assert_eq!(typed.config, Some(PathBuf::from("/etc/rvrs/node.json")));
    }

    #[test]
    fn a_flag_without_its_value_says_what_it_takes() {
        assert_eq!(
            parse(&words("--tag")).unwrap_err(),
            "--tag takes a tag, as in --tag isp=telia."
        );
        assert_eq!(
            parse(&words("--location --place x")).unwrap_err(),
            "--location takes a latitude and a longitude, as in --location 55.5,8.5."
        );
    }

    #[test]
    fn a_flag_the_node_does_not_know_is_refused() {
        assert_eq!(
            parse(&words("--tokn abc")).unwrap_err(),
            "rvrs node has no flag \u{201C}--tokn\u{201D}. List them with rvrs help node."
        );
        assert!(parse(&words("fra-1")).is_err());
    }

    #[test]
    fn the_owner_sets_where_the_state_is_what_is_allowed_and_the_limits() {
        let typed = parse(&words(
            "--state /var/lib/rvrs --allow 192.168.1.0/24 --allow=10.0.0.0/8 --limit runsPerMinute=6",
        ))
        .unwrap();
        assert_eq!(typed.state, Some(PathBuf::from("/var/lib/rvrs")));
        let allowed = allowed(&typed, &none).unwrap();
        assert_eq!(allowed.prefixes().len(), 2);
        assert_eq!(limits(&typed, &none).unwrap().runs_per_minute, 6);

        // The environment sets them too, and a flag is read over it.
        let env = |name: &str| match name {
            ENV_ALLOW => Some("fd12:3456::/32".to_string()),
            ENV_LIMITS => Some("runsPerMinute=3, secondsPerRun=120".to_string()),
            _ => None,
        };
        assert_eq!(allowed_count(&typed, &env), 3);
        let set = limits(&typed, &env).unwrap();
        assert_eq!((set.runs_per_minute, set.seconds_per_run), (6, 120));
    }

    fn allowed_count(typed: &Typed, env: &dyn Fn(&str) -> Option<String>) -> usize {
        allowed(typed, env).unwrap().prefixes().len()
    }

    #[test]
    fn a_limit_past_its_ceiling_or_without_a_value_is_refused() {
        let over = parse(&words("--limit runsPerMinute=600")).unwrap();
        assert_eq!(
            limits(&over, &none).unwrap_err(),
            "runsPerMinute must be a whole number from 0 to 60."
        );
        let bare = parse(&words("--limit runsPerMinute")).unwrap();
        assert!(limits(&bare, &none).unwrap_err().ends_with("has no value."));
        let range = parse(&words("--allow 192.168.1.0/40")).unwrap();
        assert!(allowed(&range, &none).is_err());
    }

    #[test]
    fn a_good_declaration_is_read() {
        let declared = declaration(
            &words("--tag isp=telia --location 55.5,8.5"),
            &none,
            &default_file(),
            &missing,
        )
        .unwrap();
        assert_eq!(declared.tags["isp"], "telia");
        assert_eq!(declared.location.unwrap().precision, "city");
    }

    #[test]
    fn a_bad_tag_or_coordinate_is_reported_with_its_sentence() {
        assert_eq!(
            declaration(&words("--tag Isp=Telia"), &none, &default_file(), &missing).unwrap_err(),
            "The tag \u{201C}Isp=Telia\u{201D} isn't valid. A key is 1 to 63 characters: lowercase letters, digits, \".\", \"-\" and \"_\", starting with a letter."
        );
        assert_eq!(
            declaration(&words("--location 95,8"), &none, &default_file(), &missing).unwrap_err(),
            "The location \u{201C}95,8\u{201D} isn't valid. A latitude runs from -90 to 90."
        );
        let env = |name: &str| (name == "RVRS_NODE_TAGS").then(|| "a=b,C=d".to_string());
        assert!(declaration(&[], &env, &default_file(), &missing)
            .unwrap_err()
            .ends_with("It came from RVRS_NODE_TAGS."));
    }

    #[test]
    fn the_file_sits_under_the_environment_and_the_flags() {
        let read = |_: &Path| Ok(r#"{"tags":{"isp":"tdc","site":"lab"}}"#.to_string());
        let env = |name: &str| (name == "RVRS_NODE_TAGS").then(|| "isp=telia".to_string());
        let declared =
            declaration(&words("--tag site=roof"), &env, &default_file(), &read).unwrap();
        assert_eq!(declared.tags["isp"], "telia");
        assert_eq!(declared.tags["site"], "roof");
    }

    #[test]
    fn a_file_the_person_named_has_to_be_there() {
        // The default file may be missing. One named by a flag or the environment may not.
        assert!(declaration(&[], &none, &default_file(), &missing).is_ok());
        let error = declaration(
            &words("--config /etc/rvrs/node.json"),
            &none,
            &default_file(),
            &missing,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Couldn't read the node's file /etc/rvrs/node.json. No such file."
        );
        let env = |name: &str| (name == ENV_CONFIG).then(|| "/etc/rvrs/node.json".to_string());
        assert!(declaration(&[], &env, &default_file(), &missing).is_err());
        // A file that is there and isn't a node file says so, by name.
        let read = |_: &Path| Ok("not json".to_string());
        assert!(declaration(&[], &none, &default_file(), &read)
            .unwrap_err()
            .starts_with("/nowhere/node.json isn't a node file."));
    }
}
