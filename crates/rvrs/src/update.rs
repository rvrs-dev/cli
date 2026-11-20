//! `rvrs update`: one command that brings rvrs up to date, however it got here.
//!
//! - A binary from the installer or a release replaces itself with the newest release, in place.
//! - A build from a checkout pulls the new commits and builds them.
//! - A Homebrew install is Homebrew's to upgrade, and rvrs says so.
//!
//! Between updates rvrs looks for a newer version at most once a day, from a process of its own
//! so no run ever waits for it, and says so in one line after a result. `RVRS_UPDATE_CHECK=0`
//! turns the look off. The look sends nothing but the request itself.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use rvrs_core::{format, paths};

use crate::help::{COMMIT, VERSION};
use crate::style::{ColorChoice, Style, SPINNER};
use crate::{fail, usage};

const REPO: &str = "rvrs-dev/cli";
/// How long a look for a newer version stays good.
const CHECK_EVERY: u64 = 24 * 3600;

/// How this rvrs got onto this device, which decides how it is updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// Built from a git checkout: the repo's folder, and whether it is a release build.
    Checkout { repo: PathBuf, release: bool },
    /// Installed by Homebrew.
    Homebrew,
    /// A release binary, as the installer puts it in place.
    Binary { path: PathBuf },
}

/// Tells the kinds apart by where the running binary lives.
pub fn install_of(executable: &Path) -> Install {
    let text = executable.to_string_lossy();
    if text.contains("/Cellar/") || text.contains("/homebrew/") {
        return Install::Homebrew;
    }
    // target/<profile>/rvrs inside a folder that is a git checkout with a Cargo.toml.
    let mut folder = executable.parent();
    while let Some(current) = folder {
        if current.file_name().is_some_and(|name| name == "target") {
            if let Some(repo) = current.parent() {
                if repo.join(".git").exists() && repo.join("Cargo.toml").exists() {
                    return Install::Checkout {
                        repo: repo.to_path_buf(),
                        release: text.contains("/release/"),
                    };
                }
            }
        }
        folder = current.parent();
    }
    Install::Binary {
        path: executable.to_path_buf(),
    }
}

fn running_binary() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.canonicalize().ok())
}

/// "0.2.14" as numbers, so 0.2.9 sorts before 0.2.14. A leading v is fine.
pub fn parse_version(text: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = text
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse().ok())
        .collect();
    parts.filter(|parts| !parts.is_empty())
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// The release archive for this device, named as the installer names it.
pub fn target() -> Option<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-musl",
        _ => return None,
    };
    Some(format!("{arch}-{os}"))
}

fn git(repo: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn http_get(url: &str, seconds: u64) -> Option<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(seconds)))
        .user_agent(format!("rvrs/{VERSION}"))
        .build()
        .into();
    let mut response = agent.get(url).call().ok()?;
    response.body_mut().read_to_string().ok()
}

/// The newest release's version, from GitHub. None when there is no release to see.
fn latest_release() -> Option<String> {
    let body = http_get(
        &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        8,
    )?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    parse_version(tag).map(|_| tag.trim_start_matches('v').to_string())
}

// The daily look

/// What the last look found, kept in the cache folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub checked_at: u64,
    /// A newer version, or for a checkout the number of commits it is behind.
    pub newer: Option<String>,
}

fn cache_file() -> PathBuf {
    paths::cache_dir().join("update.json")
}

pub fn read_found(path: &Path) -> Option<Found> {
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some(Found {
        checked_at: json.get("checked_at")?.as_u64()?,
        newer: json
            .get("newer")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    })
}

fn write_found(path: &Path, found: &Found) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json =
        serde_json::json!({ "checked_at": found.checked_at, "newer": found.newer, "for": VERSION });
    let _ = std::fs::write(path, json.to_string());
}

fn checks_enabled() -> bool {
    std::env::var("RVRS_UPDATE_CHECK").map_or(true, |value| value != "0")
}

