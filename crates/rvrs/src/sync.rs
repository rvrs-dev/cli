//! Keeping a signed-in install and its account the same.
//!
//! Sending: a run made while signed in is written to the `outbox` folder as the body of
//! `POST /v1/runs`, the report with the run as this CLI keeps it, and a process of its own
//! (`rvrs __sync`) sends it, so the command that made the run never waits. A run that didn't go,
//! offline for one, stays in the outbox for the next time, for a week and 500 runs at most.
//!
//! Receiving, for an install a person signed in on: the runs their other installs made are read
//! from `GET /v1/runs` into `account-runs.json`, which history lists beside the runs made here.
//! Favorites, saved runs and cases are one document, the library. The sync reads it, merges it
//! with what changed here since the last time and writes the result to both sides.
//!
//! An install that signs in with `RVRS_TOKEN` only sends. `RVRS_SYNC=0` turns all of it off.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use rvrs_core::cases::Cases;
use rvrs_core::favorites::Favorites;
use rvrs_core::history::{self, History};
use rvrs_core::model::Run;
use rvrs_core::report::Report;
use rvrs_core::{format, paths};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::account;

/// The least time between two syncs that nothing but a start of rvrs asked for.
const EVERY: u64 = 10 * 60;
/// The format this CLI's own record of a run has in the account.
const FORMAT: &str = "cli-run";
/// The API takes 2 MiB. A run larger than this goes as its report alone.
const LARGEST_BODY: usize = 1_900_000;
/// How long a run waits in the outbox for the API before it is dropped: a week offline.
const WAITS: u64 = 7 * 24 * 60 * 60;
/// How many runs wait at most. The oldest go first.
const WAITING_AT_MOST: usize = 500;

/// What a person keeps on purpose, as the account holds it: runs by their id in the account.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub favorites: Vec<String>,
    #[serde(default)]
    pub saved: Vec<String>,
    #[serde(default)]
    pub cases: Vec<LibraryCase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryCase {
    pub name: String,
    #[serde(default)]
    pub runs: Vec<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

/// What the last sync left: where reading stopped and what the account held then.
#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// The API and the person this state belongs to. Another account starts from nothing.
    #[serde(default)]
    account: String,
    /// The last run read from the account.
    #[serde(default)]
    cursor: Option<String>,
    /// A run's id here to its id in the account, for runs sent and runs received.
    #[serde(default)]
    ids: BTreeMap<String, String>,
    /// The library as the account had it after the last sync: what a change is measured against.
    #[serde(default)]
    base: Library,
    /// Runs the library names that this device has no record of to show. They count as kept
    /// here, so a device that can't show a run never removes it from the account.
    #[serde(default)]
    unshown: BTreeSet<String>,
}

fn folder() -> PathBuf {
    paths::data_dir()
}

fn state_path() -> PathBuf {
    folder().join("sync.json")
}

fn outbox() -> PathBuf {
    folder().join("outbox")
}

/// Touched when a sync is started for a start of rvrs, so a burst of commands starts one.
fn stamp_path() -> PathBuf {
    folder().join("sync.stamp")
}

/// Written when a sync changed what this device shows. The Console reads its stores again.
pub fn changed_path() -> PathBuf {
    folder().join("sync.changed")
}

fn lock_path() -> PathBuf {
    folder().join("sync.lock")
}

/// Whether syncing is on. It is unless the person turned it off.
pub fn enabled() -> bool {
    !std::env::var("RVRS_SYNC").is_ok_and(|value| value.trim() == "0")
}

fn read_state(account: &str) -> State {
    let state: State = std::fs::read(state_path())
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default();
    if state.account == account {
        return state;
    }
    // Another account, or the first sync: nothing received so far belongs to this one.
    let _ = std::fs::remove_file(history::account_runs_path());
    State {
        account: account.to_string(),
        ..State::default()
    }
}

fn write_state(state: &State) {
    let _ = std::fs::create_dir_all(folder());
    if let Ok(data) = serde_json::to_vec(state) {
        let temporary = state_path().with_extension("json.tmp");
        if std::fs::write(&temporary, data).is_ok() {
            let _ = std::fs::rename(&temporary, state_path());
        }
    }
}

/// Forgets what the account handed this device. Signing out calls it. Runs made here, saved
/// runs, cases and favorites stay.
pub fn forget() {
    let _ = std::fs::remove_file(state_path());
    let _ = std::fs::remove_file(history::account_runs_path());
    let _ = std::fs::remove_file(stamp_path());
    let _ = std::fs::remove_dir_all(outbox());
    let _ = std::fs::write(changed_path(), b"");
}

