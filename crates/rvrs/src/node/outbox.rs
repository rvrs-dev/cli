//! Where a node's finished runs wait until the API has answered for them.
//!
//! Each result is a file in the node's state folder, the body of `POST /v1/runs` as it will be
//! sent: the report, and the claim it was made under. So a restart loses nothing, and what was
//! made while the uplink was gone goes out once it is back, oldest first. Where the folder can't
//! be written the outbox is kept in memory, and the heartbeat says which.
//!
//! A file is whole or it isn't there: a result is written beside its name first and takes the
//! name once every byte is on the disk. A file that can't be read as a result anyway is removed
//! at the start and counted as dropped.
//!
//! It has a cap in runs, in bytes and in age. When it is over one, the oldest result goes and is
//! counted as dropped, which the next heartbeat says.

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// The most results that wait.
pub const RUNS_AT_MOST: usize = 500;
/// The most bytes that wait.
pub const BYTES_AT_MOST: u64 = 64 * 1024 * 1024;
/// How long a result waits before the node gives it up.
pub const WAITS: Duration = Duration::from_secs(7 * 24 * 3600);

/// One result that waits.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The file's name, and what the entry is called while it waits.
    pub name: String,
    /// The report, and the claim when the run was asked for.
    pub body: Value,
    bytes: u64,
    made: SystemTime,
    /// When the run started, by this process's own clock that only counts forward. None for a
    /// result from before this start, since that clock started over.
    started: Option<Instant>,
    /// False for a result whose file couldn't be written. It waits for as long as this runs.
    on_disk: bool,
}

impl Entry {
    /// The claim the result answers, when the run was asked for.
    pub fn claim(&self) -> Option<(String, u64)> {
        let claim = &self.body["claim"];
        Some((
            claim["run"].as_str()?.to_string(),
            claim["attempt"].as_u64()?,
        ))
    }

    /// The body as it is sent now: with how long ago the run started, when this process knows.
    pub fn to_send(&self, now: Instant) -> Value {
        let mut body = self.body.clone();
        if let Some(started) = self.started {
            let ago = now.saturating_duration_since(started).as_millis();
            body["startedAgoMilliseconds"] = Value::from(u64::try_from(ago).unwrap_or(u64::MAX));
        }
        body
    }
}

pub struct Outbox {
    /// None keeps the results in memory.
    folder: Option<PathBuf>,
    entries: VecDeque<Entry>,
    dropped: u32,
    /// Files found at the start that were no result, and were removed.
    unreadable: u32,
    made: u64,
}

/// Writes a result so that its file is whole or not there: beside its name first, and under the
/// name once every byte is on the disk. A node that loses power in between leaves nothing that
/// reads as half a result.
fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    let beside = path.with_extension("json.tmp");
    let written = (|| {
        let mut file = std::fs::File::create(&beside)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&beside, path)
    })();
    match &written {
        Ok(()) => {
            // The folder holds the new name. Where the folder can't be synced, the name is
            // there all the same unless the power goes in the next moments.
            let folder = path
                .parent()
                .and_then(|folder| std::fs::File::open(folder).ok());
            if let Some(folder) = folder {
                let _ = folder.sync_all();
            }
        }
        Err(_) => {
            let _ = std::fs::remove_file(&beside);
        }
    }
    written
}

fn millis(at: SystemTime) -> u128 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
}