/// Looks for a newer version now and remembers the answer. This is the hidden
/// `rvrs __update-check`, run as a process of its own.
pub fn check_now() -> ExitCode {
    let newer = match running_binary().map(|path| install_of(&path)) {
        Some(Install::Checkout { repo, .. }) => {
            // Ask the remote without touching the checkout.
            let fetched = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["fetch", "--quiet"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            fetched
                .then(|| git(&repo, &["rev-list", "--count", "HEAD..@{u}"]))
                .flatten()
                .filter(|behind| behind != "0")
                .map(|behind| format!("commits:{behind}"))
        }
        Some(Install::Binary { .. }) => latest_release().filter(|latest| is_newer(latest, VERSION)),
        _ => None,
    };
    write_found(
        &cache_file(),
        &Found {
            checked_at: format::now(),
            newer,
        },
    );
    ExitCode::SUCCESS
}

/// Starts the daily look when one is due. It runs in a process of its own with nothing
/// attached, so the command that started it never waits and may end first.
pub fn look_when_due() {
    if !checks_enabled()
        || running_binary().map(|path| install_of(&path)) == Some(Install::Homebrew)
    {
        return;
    }
    let due = read_found(&cache_file())
        .is_none_or(|found| format::now().saturating_sub(found.checked_at) >= CHECK_EVERY);
    if !due {
        return;
    }
    // Mark the look as made first, so a burst of commands starts one process and not twenty.
    let known = read_found(&cache_file()).and_then(|found| found.newer);
    write_found(
        &cache_file(),
        &Found {
            checked_at: format::now(),
            newer: known,
        },
    );
    if let Ok(executable) = std::env::current_exe() {
        let _ = Command::new(executable)
            .arg("__update-check")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// The one line that says a newer version is there, from what the last look found. None when
/// rvrs is current, the look is off, or the answer was for an older build of rvrs.
pub fn notice() -> Option<String> {
    if !checks_enabled() {
        return None;
    }
    let path = cache_file();
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    if json.get("for").and_then(|value| value.as_str()) != Some(VERSION) {
        return None;
    }
    notice_for(read_found(&path)?.newer.as_deref())
}

pub fn notice_for(newer: Option<&str>) -> Option<String> {
    let newer = newer?;
    if parse_version(newer).is_some() {
        return Some(format!("rvrs {newer} is out. Install it with rvrs update."));
    }
    // A checkout counts the commits on its remote that this build doesn't have.
    let count: u64 = newer.strip_prefix("commits:")?.parse().ok()?;
    Some(if count == 1 {
        "The remote has 1 commit this build doesn't. Pull and build it with rvrs update."
            .to_string()
    } else {
        format!(
            "The remote has {count} commits this build doesn't. Pull and build them with rvrs update."
        )
    })
}

/// What the daily look found, as a fact for a status line: "0.3.1 is out", "4 new commits".
pub fn newer_fact() -> Option<String> {
    if !checks_enabled() {
        return None;
    }
    let path = cache_file();
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    if json.get("for").and_then(|value| value.as_str()) != Some(VERSION) {
        return None;
    }
    let newer = read_found(&path)?.newer?;
    if parse_version(&newer).is_some() {
        return Some(format!("{newer} is out"));
    }
    match newer.strip_prefix("commits:")?.parse::<u64>().ok()? {
        1 => Some("1 new commit".to_string()),
        count => Some(format!("{count} new commits")),
    }
}

/// After a tool has printed its result on a terminal: say that a newer rvrs is there, and
/// start the daily look if one is due.
pub fn after_command() {
    if !std::io::stderr().is_terminal() || !std::io::stdout().is_terminal() {
        return;
    }
    if let Some(line) = notice() {
        eprintln!("\n{}", Style::detect_stderr().dim(&line));
    }
    look_when_due();
}

// The command

pub fn run(arguments: &[String]) -> ExitCode {
    let mut check_only = false;
    for argument in arguments {
        match argument.as_str() {
            "--check" => check_only = true,
            other => {
                return usage(&format!(
                    "rvrs update takes --check, and \u{201C}{other}\u{201D} isn't it."
                ))
            }
        }
    }
    let style = Style::detect(ColorChoice::Auto);
    let Some(executable) = running_binary() else {
        return fail("Couldn't tell where this rvrs is installed.");
    };
    match install_of(&executable) {
        Install::Homebrew => {
            println!("This rvrs was installed with Homebrew. brew upgrade rvrs updates it.");
            ExitCode::SUCCESS
        }
        Install::Checkout { repo, release } => update_checkout(&repo, release, check_only, style),
        Install::Binary { path } => update_binary(&path, check_only, style),
    }
}

fn current() -> String {
    format!("rvrs {VERSION} ({COMMIT})")
}

/// A build from a checkout: fetch, show what is new, fast-forward, build.
fn update_checkout(repo: &Path, release: bool, check_only: bool, style: Style) -> ExitCode {
    let shown = paths::display(repo);
    let Some(branch) = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]) else {
        return fail(&format!("{shown} isn't a git checkout rvrs can read."));
    };
    if git(repo, &["rev-parse", "--abbrev-ref", "@{u}"]).is_none() {
        return fail(&format!("The branch {branch} in {shown} follows no remote branch, so there is nothing to pull from."));
    }
    let fetched = with_spinner(style, "Looking for new commits", || {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["fetch", "--quiet"])
            .stdin(Stdio::null())
            .output()
    });
    match fetched {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            return fail(&format!(
                "Couldn't reach the remote of {shown}. git said: {}",
                last_line(&output.stderr)
            ));
        }
        Err(error) => return fail(&format!("Couldn't run git ({error}).")),
    }
    let behind: usize = git(repo, &["rev-list", "--count", "HEAD..@{u}"])
        .and_then(|count| count.parse().ok())
        .unwrap_or(0);
    if behind == 0 {
        write_found(
            &cache_file(),
            &Found {
                checked_at: format::now(),
                newer: None,
            },
        );
        println!("{} is up to date.", current());
        return ExitCode::SUCCESS;
    }
    let noun = if behind == 1 { "commit" } else { "commits" };
    println!(
        "{} {behind} new {noun} since {}",
        style.bold("//"),
        current()
    );
    let subjects = git(repo, &["log", "--format=%s", "HEAD..@{u}"]).unwrap_or_default();
    for subject in subjects.lines().take(8) {
        println!("  {subject}");
    }
    if behind > 8 {
        println!("  {}", style.dim(&format!("and {} more", behind - 8)));
    }
    if check_only {
        println!("\nPull and build them with rvrs update.");
        return ExitCode::SUCCESS;
    }
    if git(repo, &["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|status| !status.is_empty())
    {
        return fail(&format!(
            "{shown} has uncommitted changes, so rvrs left it alone. Commit or stash them and run rvrs update again."
        ));
    }
    if git(repo, &["merge", "--ff-only", "--quiet", "@{u}"]).is_none() {
        return fail(&format!(
            "{shown} has commits of its own that the remote doesn't, so rvrs left it alone. Rebase or merge there, then build with cargo build --release."
        ));
    }
    println!();
    let mut build = Command::new("cargo");
    build.arg("build").current_dir(repo).stdin(Stdio::null());
    if release {
        build.arg("--release");
    }
    let built = with_spinner(style, "Building", || build.output());
    match built {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let _ = std::io::stderr().write_all(&output.stderr);
            return fail("The build failed, so the rvrs you have is unchanged. cargo's output is above.");
        }
        Err(error) => return fail(&format!("Couldn't run cargo ({error}). The new commits are pulled. cargo build --release builds them.")),
    }
    write_found(
        &cache_file(),
        &Found {
            checked_at: format::now(),
            newer: None,
        },
    );
    report_new_version(style);
    ExitCode::SUCCESS
}