/// The body of `POST /v1/runs` for a run: its report, and the run as this CLI keeps it.
fn body_of(run: &Run) -> String {
    let report = serde_json::to_value(Report::of(run)).unwrap_or(Value::Null);
    let whole = json!({ "report": report, "detail": { "format": FORMAT, "run": run } }).to_string();
    if whole.len() <= LARGEST_BODY {
        return whole;
    }
    json!({ "report": report }).to_string()
}

fn put_in_outbox(run: &Run) -> bool {
    let folder = outbox();
    if std::fs::create_dir_all(&folder).is_err() {
        return false;
    }
    std::fs::write(folder.join(format!("{}.json", run.id)), body_of(run)).is_ok()
}

/// Starts `rvrs __sync` and returns at once.
pub fn start() {
    if !enabled() || account::token().is_none() {
        return;
    }
    if let Ok(executable) = std::env::current_exe() {
        let _ = Command::new(executable)
            .arg("__sync")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// Hands a run that just ended to the account, when this install is signed in.
pub fn queue(run: &Run) {
    if !enabled() || run.from.is_some() || account::token().is_none() {
        return;
    }
    if put_in_outbox(run) {
        start();
    }
}

/// Starts a sync when the last one a start of rvrs asked for is ten minutes old.
pub fn when_due() {
    if !enabled() || account::token().is_none() {
        return;
    }
    let age = std::fs::metadata(stamp_path())
        .and_then(|file| file.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .map(|age| age.as_secs());
    if age.is_some_and(|age| age < EVERY) {
        return;
    }
    let _ = std::fs::create_dir_all(folder());
    let _ = std::fs::write(stamp_path(), b"");
    start();
}

/// One sync at a time. A lock older than two minutes is one a stopped process left.
struct Lock;

impl Lock {
    fn take() -> Option<Self> {
        let path = lock_path();
        let _ = std::fs::create_dir_all(folder());
        let stale = std::fs::metadata(&path)
            .and_then(|file| file.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age.as_secs() > 120);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()
            .map(|_| Lock)
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(lock_path());
    }
}

/// What a sync did, for `rvrs sync` to say.
#[derive(Debug, Default, PartialEq)]
struct Done {
    sent: usize,
    waiting: usize,
    received: usize,
}

fn call(method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
    account::call(method, path, body, 20)
}

fn refused(status: u16, body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("The rvrs API answered {status}."))
}

/// Sends what is in the outbox. A run the API will never take is dropped, and one that
/// didn't get through stays.
fn send(state: &mut State, done: &mut Done) -> Result<(), String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(outbox())
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect()
        })
        .unwrap_or_default();
    // Oldest first, so runs arrive in the order they were made.
    let waited = |path: &PathBuf| {
        std::fs::metadata(path)
            .and_then(|file| file.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .map_or(0, |age| age.as_secs())
    };
    files.sort_by_key(|path| std::cmp::Reverse(waited(path)));
    let over = files.len().saturating_sub(WAITING_AT_MOST);
    type Numbered = Vec<(usize, PathBuf)>;
    let (dropped, files): (Numbered, Numbered) = files
        .into_iter()
        .enumerate()
        .partition(|(index, path)| *index < over || waited(path) > WAITS);
    for (_, path) in &dropped {
        let _ = std::fs::remove_file(path);
    }
    let files: Vec<PathBuf> = files.into_iter().map(|(_, path)| path).collect();
    for (index, path) in files.iter().enumerate() {
        let body: Option<Value> = std::fs::read(path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok());
        let (Some(body), Some(id)) = (body, path.file_stem().and_then(|stem| stem.to_str())) else {
            let _ = std::fs::remove_file(path);
            continue;
        };
        let (status, answer) = match call("POST", "/v1/runs", Some(&body)) {
            Ok(answer) => answer,
            Err(_) => {
                done.waiting = files.len() - index;
                break;
            }
        };
        match status {
            200 | 201 => {
                if let Some(account_id) = answer["id"].as_str() {
                    state.ids.insert(id.to_string(), account_id.to_string());
                }
                let _ = std::fs::remove_file(path);
                done.sent += 1;
            }
            401 => return Err(refused(status, &answer)),
            // Too large or not a report the API reads: sending it again changes nothing.
            413 | 422 => {
                let _ = std::fs::remove_file(path);
            }
            _ => {
                done.waiting = files.len() - index;
                break;
            }
        }
    }
    Ok(())
}

