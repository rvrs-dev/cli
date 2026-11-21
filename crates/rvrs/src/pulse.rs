//! The anonymous usage pulse, as the rvrs app sends it: which version runs, on which system,
//! how often rvrs was started and how many runs each tool made. Never a target, an address, a
//! hostname or a result.
//!
//! Counts are kept in `pulse.json` in the data folder. At most every 12 hours a process of its
//! own (`rvrs __pulse`) sends them to `POST /v1/pulses` and takes what it sent off the counts,
//! so the command that started it never waits. The install is counted by a token made from a
//! random install id and the day, the same within one day and unlinkable across days.
//!
//! `RVRS_PULSE=0` turns it off, and so does `DO_NOT_TRACK=1`. Off, nothing is counted or sent.

use std::collections::BTreeMap;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use rvrs_core::model::Tool;
use rvrs_core::{format, paths};
use serde::{Deserialize, Serialize};

use crate::help::{COMMIT, VERSION};

/// The least time between two pulses, as the app keeps it.
const EVERY: u64 = 12 * 60 * 60;

/// What has happened since the last pulse that reached the API.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Counts {
    #[serde(default)]
    opens: u64,
    /// Runs per tool, by the name the app's pulse uses.
    #[serde(default)]
    runs: BTreeMap<String, u64>,
    /// When the last pulse was started, in seconds since 1970.
    #[serde(default)]
    started_at: u64,
}

impl Counts {
    fn is_empty(&self) -> bool {
        self.opens == 0 && self.runs.values().all(|count| *count == 0)
    }
}

/// Whether the pulse is on. It is unless the person turned it off.
pub fn enabled() -> bool {
    let off = |name: &str, value: &str| std::env::var(name).is_ok_and(|set| set.trim() == value);
    !off("RVRS_PULSE", "0") && !off("DO_NOT_TRACK", "1")
}

fn counts_path() -> std::path::PathBuf {
    paths::data_dir().join("pulse.json")
}

fn read() -> Counts {
    std::fs::read(counts_path())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

fn write(counts: &Counts) {
    let path = counts_path();
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    if let Ok(text) = serde_json::to_string(counts) {
        let _ = std::fs::write(path, text);
    }
}

/// The name the app's pulse gives a tool, so the CLI's runs sit beside the app's.
pub fn tool_key(tool: Tool) -> &'static str {
    match tool {
        Tool::Traceroute => "traceroute",
        Tool::Mtr => "mtr",
        Tool::Ping => "ping",
        Tool::Iperf3 => "iperf",
        Tool::Dns => "dns",
        Tool::Bgp => "bgp",
    }
}

/// Counts that rvrs was started for a command or the Console.
pub fn count_open() {
    if !enabled() {
        return;
    }
    let mut counts = read();
    counts.opens += 1;
    write(&counts);
    send_when_due(&mut counts);
}

/// Counts one run of a tool.
pub fn count_run(tool: Tool) {
    if !enabled() {
        return;
    }
    let mut counts = read();
    *counts.runs.entry(tool_key(tool).to_string()).or_default() += 1;
    write(&counts);
}

/// Starts `rvrs __pulse` when the last pulse is 12 hours old and there is something to say.
fn send_when_due(counts: &mut Counts) {
    let now = format::now();
    if counts.is_empty() || now.saturating_sub(counts.started_at) < EVERY {
        return;
    }
    // Marked first, so a burst of commands starts one process and not twenty.
    counts.started_at = now;
    write(counts);
    if let Ok(executable) = std::env::current_exe() {
        let _ = Command::new(executable)
            .arg("__pulse")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// A random id for this install, made once and kept in the data folder. It never leaves the
/// device: only a hash of it with the day does.
fn install_id() -> Option<String> {
    let path = paths::data_dir().join("install-id");
    if let Ok(id) = std::fs::read_to_string(&path) {
        let id = id.trim().to_string();
        if id.len() >= 32 {
            return Some(id);
        }
    }
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut bytes))
        .ok()?;
    let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    std::fs::write(&path, &id).ok()?;
    Some(id)
}