/// A release binary: download the newest archive for this device and put it where this one is.
fn update_binary(path: &Path, check_only: bool, style: Style) -> ExitCode {
    let Some(target) = target() else {
        return fail(&format!(
            "There is no rvrs release for {} on {}.",
            std::env::consts::ARCH,
            std::env::consts::OS
        ));
    };
    // A local archive stands in for the download, which is how this is tested.
    let local = std::env::var_os("RVRS_UPDATE_ARCHIVE").map(PathBuf::from);
    if local.is_none() {
        let latest = with_spinner(style, "Looking for a newer release", latest_release);
        let Some(latest) = latest else {
            return fail(&format!(
                "Couldn't get the newest release from github.com/{REPO}/releases. Either none is published yet or GitHub didn't answer."
            ));
        };
        if !is_newer(&latest, VERSION) {
            write_found(
                &cache_file(),
                &Found {
                    checked_at: format::now(),
                    newer: None,
                },
            );
            println!("{} is up to date.", current());
            return ExitCode::SUCCESS;
        }
        println!(
            "{} rvrs {latest} is out. You have {VERSION}.",
            style.bold("//")
        );
        if check_only {
            println!("Install it with rvrs update.");
            return ExitCode::SUCCESS;
        }
    } else if check_only {
        println!("RVRS_UPDATE_ARCHIVE is set, so there is no release to compare with.");
        return ExitCode::SUCCESS;
    }

    let Some(folder) = path.parent() else {
        return fail("Couldn't tell which folder rvrs is in.");
    };
    let work = std::env::temp_dir().join(format!("rvrs-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    if let Err(error) = std::fs::create_dir_all(&work) {
        return fail(&format!("Couldn't make a folder to work in ({error})."));
    }
    let archive = match &local {
        Some(local) => local.clone(),
        None => {
            let url =
                format!("https://github.com/{REPO}/releases/latest/download/rvrs-{target}.tar.gz");
            let archive = work.join("rvrs.tar.gz");
            let downloaded = with_spinner(style, "Downloading", || download(&url, &archive));
            if let Err(message) = downloaded {
                let _ = std::fs::remove_dir_all(&work);
                return fail(&message);
            }
            archive
        }
    };
    let unpacked = Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(&work)
        .stdin(Stdio::null())
        .output();
    let fresh = work.join("rvrs");
    if !unpacked.is_ok_and(|output| output.status.success()) || !fresh.is_file() {
        let _ = std::fs::remove_dir_all(&work);
        return fail("The download has no rvrs binary in it, so the rvrs you have is unchanged.");
    }
    // The new binary has to run here before it takes the old one's place.
    let version = Command::new(&fresh)
        .arg("--version")
        .stdin(Stdio::null())
        .output();
    let Some(version) = version
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    else {
        let _ = std::fs::remove_dir_all(&work);
        return fail(
            "The downloaded rvrs doesn't run on this device, so the rvrs you have is unchanged.",
        );
    };
    // Written beside the old binary first and renamed over it, so there is never half an rvrs.
    let staged = folder.join(".rvrs.new");
    let placed = std::fs::copy(&fresh, &staged)
        .and_then(|_| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        })
        .and_then(|()| std::fs::rename(&staged, path));
    let _ = std::fs::remove_dir_all(&work);
    if let Err(error) = placed {
        let _ = std::fs::remove_file(&staged);
        return fail(&match error.kind() {
            std::io::ErrorKind::PermissionDenied => {
                format!(
                    "rvrs can't write to {}. sudo rvrs update can.",
                    paths::display(folder)
                )
            }
            _ => format!(
                "Couldn't put the new rvrs in place ({error}). The rvrs you have is unchanged."
            ),
        });
    }
    write_found(
        &cache_file(),
        &Found {
            checked_at: format::now(),
            newer: None,
        },
    );
    println!(
        "{} {}",
        style.bold_green("Updated"),
        format!("{} \u{2192} {version}", current())
    );
    ExitCode::SUCCESS
}