/// The run a `GET /v1/runs` entry carries, when it is in this CLI's format.
fn run_of(entry: &Value) -> Option<(String, Run)> {
    let id = entry["id"].as_str()?;
    if entry["detail"]["format"].as_str()? != FORMAT {
        return None;
    }
    let mut run: Run = serde_json::from_value(entry["detail"]["run"].clone()).ok()?;
    run.from = Some(
        entry["install"]["name"]
            .as_str()
            .unwrap_or("another install")
            .to_string(),
    );
    Some((id.to_string(), run))
}

/// Reads the runs the person's other installs made since the last time.
fn receive(state: &mut State, done: &mut Done) -> Result<(), String> {
    let own = History::at(folder().join("history.json"));
    let mut kept = history::account_runs();
    let mut changed = false;
    for _ in 0..10 {
        let mut path = "/v1/runs?others=true&limit=200".to_string();
        if let Some(cursor) = &state.cursor {
            path.push_str(&format!("&after={cursor}"));
        }
        let (status, body) = call("GET", &path, None)?;
        if status != 200 {
            return Err(refused(status, &body));
        }
        for entry in body["runs"].as_array().into_iter().flatten() {
            let Some((account_id, run)) = run_of(entry) else {
                continue;
            };
            // A run made here before this install signed in again is the same run.
            let here = own.runs.iter().find(|mine| mine.id == run.id);
            if here.is_some_and(|mine| mine.started_at != run.started_at) {
                continue;
            }
            state.ids.insert(run.id.clone(), account_id);
            if here.is_none() && !kept.iter().any(|other| other.id == run.id) {
                kept.push(run);
                done.received += 1;
                changed = true;
            }
        }
        if let Some(cursor) = body["cursor"].as_str() {
            state.cursor = Some(cursor.to_string());
        }
        if body["more"].as_bool() != Some(true) {
            break;
        }
    }
    if changed {
        history::write_account_runs(kept).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn lower(text: &str) -> String {
    text.trim().to_lowercase()
}

fn exact(text: &str) -> String {
    text.to_string()
}

/// Three-way merge of a list kept as a set: what was added here goes first, what was removed
/// here goes, and everything else is as the account has it.
fn merge_set(
    base: &[String],
    local: &[String],
    remote: &[String],
    key: fn(&str) -> String,
) -> Vec<String> {
    let has = |list: &[String], wanted: &str| list.iter().any(|item| key(item) == wanted);
    let mut merged: Vec<String> = Vec::new();
    for item in local {
        let wanted = key(item);
        if !has(base, &wanted) && !has(remote, &wanted) && !has(&merged, &wanted) {
            merged.push(item.clone());
        }
    }
    for item in remote {
        let wanted = key(item);
        if has(base, &wanted) && !has(local, &wanted) {
            continue;
        }
        merged.push(item.clone());
    }
    merged
}

/// Three-way merge of two libraries against the one both started from. Cases are told apart
/// by name, so a case made under the same name on two devices is one case with both sets of runs.
pub fn merge(base: &Library, local: &Library, remote: &Library) -> Library {
    let find = |library: &Library, name: &str| -> Option<LibraryCase> {
        library
            .cases
            .iter()
            .find(|case| lower(&case.name) == lower(name))
            .cloned()
    };
    let mut cases = Vec::new();
    for theirs in &remote.cases {
        let before = find(base, &theirs.name);
        let Some(mine) = find(local, &theirs.name) else {
            if before.is_none() {
                cases.push(theirs.clone());
            }
            continue;
        };
        let started: Vec<String> = before.map(|case| case.runs).unwrap_or_default();
        cases.push(LibraryCase {
            name: theirs.name.clone(),
            runs: merge_set(&started, &mine.runs, &theirs.runs, exact),
            created_at: theirs.created_at.clone(),
        });
    }
    for mine in &local.cases {
        if find(remote, &mine.name).is_none() && find(base, &mine.name).is_none() {
            cases.push(mine.clone());
        }
    }
    let mut merged = Library {
        version: remote.version,
        favorites: merge_set(&base.favorites, &local.favorites, &remote.favorites, lower),
        saved: merge_set(&base.saved, &local.saved, &remote.saved, exact),
        cases,
    };
    // What the API takes at most.
    merged.favorites.truncate(500);
    merged.saved.truncate(2000);
    merged.cases.truncate(200);
    merged
}

/// Seconds since 1970 of an ISO 8601 moment in UTC, as the API writes them.
fn seconds_of(moment: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| moment.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    // Days from the civil date, after Howard Hinnant.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year.rem_euclid(400);
    let of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + of_era * 365 + of_era / 4 - of_era / 100 + of_year - 719_468;
    u64::try_from(days * 86_400 + hour * 3600 + minute * 60 + second).ok()
}

/// What this device keeps, in the account's terms. A saved run the account doesn't have yet is
/// left out until it was sent. A run this device can't show counts as kept.
fn library_here(state: &State) -> Library {
    let saved = History::saved();
    let cases = Cases::open(&saved);
    let account_id = |id: &String| state.ids.get(id).cloned();
    let unshown = |ids: &[String]| -> Vec<String> {
        ids.iter()
            .filter(|id| state.unshown.contains(*id))
            .cloned()
            .collect()
    };
    let mut kept: Vec<String> = saved
        .runs
        .iter()
        .filter_map(|run| account_id(&run.id))
        .collect();
    kept.extend(unshown(&state.base.saved));
    Library {
        version: state.base.version,
        favorites: Favorites::open().hosts,
        saved: kept,
        cases: cases
            .cases
            .iter()
            .map(|case| {
                let mut runs: Vec<String> = case.run_ids.iter().filter_map(account_id).collect();
                let before = state
                    .base
                    .cases
                    .iter()
                    .find(|other| lower(&other.name) == lower(&case.name));
                if let Some(before) = before {
                    runs.extend(unshown(&before.runs));
                }
                LibraryCase {
                    name: case.name.clone(),
                    runs,
                    created_at: format::iso8601(case.created_at),
                }
            })
            .collect(),
    }
}

/// Makes this device keep what the merged library says.
fn keep_here(library: &Library, state: &mut State) -> Result<bool, String> {
    let failed = |error: std::io::Error| error.to_string();
    let mut changed = false;
    let mut favorites = Favorites::open();
    if favorites.hosts != library.favorites {
        favorites.set(library.favorites.clone()).map_err(failed)?;
        changed = true;
    }

    let mut saved = History::saved();
    let here = |state: &State, account_id: &str| -> Option<String> {
        state
            .ids
            .iter()
            .find(|(_, other)| other.as_str() == account_id)
            .map(|(id, _)| id.clone())
    };
    state.unshown.clear();
    let known = History::open();
    for account_id in library.saved.iter().rev() {
        if here(state, account_id).is_some_and(|id| saved.contains(&id)) {
            continue;
        }
        let run = here(state, account_id)
            .and_then(|id| known.runs.iter().find(|run| run.id == id).cloned())
            .or_else(|| {
                // Older than what this device read: ask for this one run.
                let (status, body) = call("GET", &format!("/v1/runs/{account_id}"), None).ok()?;
                let (_, run) = (status == 200).then(|| run_of(&body)).flatten()?;
                state.ids.insert(run.id.clone(), account_id.clone());
                Some(run)
            });
        match run {
            Some(run) => {
                saved.save(&run).map_err(failed)?;
                changed = true;
            }
            None => {
                state.unshown.insert(account_id.clone());
            }
        }
    }
    let gone: Vec<String> = saved
        .runs
        .iter()
        .filter(|run| {
            state
                .ids
                .get(&run.id)
                .is_some_and(|account_id| !library.saved.contains(account_id))
        })
        .map(|run| run.id.clone())
        .collect();
    for id in gone {
        saved.remove(&id).map_err(failed)?;
        changed = true;
    }

    let mut cases = Cases::open(&saved);
    let kept = library
        .cases
        .iter()
        .map(|case| {
            let mut runs: Vec<String> = Vec::new();
            for account_id in &case.runs {
                match here(state, account_id).filter(|id| saved.contains(id)) {
                    Some(id) => runs.push(id),
                    None => {
                        state.unshown.insert(account_id.clone());
                    }
                }
            }
            // Runs saved here that the account doesn't have yet stay in their case.
            if let Some(mine) = cases.find(&case.name) {
                for id in &mine.run_ids {
                    if !state.ids.contains_key(id) && !runs.contains(id) {
                        runs.push(id.clone());
                    }
                }
            }
            let made = seconds_of(&case.created_at).unwrap_or_else(format::now);
            (case.name.clone(), runs, made)
        })
        .collect::<Vec<_>>();
    let before = cases.cases.clone();
    cases.replace(kept).map_err(failed)?;
    Ok(changed || before != cases.cases)
}

/// Brings the library on this device and in the account to the same state.
fn sync_library(state: &mut State) -> Result<bool, String> {
    for _ in 0..3 {
        let (status, body) = call("GET", "/v1/library", None)?;
        if status != 200 {
            return Err(refused(status, &body));
        }
        let remote: Library = serde_json::from_value(body).map_err(|error| error.to_string())?;
        let mut merged = merge(&state.base, &library_here(state), &remote);
        let same = merged.favorites == remote.favorites
            && merged.saved == remote.saved
            && merged.cases == remote.cases;
        if !same {
            let input = json!({
                "base": remote.version,
                "favorites": merged.favorites,
                "saved": merged.saved,
                "cases": merged.cases,
            });
            let (status, body) = call("PUT", "/v1/library", Some(&input))?;
            match status {
                200 => merged.version = body["version"].as_u64().unwrap_or(remote.version + 1),
                // Another device was faster. Read what it wrote and merge again.
                409 => continue,
                _ => return Err(refused(status, &body)),
            }
        }
        let changed = keep_here(&merged, state)?;
        state.base = merged;
        return Ok(changed);
    }
    Ok(false)
}

/// Saved runs made here that the account doesn't have go into the outbox, so what a person
/// kept before they signed in follows them.
fn queue_saved(state: &State) {
    for run in &History::saved().runs {
        if run.from.is_none() && !state.ids.contains_key(&run.id) {
            let waiting = outbox().join(format!("{}.json", run.id));
            if !waiting.exists() {
                put_in_outbox(run);
            }
        }
    }
}

fn sync() -> Result<Done, String> {
    let Some(token) = account::token() else {
        return Err("Not signed in. rvrs login signs in.".into());
    };
    // One started by the run before this may be at work. It takes a few seconds.
    let mut lock = Lock::take();
    for _ in 0..100 {
        if lock.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        lock = Lock::take();
    }
    let Some(_lock) = lock else {
        return Err("Another sync is still running. Try again in a moment.".into());
    };
    // A token from the environment is a device nobody sits at: it sends and reads nothing.
    let sends_only = account::env_token();
    let who = match account::Credential::stored() {
        Some(credential) if !sends_only => format!("{}|{}", credential.api, credential.email),
        _ => format!(
            "{}|token|{}",
            account::api_base(),
            &token[..token.len().min(12)]
        ),
    };
    let mut state = read_state(&who);
    let mut done = Done::default();
    if !sends_only {
        queue_saved(&state);
    }
    let mut result = send(&mut state, &mut done);
    let mut changed = false;
    if result.is_ok() && !sends_only {
        result = receive(&mut state, &mut done).and_then(|()| {
            changed = sync_library(&mut state)?;
            Ok(())
        });
    }
    write_state(&state);
    if changed || done.received > 0 {
        let _ = std::fs::write(changed_path(), b"");
    }
    result.map(|()| done)
}

fn count(number: usize) -> String {
    match number {
        1 => "1 run".to_string(),
        many => format!("{many} runs"),
    }
}

/// The sentence `rvrs sync` ends with.
fn sentence(done: &Done) -> String {
    let mut parts = Vec::new();
    if done.sent > 0 {
        parts.push(format!("Sent {}.", count(done.sent)));
    }
    if done.received > 0 {
        parts.push(format!("Received {}.", count(done.received)));
    }
    if done.waiting > 0 {
        parts.push(format!(
            "{} still to send, since the rvrs API didn't take {}.",
            count(done.waiting),
            if done.waiting == 1 { "it" } else { "them" }
        ));
    }
    if parts.is_empty() {
        parts.push("This device and your account are the same.".to_string());
    }
    parts.join(" ")
}

/// `rvrs sync`: send what is waiting and read what is new, now, and say what happened.
pub fn run(arguments: &[String]) -> ExitCode {
    if let Some(other) = arguments.first() {
        eprintln!("rvrs: rvrs sync takes nothing, and \u{201C}{other}\u{201D} is something.");
        return ExitCode::from(2);
    }
    if !enabled() {
        eprintln!("rvrs: RVRS_SYNC=0 is set, so runs stay on this device.");
        return ExitCode::from(1);
    }
    match sync() {
        Ok(done) => {
            println!("{}", sentence(&done));
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("rvrs: {message}");
            ExitCode::from(1)
        }
    }
}

/// The hidden `rvrs __sync`: the same, saying nothing.
pub fn run_quietly() -> ExitCode {
    if !enabled() {
        return ExitCode::SUCCESS;
    }
    match sync() {
        Ok(_) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn case(name: &str, runs: &[&str]) -> LibraryCase {
        LibraryCase {
            name: name.into(),
            runs: list(runs),
            created_at: "2026-11-21T10:00:00Z".into(),
        }
    }

    #[test]
    fn a_set_keeps_what_either_side_added_and_drops_what_either_removed() {
        let base = list(&["a", "b", "c"]);
        let local = list(&["d", "a", "c"]);
        let remote = list(&["a", "b", "e"]);
        assert_eq!(
            merge_set(&base, &local, &remote, exact),
            list(&["d", "a", "e"])
        );
    }

    #[test]
    fn favorites_compare_without_regard_to_case() {
        let merged = merge_set(&[], &list(&["Example.com"]), &list(&["example.com"]), lower);
        assert_eq!(merged, list(&["example.com"]));
    }

    #[test]
    fn a_fresh_device_takes_the_account_as_it_is_and_removes_nothing() {
        let remote = Library {
            version: 4,
            favorites: list(&["example.com"]),
            saved: list(&["r1"]),
            cases: vec![case("Slow checkout", &["r1"])],
        };
        let merged = merge(&Library::default(), &Library::default(), &remote);
        assert_eq!(merged, remote);
    }

    #[test]
    fn cases_merge_by_name_and_a_deleted_case_stays_deleted() {
        let base = Library {
            cases: vec![case("Old", &["r1"]), case("Shared", &["r1"])],
            ..Library::default()
        };
        let local = Library {
            cases: vec![case("shared", &["r1", "r2"]), case("Mine", &["r3"])],
            ..Library::default()
        };
        let remote = Library {
            cases: vec![
                case("Old", &["r1"]),
                case("Shared", &["r1", "r4"]),
                case("Theirs", &[]),
            ],
            ..Library::default()
        };
        let merged = merge(&base, &local, &remote);
        let names: Vec<&str> = merged.cases.iter().map(|case| case.name.as_str()).collect();
        assert_eq!(names, ["Shared", "Theirs", "Mine"]);
        assert_eq!(merged.cases[0].runs, list(&["r2", "r1", "r4"]));
    }

    #[test]
    fn a_case_the_account_dropped_is_not_brought_back() {
        let base = Library {
            cases: vec![case("Gone", &["r1"])],
            ..Library::default()
        };
        let merged = merge(&base, &base, &Library::default());
        assert!(merged.cases.is_empty());
    }

    #[test]
    fn a_moment_reads_back_as_the_seconds_it_was_written_from() {
        for seconds in [0, 951_782_400, 1_794_735_000, 4_102_444_799] {
            assert_eq!(seconds_of(&format::iso8601(seconds)), Some(seconds));
        }
        assert_eq!(seconds_of("2026-11-21T10:00:00.000Z"), Some(1_795_255_200));
        assert_eq!(seconds_of("soon"), None);
    }

    #[test]
    fn the_body_is_the_report_with_the_run_as_this_cli_keeps_it() {
        let run = Run::new(
            rvrs_core::model::Tool::Ping,
            "example.com",
            rvrs_core::FamilyChoice::Auto,
            3,
            30,
        );
        let body: Value = serde_json::from_str(&body_of(&run)).unwrap();
        assert_eq!(body["report"]["id"], run.id.as_str());
        assert_eq!(body["report"]["schema"], 1);
        assert_eq!(body["detail"]["format"], "cli-run");
        let back: Run = serde_json::from_value(body["detail"]["run"].clone()).unwrap();
        assert_eq!(back, run);
    }

    #[test]
    fn the_sentence_says_what_went_each_way() {
        assert_eq!(
            sentence(&Done::default()),
            "This device and your account are the same."
        );
        let done = Done {
            sent: 1,
            waiting: 2,
            received: 3,
        };
        assert_eq!(
            sentence(&done),
            "Sent 1 run. Received 3 runs. 2 runs still to send, since the rvrs API didn't take them."
        );
    }
}
