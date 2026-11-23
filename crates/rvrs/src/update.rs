//! `rvrs update`: one command that brings rvrs up to date, however it got here.
//!
//! - A binary from the installer or a release replaces itself with the newest release, in place.
//! - A build from a checkout pulls the new commits and builds them.
//! - A Homebrew install is Homebrew's to upgrade, and rvrs says so.
//!
//! An update keeps the build features (`features.rs`): it installs the build that leaves out what
//! this one leaves out, and nothing more, so a build feature added since comes with it. `--with` and `--without`
//! change that set. A release without the matching build leaves rvrs as it is, since a router
//! with little flash must never get a bigger build than the one it has.
//!
//! Between updates rvrs looks for a newer version at most once a day, from a process of its own
//! so no run ever waits for it, and says so in one line after a result. `RVRS_UPDATE_CHECK=0`
//! turns the look off. The look sends nothing but the request itself.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use rvrs_core::{format, paths};

use crate::features;
use crate::help::VERSION;
use crate::style::{ColorChoice, Style, SPINNER};
use crate::{fail, usage};

const REPO: &str = "rvrs-dev/cli";
/// How long a look for a newer version stays good.
const CHECK_EVERY: u64 = 24 * 3600;

/// How this rvrs got onto this device, which decides how it is updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Install {
    /// Built from a git checkout: the repo's folder and how the running binary was built, so an
    /// update builds the same binary again.
    Checkout { repo: PathBuf, build: CheckoutBuild },
    /// Installed by Homebrew.
    Homebrew,
    /// Installed from a .deb or .rpm, into /usr/bin. The package manager owns the file.
    Package,
    /// A release binary, as the installer puts it in place.
    Binary { path: PathBuf },
}

/// How a checkout's binary was built, read off its path: `target/[<target>/]<profile>/rvrs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutBuild {
    /// The Cargo profile: `dev` for `target/debug`, else the folder's name (`release`, `small`).
    pub profile: String,
    /// The target triple when it was built with `--target`.
    pub target: Option<String>,
}

impl CheckoutBuild {
    /// Reads the folders between `target` and the binary: `release`, or
    /// `armv7-unknown-linux-musleabihf/small`.
    fn of(folders: &[String]) -> Self {
        let (target, profile) = match folders {
            [target, profile] => (Some(target.clone()), profile.clone()),
            [.., profile] => (None, profile.clone()),
            [] => (None, "dev".to_string()),
        };
        let profile = if profile == "debug" {
            "dev".to_string()
        } else {
            profile
        };
        Self { profile, target }
    }

