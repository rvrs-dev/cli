//! Every run kept, newest first: `history.json` in the data directory, 500 at most. One file,
//! read whole and written atomically, the way the app keeps its history.

use std::path::{Path, PathBuf};

use crate::model::Run;
use crate::paths;

pub const LIMIT: usize = 500;

/// The file saved runs were kept in before they had their name. Read once, then renamed.
const EARLIER_SAVED: &str = "bookmarks.json";

pub struct History {
    path: PathBuf,
    limit: usize,
    pub runs: Vec<Run>,
    /// Where the runs the account handed this device are kept, when this is the history and
    /// not the saved runs. They are listed with the runs made here and written apart from them.
    account: Option<PathBuf>,
}

fn read_runs(path: &Path) -> Vec<Run> {
    std::fs::read(path)
        .ok()
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_default()
}

fn write_runs(path: &Path, runs: &[&Run]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec(runs).map_err(std::io::Error::other)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, data)?;
    std::fs::rename(&temporary, path)
}

/// The file with the runs other installs of the account made, newest first.
pub fn account_runs_path() -> PathBuf {
    paths::data_dir().join("account-runs.json")
}

/// The runs other installs of the account made, as the last sync left them.
pub fn account_runs() -> Vec<Run> {
    read_runs(&account_runs_path())
}

/// Replaces them, newest first and [`LIMIT`] at most. Only the sync writes this file.
pub fn write_account_runs(mut runs: Vec<Run>) -> std::io::Result<()> {
    runs.sort_by_key(|run| std::cmp::Reverse(run.started_at));
    runs.truncate(LIMIT);
    write_runs(&account_runs_path(), &runs.iter().collect::<Vec<_>>())
}

impl History {
    /// The runs made here together with the ones the account handed this device.
    pub fn open() -> Self {
        Self::with_account(paths::data_dir().join("history.json"), account_runs_path())
    }

    pub fn with_account(path: PathBuf, account: PathBuf) -> Self {
        let mut history = Self::at(path);
        let mine: Vec<String> = history.runs.iter().map(|run| run.id.clone()).collect();
        let others = read_runs(&account)
            .into_iter()
            .filter(|run| run.from.is_some() && !mine.contains(&run.id));
        history.runs.extend(others);
        // Stable, so runs of one second stay in the order they were kept.
        history
            .runs
            .sort_by_key(|run| std::cmp::Reverse(run.started_at));
        history.account = Some(account);
        history
    }

    pub fn at(path: PathBuf) -> Self {
        let runs = read_runs(&path);
        Self {
            path,
            limit: LIMIT,
            runs,
            account: None,
        }
    }

    /// Saved runs: runs kept on purpose, in `saved.json`. They are whole runs, apart from
    /// history, so they stay when history is cleared or a run ages out of it.
    pub fn saved() -> Self {
        Self::saved_in(&paths::data_dir())
    }

    /// Saved runs in a data directory. Versions before saved runs kept the same runs under an
    /// older file name; when `saved.json` is missing and that file is there, it is renamed, so
    /// nobody loses a run.
    pub fn saved_in(dir: &Path) -> Self {
        let path = dir.join("saved.json");
        let earlier = dir.join(EARLIER_SAVED);
        if !path.exists() && earlier.exists() {
            let _ = std::fs::rename(&earlier, &path);
        }
        let mut store = Self::at(path);
        store.limit = usize::MAX;
        store
    }

    pub fn contains(&self, id: &str) -> bool {
        self.runs.iter().any(|run| run.id == id)
    }