fn download(url: &str, to: &Path) -> Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .user_agent(format!("rvrs/{VERSION}"))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|_| format!("Couldn't download {url}."))?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(200 * 1024 * 1024)
        .read_to_vec()
        .map_err(|_| format!("Couldn't download {url}."))?;
    std::fs::write(to, bytes).map_err(|error| format!("Couldn't save the download ({error})."))
}

/// Says what the rebuilt binary calls itself, next to what this one is.
fn report_new_version(style: Style) {
    let fresh = std::env::current_exe()
        .ok()
        .and_then(|path| {
            Command::new(path)
                .arg("--version")
                .stdin(Stdio::null())
                .output()
                .ok()
        })
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());
    match fresh {
        Some(fresh) => println!(
            "{} {} \u{2192} {fresh}",
            style.bold_green("Updated"),
            current()
        ),
        None => println!("{} from {}", style.bold_green("Updated"), current()),
    }
}

fn last_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .last()
        .unwrap_or("nothing")
        .trim()
        .to_string()
}

/// Runs the work on a thread and spins beside its label until it is done. Without a terminal
/// it runs without one.
fn with_spinner<T: Send>(style: Style, label: &str, work: impl FnOnce() -> T + Send) -> T {
    if !std::io::stderr().is_terminal() {
        return work();
    }
    std::thread::scope(|scope| {
        let handle = scope.spawn(work);
        let started = Instant::now();
        let mut frame = 0;
        while !handle.is_finished() {
            let seconds = started.elapsed().as_secs();
            let elapsed = if seconds >= 3 {
                format!(" \u{00B7} {seconds} s")
            } else {
                String::new()
            };
            eprint!(
                "\r{} {}\x1b[K",
                style.blue(SPINNER[frame % SPINNER.len()]),
                style.dim(&format!("{label}{elapsed}"))
            );
            let _ = std::io::stderr().flush();
            frame += 1;
            std::thread::sleep(Duration::from_millis(80));
        }
        eprint!("\r\x1b[K");
        let _ = std::io::stderr().flush();
        handle.join().expect("the update step panicked")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.2.14", "0.2.9"));
        assert!(is_newer("v0.3.0", "0.2.99"));
        assert!(!is_newer("0.2.9", "0.2.9"));
        assert!(!is_newer("0.2.8", "0.2.9"));
        assert!(!is_newer("nightly", "0.2.9"));
        assert_eq!(parse_version("v1.2.3"), Some(vec![1, 2, 3]));
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn where_rvrs_lives_says_how_it_is_updated() {
        assert_eq!(
            install_of(Path::new("/opt/homebrew/Cellar/rvrs/0.2.1/bin/rvrs")),
            Install::Homebrew
        );
        assert_eq!(
            install_of(Path::new("/home/me/.local/bin/rvrs")),
            Install::Binary {
                path: PathBuf::from("/home/me/.local/bin/rvrs")
            }
        );
        // A target folder outside a checkout is no checkout.
        assert!(matches!(
            install_of(Path::new("/tmp/nowhere/target/release/rvrs")),
            Install::Binary { .. }
        ));
        // A binary in the target folder of a git checkout is a build from that checkout.
        let repo = std::env::temp_dir().join(format!("rvrs-test-checkout-{}", std::process::id()));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("target/release")).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "").unwrap();
        assert_eq!(
            install_of(&repo.join("target/release/rvrs")),
            Install::Checkout {
                repo: repo.clone(),
                release: true
            }
        );
        assert_eq!(
            install_of(&repo.join("target/debug/rvrs")),
            Install::Checkout {
                repo: repo.clone(),
                release: false
            }
        );
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn the_notice_says_what_is_new_and_how_to_get_it() {
        assert_eq!(notice_for(None), None);
        assert_eq!(
            notice_for(Some("0.3.1")).unwrap(),
            "rvrs 0.3.1 is out. Install it with rvrs update."
        );
        assert_eq!(
            notice_for(Some("commits:4")).unwrap(),
            "The remote has 4 commits this build doesn't. Pull and build them with rvrs update."
        );
        assert_eq!(
            notice_for(Some("commits:1")).unwrap(),
            "The remote has 1 commit this build doesn't. Pull and build it with rvrs update."
        );
        assert_eq!(notice_for(Some("nonsense")), None);
    }

    #[test]
    fn what_a_look_found_survives_a_reload() {
        let path =
            std::env::temp_dir().join(format!("rvrs-test-update-{}.json", std::process::id()));
        write_found(
            &path,
            &Found {
                checked_at: 42,
                newer: Some("0.9.0".into()),
            },
        );
        assert_eq!(
            read_found(&path),
            Some(Found {
                checked_at: 42,
                newer: Some("0.9.0".into())
            })
        );
        write_found(
            &path,
            &Found {
                checked_at: 43,
                newer: None,
            },
        );
        assert_eq!(
            read_found(&path),
            Some(Found {
                checked_at: 43,
                newer: None
            })
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn there_is_a_release_name_for_this_device() {
        let target = target().unwrap();
        assert!(target.ends_with("apple-darwin") || target.ends_with("unknown-linux-musl"));
    }
}