    /// The arguments after `cargo build` that put the binary where this one is.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = match self.profile.as_str() {
            "dev" => vec![],
            "release" => vec!["--release".into()],
            other => vec!["--profile".into(), other.into()],
        };
        if let Some(target) = &self.target {
            args.extend(["--target".into(), target.clone()]);
        }
        args
    }

    /// The command as a person types it: "cargo build --profile small --target …".
    fn command(&self) -> String {
        std::iter::once("cargo build".to_string())
            .chain(self.args())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Tells the kinds apart by where the running binary lives.
pub fn install_of(executable: &Path) -> Install {
    let text = executable.to_string_lossy();
    if text.contains("/Cellar/") || text.contains("/homebrew/") {
        return Install::Homebrew;
    }
    if text == "/usr/bin/rvrs" {
        return Install::Package;
    }
    // target/[<target>/]<profile>/rvrs inside a folder that is a git checkout with a Cargo.toml.
    let mut folder = executable.parent();
    while let Some(current) = folder {
        if current.file_name().is_some_and(|name| name == "target") {
            if let Some(repo) = current.parent() {
                if repo.join(".git").exists() && repo.join("Cargo.toml").exists() {
                    let folders: Vec<String> = executable
                        .parent()
                        .and_then(|parent| parent.strip_prefix(current).ok())
                        .map(|inside| {
                            inside
                                .components()
                                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                                .collect()
                        })
                        .unwrap_or_default();
                    return Install::Checkout {
                        repo: repo.to_path_buf(),
                        build: CheckoutBuild::of(&folders),
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

/// The release target for this device, named as the installer names it.
pub fn target() -> Option<String> {
    target_for(std::env::consts::ARCH, std::env::consts::OS)
}

fn target_for(arch: &str, os: &str) -> Option<String> {
    match (arch, os) {
        ("x86_64" | "aarch64", "macos") => Some(format!("{arch}-apple-darwin")),
        ("x86_64" | "aarch64", "linux") => Some(format!("{arch}-unknown-linux-musl")),
        // 32-bit ARM routers and boards: ARMv7 with hardware floating point.
        ("arm", "linux") => Some("armv7-unknown-linux-musleabihf".to_string()),
        _ => None,
    }
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
        || running_binary()
            .map(|path| install_of(&path))
            .is_some_and(|install| matches!(install, Install::Homebrew | Install::Package))
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
#[cfg_attr(not(feature = "console"), allow(dead_code))]
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

/// What `rvrs update` was asked: `--check`, and the build features `--with` adds back and `--without`
/// leaves out.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Asked {
    pub check_only: bool,
    pub with: Vec<&'static str>,
    pub without: Vec<&'static str>,
}

pub fn parse(arguments: &[String]) -> Result<Asked, String> {
    let mut asked = Asked::default();
    let mut words = arguments.iter();
    while let Some(argument) = words.next() {
        let (flag, inline) = match argument.split_once('=') {
            Some((flag, value)) => (flag, Some(value.to_string())),
            None => (argument.as_str(), None),
        };
        match flag {
            "--check" if inline.is_none() => asked.check_only = true,
            "--with" | "--without" => {
                let Some(value) = inline.or_else(|| words.next().cloned()) else {
                    return Err(format!("{flag} takes the build features to change, as in {flag} console."));
                };
                let names = features::parse_list(&value)?;
                if flag == "--with" {
                    asked.with.extend(names);
                } else {
                    asked.without.extend(names);
                }
            }
            _ => {
                return Err(format!(
                    "rvrs update takes --check, --with and --without, and \u{201C}{argument}\u{201D} is none of them."
                ))
            }
        }
    }
    if let Some(both) = asked.with.iter().find(|name| asked.without.contains(name)) {
        return Err(format!(
            "{both} is in both --with and --without. Name it in one."
        ));
    }
    Ok(asked)
}

/// The build an update installs: what it leaves out, and how that differs from this one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wanted {
    left_out: Vec<&'static str>,
    /// Build features this build has and the update leaves out.
    removed: Vec<&'static str>,
    /// Build features this build leaves out and the update has.
    added: Vec<&'static str>,
}

impl Wanted {
    fn of(asked: &Asked) -> Self {
        Self::from(&features::left_out(), asked)
    }

    fn from(current: &[&'static str], asked: &Asked) -> Self {
        let left_out = features::wanted(current, &asked.with, &asked.without);
        let removed = left_out
            .iter()
            .copied()
            .filter(|name| !current.contains(name))
            .collect();
        let added = current
            .iter()
            .copied()
            .filter(|name| !left_out.contains(name))
            .collect();
        Self {
            left_out,
            removed,
            added,
        }
    }

    fn changes(&self) -> bool {
        !self.removed.is_empty() || !self.added.is_empty()
    }

    /// "This update leaves out console.", "This update adds console back."
    fn change_words(&self) -> String {
        match (self.removed.as_slice(), self.added.as_slice()) {
            ([], added) => format!("This update adds {} back.", features::and_list(added)),
            (removed, []) => format!("This update leaves out {}.", features::and_list(removed)),
            (removed, added) => format!(
                "This update leaves out {} and adds {} back.",
                features::and_list(removed),
                features::and_list(added)
            ),
        }
    }
}

pub fn run(arguments: &[String]) -> ExitCode {
    let asked = match parse(arguments) {
        Ok(asked) => asked,
        Err(message) => return usage(&message),
    };
    let wanted = Wanted::of(&asked);
    let style = Style::detect(ColorChoice::Auto);
    let Some(executable) = running_binary() else {
        return fail("Couldn't tell where this rvrs is installed.");
    };
    match install_of(&executable) {
        Install::Homebrew if wanted.changes() => fail(&format!(
            "Homebrew installs the full build, so it can't leave build features out. The installer can: curl -fsSL https://rvrs.dev/install.sh | sh -s -- --without {}",
            wanted.left_out.join(",")
        )),
        Install::Package if wanted.changes() => fail(&format!(
            "The .deb and .rpm hold the full build, so they can't leave build features out. The installer can: curl -fsSL https://rvrs.dev/install.sh | sh -s -- --without {}",
            wanted.left_out.join(",")
        )),
        Install::Homebrew => {
            println!("This rvrs was installed with Homebrew. Update it with brew upgrade rvrs.");
            ExitCode::SUCCESS
        }
        Install::Package => {
            println!("This rvrs was installed from a package. Update it with the newest .deb or .rpm from github.com/{REPO}/releases.");
            ExitCode::SUCCESS
        }
        Install::Checkout { repo, build } => {
            update_checkout(&repo, &build, asked.check_only, &wanted, style)
        }
        Install::Binary { path } => update_binary(&path, asked.check_only, &wanted, style),
    }
}

/// This build as `rvrs --version` names it, with what it leaves out.
fn current() -> String {
    crate::help::version()
}

/// A build from a checkout: fetch, show what is new, fast-forward, build.
fn update_checkout(
    repo: &Path,
    how: &CheckoutBuild,
    check_only: bool,
    wanted: &Wanted,
    style: Style,
) -> ExitCode {
    let shown = paths::display(repo);
    let Some(branch) = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]) else {
        return fail(&format!("{shown} isn't a git checkout rvrs can read."));
    };
    // A change of build features builds this checkout as it is, so it needs no remote.
    let follows = git(repo, &["rev-parse", "--abbrev-ref", "@{u}"]).is_some();
    if !follows && !wanted.changes() {
        return fail(&format!("The branch {branch} in {shown} follows no remote branch, so there is nothing to pull from."));
    }
    let mut behind: usize = 0;
    if follows {
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
        behind = git(repo, &["rev-list", "--count", "HEAD..@{u}"])
            .and_then(|count| count.parse().ok())
            .unwrap_or(0);
    }
    if behind == 0 && !wanted.changes() {
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
    if behind > 0 {
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
    }
    if wanted.changes() {
        println!("{} {}", style.bold("//"), wanted.change_words());
    }
    if check_only {
        if behind > 0 {
            println!("\nPull and build with rvrs update.");
        } else {
            println!("\nBuild it with rvrs update.");
        }
        return ExitCode::SUCCESS;
    }
    if behind > 0 {
        if git(repo, &["status", "--porcelain", "--untracked-files=no"])
            .is_some_and(|status| !status.is_empty())
        {
            return fail(&format!(
                "{shown} has uncommitted changes, so rvrs left it alone. Commit or stash them and run rvrs update again."
            ));
        }
        if git(repo, &["merge", "--ff-only", "--quiet", "@{u}"]).is_none() {
            return fail(&format!(
                "{shown} has commits of its own that the remote doesn't, so rvrs left it alone. Rebase or merge there, then build with {}.",
                how.command()
            ));
        }
    }
    println!();
    // The profile and the target the running binary was built with, so the build lands where
    // it is and replaces it.
    let mut build = Command::new("cargo");
    build
        .arg("build")
        .args(how.args())
        .current_dir(repo)
        .stdin(Stdio::null());
    // The build features: the pulled Cargo.toml's default features, less what this build leaves
    // out, so one added in the new commits is built in.
    if !wanted.left_out.is_empty() {
        let manifest =
            std::fs::read_to_string(repo.join("crates/rvrs/Cargo.toml")).unwrap_or_default();
        let Some(default) = features::default_features(&manifest) else {
            return fail(&format!(
                "crates/rvrs/Cargo.toml in {shown} names no default features, so rvrs can't tell what to build."
            ));
        };
        build.args(["-p", "rvrs", "--no-default-features", "--features"]);
        build.arg(features::cargo_features(&default, &wanted.left_out).join(","));
    }
    let built = with_spinner(style, "Building", || build.output());
    match built {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let _ = std::io::stderr().write_all(&output.stderr);
            return fail(
                "The build failed, so the rvrs you have is unchanged. cargo's output is above.",
            );
        }
        Err(error) => {
            return fail(&format!(
                "Couldn't run cargo ({error}). The new commits are pulled. {} builds them.",
                how.command()
            ))
        }
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
fn update_binary(path: &Path, check_only: bool, wanted: &Wanted, style: Style) -> ExitCode {
    let Some(target) = target() else {
        return fail(&format!(
            "There is no rvrs release for {} on {}.",
            std::env::consts::ARCH,
            std::env::consts::OS
        ));
    };
    // A local archive stands in for the download, which is how this is tested.
    let local = std::env::var_os("RVRS_UPDATE_ARCHIVE").map(PathBuf::from);
    let mut release = None;
    if local.is_none() {
        let latest = with_spinner(style, "Looking for a newer release", latest_release);
        let Some(latest) = latest else {
            return fail(&format!(
                "Couldn't get the newest release from github.com/{REPO}/releases. Either none is published yet or GitHub didn't answer."
            ));
        };
        let newer = is_newer(&latest, VERSION);
        if !newer && !wanted.changes() {
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
        if newer {
            println!(
                "{} rvrs {latest} is out. You have {VERSION}.",
                style.bold("//")
            );
        }
        if wanted.changes() {
            println!("{} {}", style.bold("//"), wanted.change_words());
        }
        if check_only {
            println!("Install it with rvrs update.");
            return ExitCode::SUCCESS;
        }
        release = Some(latest);
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
            // The checksums first: they say which builds the release has.
            let name = features::archive(&target, &wanted.left_out);
            let base = format!("https://github.com/{REPO}/releases/latest/download");
            let sums = work.join("SHA256SUMS");
            if let Err(message) = with_spinner(style, "Downloading", || {
                download(&format!("{base}/SHA256SUMS"), &sums)
            }) {
                let _ = std::fs::remove_dir_all(&work);
                return fail(&message);
            }
            let sums_text = std::fs::read_to_string(&sums).unwrap_or_default();
            if checksum_for(&sums_text, &name).is_none() {
                let _ = std::fs::remove_dir_all(&work);
                return fail(&no_such_build(
                    release.as_deref().unwrap_or("The newest release"),
                    &target,
                    wanted,
                    &sums_text,
                ));
            }
            let archive = work.join("rvrs.tar.gz");
            if let Err(message) = with_spinner(style, "Downloading", || {
                download(&format!("{base}/{name}"), &archive)
            }) {
                let _ = std::fs::remove_dir_all(&work);
                return fail(&message);
            }
            let expected = checksum_for(&sums_text, &name);
            let actual = std::fs::read(&archive)
                .ok()
                .map(|bytes| hex(&sha256(&bytes)));
            match (expected, actual) {
                (Some(expected), Some(actual)) if expected == actual => {}
                (None, _) => {
                    let _ = std::fs::remove_dir_all(&work);
                    return fail(&format!("The release lists no checksum for {name}, so the rvrs you have is unchanged."));
                }
                _ => {
                    let _ = std::fs::remove_dir_all(&work);
                    return fail(&format!(
                        "{name} doesn't match its checksum, so the rvrs you have is unchanged."
                    ));
                }
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
        "{} {} \u{2192} {version}",
        style.bold_green("Updated"),
        current()
    );
    ExitCode::SUCCESS
}

/// Why an update stopped: the release has no build that leaves out what this one does. A
/// bigger build never takes its place unasked.
fn no_such_build(release: &str, target: &str, wanted: &Wanted, sums: &str) -> String {
    let release = if release.starts_with(|c: char| c.is_ascii_digit()) {
        format!("rvrs {release}")
    } else {
        release.to_string()
    };
    let builds = features::builds_in(sums, target);
    let mut text = format!(
        "{release} has no build without {} for {target}, so the rvrs you have is unchanged. It has {}.",
        features::and_list(&wanted.left_out),
        features::builds_text(&builds)
    );
    if builds.iter().any(Vec::is_empty) {
        text.push_str(&format!(
            " rvrs update --with {} installs the full build.",
            wanted.left_out.join(",")
        ));
    }
    text
}

/// The checksum `SHA256SUMS` lists for a file, as sha256sum writes it: "<hex>  <name>".
fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (sum, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then(|| sum.to_lowercase())
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// SHA-256 (FIPS 180-4), for the release checksum. Small enough not to need a crate.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    for block in message.chunks(64) {
        let mut w = [0u32; 64];
        for (index, word) in block.chunks(4).enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 32];
    for (index, word) in state.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
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
    fn a_download_is_checked_against_its_sha256() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let long = vec![b'a'; 1_000];
        assert_eq!(
            hex(&sha256(&long)),
            "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
        );
        let sums =
            "AB12  rvrs-x86_64-apple-darwin.tar.gz\ncd34 *rvrs-aarch64-apple-darwin.tar.gz\n";
        assert_eq!(
            checksum_for(sums, "rvrs-x86_64-apple-darwin.tar.gz").as_deref(),
            Some("ab12")
        );
        assert_eq!(
            checksum_for(sums, "rvrs-aarch64-apple-darwin.tar.gz").as_deref(),
            Some("cd34")
        );
        assert_eq!(
            checksum_for(sums, "rvrs-x86_64-unknown-linux-musl.tar.gz"),
            None
        );
        assert_eq!(install_of(Path::new("/usr/bin/rvrs")), Install::Package);
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
        let built = |path: &str| match install_of(&repo.join(path)) {
            Install::Checkout { repo: found, build } if found == repo => Some(build.command()),
            _ => None,
        };
        assert_eq!(
            built("target/release/rvrs").as_deref(),
            Some("cargo build --release")
        );
        assert_eq!(built("target/debug/rvrs").as_deref(), Some("cargo build"));
        // The small build, and builds for another target, are built again the same way, so the
        // update replaces the binary that is running.
        assert_eq!(
            built("target/small/rvrs").as_deref(),
            Some("cargo build --profile small")
        );
        assert_eq!(
            built("target/armv7-unknown-linux-musleabihf/small/rvrs").as_deref(),
            Some("cargo build --profile small --target armv7-unknown-linux-musleabihf")
        );
        assert_eq!(
            built("target/x86_64-unknown-linux-musl/release/rvrs").as_deref(),
            Some("cargo build --release --target x86_64-unknown-linux-musl")
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
        assert_eq!(
            target_for("arm", "linux").as_deref(),
            Some("armv7-unknown-linux-musleabihf")
        );
        assert_eq!(
            target_for("aarch64", "linux").as_deref(),
            Some("aarch64-unknown-linux-musl")
        );
        assert_eq!(target_for("arm", "macos"), None);
        assert_eq!(target_for("mips", "linux"), None);
    }

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn update_reads_check_with_and_without() {
        assert_eq!(parse(&[]).unwrap(), Asked::default());
        let asked = parse(&words("--check --without console")).unwrap();
        assert!(asked.check_only);
        assert_eq!(asked.without, vec!["console"]);
        let asked = parse(&words("--with=bgp,console --without mtu")).unwrap();
        assert_eq!(asked.with, vec!["bgp", "console"]);
        assert_eq!(asked.without, vec!["mtu"]);
        assert!(parse(&words("--without"))
            .unwrap_err()
            .contains("--without console"));
        assert!(parse(&words("--with nope"))
            .unwrap_err()
            .contains("\u{201C}nope\u{201D}"));
        assert!(parse(&words("--with console --without console"))
            .unwrap_err()
            .contains("console is in both"));
        assert!(parse(&words("--force"))
            .unwrap_err()
            .contains("\u{201C}--force\u{201D}"));
    }

    #[test]
    fn the_full_build_updates_to_the_full_build() {
        // This is how every rvrs from before build features updates, since its binary is the full build
        // and asks for rvrs-<target>.tar.gz as it always did.
        let wanted = Wanted::of(&Asked::default());
        assert_eq!(wanted.left_out, Vec::<&str>::new());
        assert!(!wanted.changes());
        assert_eq!(
            features::archive("aarch64-apple-darwin", &wanted.left_out),
            "rvrs-aarch64-apple-darwin.tar.gz"
        );
        let headless = Wanted::of(&parse(&words("--without console")).unwrap());
        assert_eq!(headless.left_out, vec!["console"]);
        assert!(headless.changes());
        assert_eq!(headless.change_words(), "This update leaves out console.");
    }

    #[test]
    fn a_build_that_leaves_out_keeps_leaving_out() {
        let kept = Wanted::from(&["console"], &Asked::default());
        assert_eq!(kept.left_out, vec!["console"]);
        assert!(!kept.changes());
        let back = Wanted::from(&["console"], &parse(&words("--with console")).unwrap());
        assert!(back.left_out.is_empty());
        assert_eq!(back.change_words(), "This update adds console back.");
        let both = Wanted::from(
            &["console"],
            &parse(&words("--with console --without bgp")).unwrap(),
        );
        assert_eq!(
            both.change_words(),
            "This update leaves out bgp and adds console back."
        );
    }

    #[test]
    fn a_release_without_the_build_leaves_rvrs_alone() {
        let wanted = Wanted {
            left_out: vec!["bgp", "console"],
            removed: vec![],
            added: vec![],
        };
        let sums = "ab  rvrs-x86_64-unknown-linux-musl.tar.gz\n\
                    cd  rvrs-x86_64-unknown-linux-musl-without-console.tar.gz\n";
        assert_eq!(
            no_such_build("0.15.0", "x86_64-unknown-linux-musl", &wanted, sums),
            "rvrs 0.15.0 has no build without bgp and console for x86_64-unknown-linux-musl, so the rvrs you have is unchanged. It has the full build and the one without console. rvrs update --with bgp,console installs the full build."
        );
    }
}