    /// Removes a run by id. Returns whether there was one.
    pub fn remove(&mut self, id: &str) -> std::io::Result<bool> {
        let before = self.runs.len();
        self.runs.retain(|run| run.id != id);
        if self.runs.len() == before {
            return Ok(false);
        }
        self.write_account()?;
        self.write().map(|()| true)
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Saves a run at the top. A run saved before (the Console saves while a run is open) is replaced.
    pub fn save(&mut self, run: &Run) -> std::io::Result<()> {
        self.runs.retain(|saved| saved.id != run.id);
        self.runs.insert(0, run.clone());
        if self.account.is_none() {
            self.runs.truncate(self.limit);
        }
        self.write()
    }

    pub fn clear(&mut self) -> std::io::Result<()> {
        self.runs.clear();
        self.write_account()?;
        self.write()
    }

    pub fn last(&self) -> Option<&Run> {
        self.runs.first()
    }

    /// A run by its id, or by the start of it when that names exactly one.
    pub fn find(&self, id: &str) -> Option<&Run> {
        if let Some(exact) = self.runs.iter().find(|run| run.id == id) {
            return Some(exact);
        }
        let mut matches = self.runs.iter().filter(|run| run.id.starts_with(id));
        match (matches.next(), matches.next()) {
            (Some(only), None) if !id.is_empty() => Some(only),
            _ => None,
        }
    }

    /// The newest DNS run of the same name other than `run`, to compare a lookup with.
    pub fn previous_dns(&self, run: &Run) -> Option<&Run> {
        self.runs
            .iter()
            .filter(|other| other.id != run.id && other.started_at <= run.started_at)
            .find(|other| {
                other.tool == crate::model::Tool::Dns
                    && other.target.eq_ignore_ascii_case(&run.target)
                    && other.dns.is_some()
            })
    }

    /// The newest traceroute or MTR to `target` that has hops, to lay against a BGP result.
    pub fn last_trace(&self, target: &str) -> Option<&Run> {
        use crate::model::Tool;
        self.runs.iter().find(|run| {
            matches!(run.tool, Tool::Traceroute | Tool::Mtr)
                && run.target.eq_ignore_ascii_case(target)
                && !run.hops.is_empty()
        })
    }

    /// Targets as typed, most recent first, each once. What the Console and shell completion suggest.
    pub fn targets(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for run in &self.runs {
            if !seen.contains(&run.target.as_str()) {
                seen.push(&run.target);
            }
        }
        seen
    }

    /// Writes the runs this file holds. In the history those are the runs made here, the
    /// newest [`LIMIT`]: the account's runs have a file of their own that the sync writes.
    fn write(&self) -> std::io::Result<()> {
        let kept: Vec<&Run> = match self.account {
            Some(_) => self
                .runs
                .iter()
                .filter(|run| run.from.is_none())
                .take(self.limit)
                .collect(),
            None => self.runs.iter().collect(),
        };
        write_runs(&self.path, &kept)
    }

    /// Writes the account's runs after one was removed here or history was cleared.
    fn write_account(&self) -> std::io::Result<()> {
        let Some(path) = &self.account else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let kept: Vec<&Run> = self.runs.iter().filter(|run| run.from.is_some()).collect();
        write_runs(path, &kept)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Tool;
    use crate::FamilyChoice;

    fn run(target: &str) -> Run {
        Run::new(Tool::Traceroute, target, FamilyChoice::Auto, 3, 30)
    }

    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rvrs-test-{name}-{}.json", std::process::id()))
    }

    #[test]
    fn saves_newest_first_and_survives_a_reload() {
        let path = scratch("history");
        let mut history = History::at(path.clone());
        history.save(&run("a.example")).unwrap();
        history.save(&run("b.example")).unwrap();
        history.save(&run("a.example")).unwrap();
        let reloaded = History::at(path.clone());
        assert_eq!(reloaded.runs.len(), 3);
        assert_eq!(reloaded.last().unwrap().target, "a.example");
        assert_eq!(reloaded.targets(), vec!["a.example", "b.example"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn saving_a_run_again_replaces_it() {
        let path = scratch("replace");
        let mut history = History::at(path.clone());
        let mut first = run("a.example");
        history.save(&first).unwrap();
        first.max_ttl = 12;
        history.save(&first).unwrap();
        assert_eq!(history.runs.len(), 1);
        assert_eq!(history.runs[0].max_ttl, 12);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn finds_by_id_or_unique_prefix() {
        let mut history = History::at(scratch("find"));
        let mut one = run("a.example");
        one.id = "abc234".into();
        let mut two = run("b.example");
        two.id = "abd567".into();
        history.runs = vec![one, two];
        assert_eq!(history.find("abc234").unwrap().target, "a.example");
        assert_eq!(history.find("abd").unwrap().target, "b.example");
        assert!(history.find("ab").is_none());
        assert!(history.find("").is_none());
    }

    #[test]
    fn saved_runs_are_added_found_and_removed() {
        let path = scratch("saved");
        let mut saved = History::at(path.clone());
        let mut kept = run("a.example");
        kept.id = "abc234".into();
        saved.save(&kept).unwrap();
        assert!(saved.contains("abc234"));
        assert!(History::at(path.clone()).contains("abc234"));
        assert!(saved.remove("abc234").unwrap());
        assert!(!saved.remove("abc234").unwrap());
        assert!(History::at(path.clone()).runs.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn saved_runs_from_the_earlier_file_are_kept() {
        let dir = std::env::temp_dir().join(format!("rvrs-test-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut earlier = History::at(dir.join(EARLIER_SAVED));
        let mut kept = run("a.example");
        kept.id = "abc234".into();
        earlier.save(&kept).unwrap();
        let saved = History::saved_in(&dir);
        assert!(saved.contains("abc234"));
        assert_eq!(saved.path(), dir.join("saved.json"));
        assert!(!dir.join(EARLIER_SAVED).exists());
        // Once saved.json exists, a stray earlier file is left alone.
        std::fs::write(dir.join(EARLIER_SAVED), b"[]").unwrap();
        assert!(History::saved_in(&dir).contains("abc234"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_broken_file_reads_as_empty_history() {
        let path = scratch("broken");
        std::fs::write(&path, b"not json").unwrap();
        assert!(History::at(path.clone()).runs.is_empty());
        let _ = std::fs::remove_file(path);
    }
}