/// The first 16 hex characters of SHA-256 over `"<install id>|<day>"`, as the app makes it.
fn token(install: &str, day: &str) -> String {
    crate::update::sha256(format!("{install}|{day}").as_bytes())
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The system and its kernel release, as in "macOS 25.1.0" or "Linux 6.8.0".
fn system() -> String {
    let name = match std::env::consts::OS {
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    };
    // SAFETY: utsname is plain data, and uname fills it.
    let mut info: libc::utsname = unsafe { std::mem::zeroed() };
    let release = if unsafe { libc::uname(&mut info) } == 0 {
        // SAFETY: uname wrote a NUL-terminated string into `release`.
        unsafe { std::ffi::CStr::from_ptr(info.release.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    } else {
        String::new()
    };
    format!("{name} {release}")
        .trim()
        .chars()
        .take(64)
        .collect()
}

/// The language from the environment, as in "en" or "de-DE". "und" when it names none.
fn locale() -> String {
    let value = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default();
    let tag = value
        .split(['.', '@'])
        .next()
        .unwrap_or("")
        .replace('_', "-");
    if tag.is_empty() || tag == "C" || tag == "POSIX" {
        "und".to_string()
    } else {
        tag.chars().take(16).collect()
    }
}

fn payload(counts: &Counts, install: &str, day: &str) -> serde_json::Value {
    let clamp = |count: u64| count.min(1_000_000);
    serde_json::json!({
        "schema": 1,
        "app": "rvrs",
        "product": "cli",
        "version": VERSION,
        "build": COMMIT,
        "os": system(),
        "device": std::env::consts::ARCH,
        "locale": locale(),
        "day": day,
        "token": token(install, day),
        "opens": clamp(counts.opens),
        "runs": counts.runs.iter().map(|(tool, count)| (tool.clone(), clamp(*count))).collect::<BTreeMap<_, _>>(),
        "asnCacheHits": 0,
        "asnCacheMisses": 0,
    })
}

/// The hidden `rvrs __pulse`: sends the counts, and on success takes them off what is kept.
pub fn send_now() -> ExitCode {
    if !enabled() {
        return ExitCode::SUCCESS;
    }
    let counts = read();
    if counts.is_empty() {
        return ExitCode::SUCCESS;
    }
    let Some(install) = install_id() else {
        return ExitCode::from(1);
    };
    let day = format::iso8601(format::now())[..10].to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(format!("rvrs/{VERSION}"))
        .build()
        .into();
    let sent = agent
        .post(&format!("{}/v1/pulses", crate::account::api_base()))
        .header("content-type", "application/json")
        .send(payload(&counts, &install, &day).to_string());
    if !sent.is_ok_and(|response| response.status().as_u16() == 202) {
        return ExitCode::from(1);
    }
    // What happened while this was on its way stays for the next pulse.
    let mut now = read();
    now.opens = now.opens.saturating_sub(counts.opens);
    for (tool, count) in &counts.runs {
        if let Some(kept) = now.runs.get_mut(tool) {
            *kept = kept.saturating_sub(*count);
        }
    }
    now.runs.retain(|_, count| *count > 0);
    write(&now);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_the_apps_per_install_per_day_hash() {
        let install = "0123456789abcdef0123456789abcdef";
        let today = token(install, "2026-11-21");
        assert_eq!(today.len(), 16);
        assert!(today
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(today, token(install, "2026-11-21"));
        assert_ne!(today, token(install, "2026-11-22"));
    }

    #[test]
    fn the_payload_carries_counts_and_nothing_about_runs_themselves() {
        let mut counts = Counts {
            opens: 4,
            ..Counts::default()
        };
        counts.runs.insert("mtr".into(), 2);
        let body = payload(&counts, "0123456789abcdef0123456789abcdef", "2026-11-21");
        assert_eq!(body["product"], "cli");
        assert_eq!(body["day"], "2026-11-21");
        assert_eq!(body["opens"], 4);
        assert_eq!(body["runs"]["mtr"], 2);
        let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "app",
                "asnCacheHits",
                "asnCacheMisses",
                "build",
                "day",
                "device",
                "locale",
                "opens",
                "os",
                "product",
                "runs",
                "schema",
                "token",
                "version"
            ]
        );
    }

    #[test]
    fn tools_take_the_apps_names() {
        assert_eq!(tool_key(Tool::Dns), "dns");
        assert_eq!(tool_key(Tool::Iperf3), "iperf");
        assert_eq!(tool_key(Tool::Traceroute), "traceroute");
    }

    #[test]
    fn empty_counts_send_nothing() {
        assert!(Counts::default().is_empty());
        let mut counts = Counts::default();
        counts.runs.insert("ping".into(), 0);
        assert!(counts.is_empty());
        counts.opens = 1;
        assert!(!counts.is_empty());
    }
}