impl Outbox {
    /// Opens the outbox in `folder` with whatever waits there from before. A folder that can't
    /// be made or read gives an outbox in memory.
    pub fn open(folder: &Path) -> Outbox {
        if std::fs::create_dir_all(folder).is_err() {
            return Outbox::in_memory();
        }
        let Ok(listing) = std::fs::read_dir(folder) else {
            return Outbox::in_memory();
        };
        let mut entries = Vec::new();
        let mut unreadable = 0u32;
        for file in listing.flatten() {
            let path = file.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            // A write that never finished. Its result was never counted as waiting.
            if name.ends_with(".tmp") {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if !name.ends_with(".json") {
                continue;
            }
            // A file that can't be opened now may be readable at the next start. It stays.
            let Ok(data) = std::fs::read(&path) else {
                continue;
            };
            let body = serde_json::from_slice::<Value>(&data)
                .ok()
                .filter(|body| body["report"].is_object());
            let Some(body) = body else {
                // No result can be read from it, now or later.
                let _ = std::fs::remove_file(&path);
                unreadable += 1;
                continue;
            };
            let made = file
                .metadata()
                .and_then(|file| file.modified())
                .unwrap_or_else(|_| SystemTime::now());
            entries.push(Entry {
                name: name.to_string(),
                body,
                bytes: data.len() as u64,
                made,
                started: None,
                on_disk: true,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let mut outbox = Outbox {
            folder: Some(folder.to_path_buf()),
            entries: entries.into(),
            dropped: unreadable,
            unreadable,
            made: 0,
        };
        outbox.trim(SystemTime::now());
        outbox
    }

    pub fn in_memory() -> Outbox {
        Outbox {
            folder: None,
            entries: VecDeque::new(),
            dropped: 0,
            unreadable: 0,
            made: 0,
        }
    }

    /// `disk` or `memory`, as the heartbeat says it. It is `memory` for as long as one result
    /// waits that has no file, since a restart would lose that one.
    pub fn keeps(&self) -> &'static str {
        if self.folder.is_some() && self.entries.iter().all(|entry| entry.on_disk) {
            "disk"
        } else {
            "memory"
        }
    }

    /// Files found at the start that were no result and were removed. They count as dropped.
    pub fn unreadable(&self) -> u32 {
        self.unreadable
    }

    pub fn waiting(&self) -> u32 {
        u32::try_from(self.entries.len()).unwrap_or(u32::MAX)
    }

    /// Results given up since this start.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// The claims of the results that wait. A claim lists them as held, since the API hasn't
    /// answered for them yet.
    pub fn claims(&self) -> Vec<(String, u64)> {
        self.entries.iter().filter_map(Entry::claim).collect()
    }

    /// The oldest result, which is the next to send.
    pub fn oldest(&self) -> Option<&Entry> {
        self.entries.front()
    }

    fn path(&self, name: &str) -> Option<PathBuf> {
        self.folder.as_ref().map(|folder| folder.join(name))
    }

    /// Keeps a result until the API has answered for it. `started` is when its run started.
    /// False when the outbox has a folder and the result's file couldn't be written, so the
    /// result waits in memory.
    pub fn put(&mut self, body: Value, started: Option<Instant>) -> bool {
        let now = SystemTime::now();
        // Names sort by when they were made, and two made in one millisecond stay in order. A
        // name is never taken twice, also where the clock started over with the machine.
        let name = loop {
            self.made += 1;
            let name = format!("{:013}-{:06}.json", millis(now), self.made % 1_000_000);
            let taken = self.entries.iter().any(|entry| entry.name == name)
                || self.path(&name).is_some_and(|path| path.exists());
            if !taken {
                break name;
            }
        };
        let text = body.to_string();
        let on_disk = match self.path(&name) {
            Some(path) => write_whole(&path, &text).is_ok(),
            None => false,
        };
        let kept = on_disk || self.folder.is_none();
        self.entries.push_back(Entry {
            name,
            bytes: text.len() as u64,
            body,
            made: now,
            started,
            on_disk,
        });
        self.trim(now);
        kept
    }

    fn take(&mut self, name: &str) -> bool {
        let Some(at) = self.entries.iter().position(|entry| entry.name == name) else {
            return false;
        };
        self.entries.remove(at);
        if let Some(path) = self.path(name) {
            let _ = std::fs::remove_file(path);
        }
        true
    }

    /// The API answered for this result: it has it, or the claim is lost and it wants none.
    pub fn answered(&mut self, name: &str) {
        self.take(name);
    }

    /// The API will never take this result, so sending it again changes nothing. It counts as
    /// dropped.
    pub fn give_up(&mut self, name: &str) {
        if self.take(name) {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    /// A run whose report can't be sent at all. It counts as dropped.
    pub fn lost(&mut self) {
        self.dropped = self.dropped.saturating_add(1);
    }

    /// The API said to drop a run. Its result is sent to nobody and isn't counted: nothing was
    /// lost that anyone waits for.
    pub fn forget(&mut self, run: &str, attempt: u64) {
        let names: Vec<String> = self
            .entries
            .iter()
            .filter(|entry| entry.claim().is_some_and(|(r, a)| r == run && a == attempt))
            .map(|entry| entry.name.clone())
            .collect();
        for name in names {
            self.take(&name);
        }
    }

    /// Drops the oldest results while the outbox is over a cap.
    fn trim(&mut self, now: SystemTime) {
        loop {
            let bytes: u64 = self.entries.iter().map(|entry| entry.bytes).sum();
            let too_old = self.entries.front().is_some_and(|entry| {
                now.duration_since(entry.made)
                    .is_ok_and(|waited| waited > WAITS)
            });
            // The newest result always stays, however large it is.
            let over = self.entries.len() > 1
                && (self.entries.len() > RUNS_AT_MOST || bytes > BYTES_AT_MOST);
            if !too_old && !over {
                return;
            }
            let Some(name) = self.entries.front().map(|entry| entry.name.clone()) else {
                return;
            };
            self.give_up(&name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn folder(name: &str) -> PathBuf {
        let folder =
            std::env::temp_dir().join(format!("rvrs-outbox-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        folder
    }

    fn result(id: &str, run: Option<&str>) -> Value {
        match run {
            Some(run) => json!({ "report": { "id": id }, "claim": { "run": run, "attempt": 1 } }),
            None => json!({ "report": { "id": id } }),
        }
    }

    #[test]
    fn results_wait_on_disk_across_a_restart_and_go_out_oldest_first() {
        let folder = folder("restart");
        let mut outbox = Outbox::open(&folder);
        assert_eq!(outbox.keeps(), "disk");
        outbox.put(result("a", Some("run-a")), Some(Instant::now()));
        outbox.put(result("b", None), Some(Instant::now()));
        assert_eq!(outbox.waiting(), 2);

        // The node restarts. Both results are still there, in the order they were made.
        let mut outbox = Outbox::open(&folder);
        assert_eq!(outbox.waiting(), 2);
        assert_eq!(outbox.claims(), [("run-a".to_string(), 1)]);
        let first = outbox.oldest().unwrap().clone();
        assert_eq!(first.body["report"]["id"], "a");
        // The clock that counted how long ago the run started is gone with the old process.
        assert!(first.to_send(Instant::now())["startedAgoMilliseconds"].is_null());
        outbox.answered(&first.name);
        assert_eq!(outbox.oldest().unwrap().body["report"]["id"], "b");
        assert_eq!(outbox.dropped(), 0);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_file_is_a_whole_result_or_it_is_removed_and_counted() {
        let folder = folder("whole");
        let mut outbox = Outbox::open(&folder);
        assert!(outbox.put(result("a", Some("run-a")), None));
        // Nothing is left beside the result's file.
        let names: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .flatten()
            .map(|file| file.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1);
        assert!(names[0].ends_with(".json"));

        // The power went during a write, and two other files are no result.
        std::fs::write(folder.join("0000000000001-000001.json.tmp"), "{\"report\":").unwrap();
        std::fs::write(
            folder.join("0000000000002-000001.json"),
            "{\"report\":{\"id\"",
        )
        .unwrap();
        std::fs::write(folder.join("0000000000003-000001.json"), "[]").unwrap();
        let outbox = Outbox::open(&folder);
        assert_eq!(outbox.waiting(), 1);
        assert_eq!(outbox.oldest().unwrap().body["report"]["id"], "a");
        assert_eq!(outbox.unreadable(), 2);
        assert_eq!(outbox.dropped(), 2);
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_result_without_a_file_waits_in_memory_and_the_outbox_says_so() {
        let folder = folder("nodisk");
        let mut outbox = Outbox::open(&folder);
        assert!(outbox.put(result("a", None), None));
        assert_eq!(outbox.keeps(), "disk");
        // The folder is gone, as when a volume is pulled.
        std::fs::remove_dir_all(&folder).unwrap();
        assert!(!outbox.put(result("b", None), None));
        assert_eq!(outbox.waiting(), 2);
        assert_eq!(outbox.keeps(), "memory");
        // Once the API has answered for the result without a file, what waits has one again.
        outbox.answered(&outbox.entries[1].name.clone());
        assert_eq!(outbox.keeps(), "disk");
    }

    #[test]
    fn a_name_is_never_taken_twice() {
        let folder = folder("names");
        let mut first = Outbox::open(&folder);
        first.put(result("a", None), None);
        // A second start counts from one again, in what may be the same millisecond.
        let mut second = Outbox::open(&folder);
        for id in ["b", "c", "d"] {
            second.put(result(id, None), None);
        }
        assert_eq!(Outbox::open(&folder).waiting(), 4);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_result_says_how_long_ago_its_run_started() {
        let mut outbox = Outbox::in_memory();
        assert_eq!(outbox.keeps(), "memory");
        let started = Instant::now();
        outbox.put(result("a", None), Some(started));
        let sent = outbox
            .oldest()
            .unwrap()
            .to_send(started + Duration::from_millis(14_412));
        assert_eq!(sent["startedAgoMilliseconds"], 14_412);
        assert_eq!(sent["report"]["id"], "a");
    }

    #[test]
    fn the_oldest_goes_when_the_outbox_is_full_and_is_counted() {
        let mut outbox = Outbox::in_memory();
        for n in 0..RUNS_AT_MOST + 3 {
            outbox.put(result(&format!("r{n}"), None), None);
        }
        assert_eq!(outbox.waiting() as usize, RUNS_AT_MOST);
        assert_eq!(outbox.dropped(), 3);
        assert_eq!(outbox.oldest().unwrap().body["report"]["id"], "r3");
    }

    #[test]
    fn a_result_the_api_will_never_take_counts_as_dropped_and_a_lost_claim_does_not() {
        let mut outbox = Outbox::in_memory();
        outbox.put(result("a", Some("run-a")), None);
        outbox.put(result("b", Some("run-b")), None);
        outbox.put(result("c", Some("run-c")), None);
        let name = outbox.oldest().unwrap().name.clone();
        outbox.give_up(&name);
        assert_eq!(outbox.dropped(), 1);
        let name = outbox.oldest().unwrap().name.clone();
        outbox.answered(&name);
        outbox.forget("run-c", 1);
        assert_eq!(outbox.dropped(), 1);
        assert_eq!(outbox.waiting(), 0);
    }
}
