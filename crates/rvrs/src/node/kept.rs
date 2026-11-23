//! The node's copy of its schedules, kept where the outbox is kept.
//!
//! A node with a disk writes the copy the API handed it into its state folder, as
//! `schedules.json`. After a restart it starts its schedules from that file, so it goes on
//! measuring when the API is out of reach at the start. Where the folder can't be written the
//! copy lives in memory alone, and a restart begins without schedules until the API answers.
//!
//! The file holds what to run and how often, and the copy's age beside it: when its answer
//! arrived and how long the node has run with it since (`rvrs_core::node::schedules::Age`). The
//! age is what keeps a restart from making an old copy fresh. It is written with the copy, then
//! every ten minutes while the node runs, and when the node stops. A file without it is no copy.
//!
//! When a schedule's next run starts is different in every answer and is never written.

use std::path::{Path, PathBuf};

use rvrs_core::node::schedules::{Age, Copy};
use serde_json::Value;

use super::outbox::write_whole;

/// The file's name in the node's state folder.
const FILE_NAME: &str = "schedules.json";

pub struct KeptCopy {
    /// None keeps the copy in memory alone.
    path: Option<PathBuf>,
    /// The copy the file is to hold, and its age as it was last written.
    copy: Option<(Copy, Age)>,
    /// What the file holds, as far as this process knows.
    written: Option<String>,
}

/// The file's text: the copy, with its age beside it.
fn text_of(copy: &Copy, age: Age) -> Option<String> {
    let mut value = serde_json::to_value(copy).ok()?;
    value["readAt"] = age.read_at.into();
    value["usedSeconds"] = age.used_seconds.into();
    Some(value.to_string())
}

impl KeptCopy {
    /// The copy in `state`, the folder that also holds the outbox.
    pub fn at(state: &Path) -> KeptCopy {
        KeptCopy {
            path: Some(state.join(FILE_NAME)),
            copy: None,
            written: None,
        }
    }

    /// The copy the node kept from before this start, with its age. None when there is no file,
    /// when it holds no copy that can be read, and when it says nothing of the copy's age, as a
    /// file from an older build does. A copy of unknown age could be one that ran out long ago,
    /// so the node starts without schedules and reads its copy when it connects.
    pub fn load(&mut self) -> Option<(Copy, Age)> {
        let text = std::fs::read_to_string(self.path.as_ref()?).ok()?;
        let value = serde_json::from_str::<Value>(&text).ok()?;
        let copy = Copy::read(&value)?;
        let age = Age::read(&value)?;
        self.copy = Some((copy.clone(), age));
        self.written = Some(text);
        Some((copy, age))
    }

    /// Writes the file when it would say something else than it does, or when it is gone. It is
    /// whole or not there, as a result in the outbox is. False when it couldn't be written, so
    /// the copy lives in memory alone.
    fn write(&mut self, text: String) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        // A file someone removed, or a volume that came back empty, holds nothing, whatever
        // this process wrote before.
        if self.written.as_deref() == Some(text.as_str()) && path.exists() {
            return true;
        }
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let kept = write_whole(path, &text).is_ok();
        self.written = kept.then_some(text);
        kept
    }

    /// Keeps a copy the API answered with, and its age. False when the copy couldn't be
    /// written, so it lives in memory alone.
    ///
    /// A copy without a schedule in it has nothing that could run too long, so its age alone is
    /// no reason to write: a node without schedules writes its file once and then leaves the
    /// disk alone, however often it connects.
    pub fn save(&mut self, copy: &Copy, age: Age) -> bool {
        let age = match &self.copy {
            Some((held, written)) if copy.schedules.is_empty() && held == copy => *written,
            _ => age,
        };
        self.copy = Some((copy.clone(), age));
        match text_of(copy, age) {
            Some(text) => self.write(text),
            None => false,
        }
    }

    /// Writes the age of the copy the file holds, as it is now. The node calls this every ten
    /// minutes while it runs with schedules, and when it stops. True when there is nothing to
    /// write or the file has it.
    pub fn save_age(&mut self, age: Age) -> bool {
        let Some((copy, _)) = self.copy.clone() else {
            return true;
        };
        if copy.schedules.is_empty() {
            return true;
        }
        self.save(&copy, age)
    }

    /// The node holds no schedules any more, so it keeps none.
    pub fn remove(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
        self.copy = None;
        self.written = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WALL: u64 = 1_795_000_000;

    fn state(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("rvrs-kept-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        folder
    }

    fn copy(starts_in: u64) -> Copy {
        Copy::read(&json!({
            "version": "5d0c2a9e41b7f630",
            "keepSeconds": 604_800,
            "schedules": [{
                "schedule": "7a1c9e52-3b4d-4f6a-8c0e-2d5f7b9a1c3e", "tool": "ping",
                "target": "example.com", "options": { "packets": 10 }, "everySeconds": 60,
                "startsInMilliseconds": starts_in,
            }],
        }))
        .unwrap()
    }

    fn used(seconds: u64) -> Age {
        Age {
            read_at: WALL,
            used_seconds: seconds,
        }
    }

    fn on_disk(state: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(state.join(FILE_NAME)).unwrap()).unwrap()
    }

    #[test]
    fn a_copy_is_there_after_a_restart_with_its_age_and_without_when_its_runs_start() {
        let state = state("restart");
        let mut kept = KeptCopy::at(&state);
        assert_eq!(kept.load(), None);
        assert!(kept.save(&copy(41_250), Age::new(WALL)));
        // Nothing is left beside the file.
        assert_eq!(std::fs::read_dir(&state).unwrap().count(), 1);
        // Ten minutes on, the node writes how long it has run with the copy.
        assert!(kept.save_age(used(600)));

        let mut again = KeptCopy::at(&state);
        let (read, age) = again.load().unwrap();
        assert_eq!(read.version, "5d0c2a9e41b7f630");
        assert_eq!(read.keep_seconds, 604_800);
        assert!(read.schedules[0].says_the_same(&copy(0).schedules[0]));
        assert_eq!(read.schedules[0].starts_in_milliseconds, 0);
        assert_eq!(age, used(600));
        let written = on_disk(&state);
        assert_eq!(written["readAt"], WALL);
        assert_eq!(written["usedSeconds"], 600);
        assert!(written["schedules"][0]
            .get("startsInMilliseconds")
            .is_none());
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_file_without_the_copys_age_is_no_copy() {
        // What a build from before the age wrote: the copy alone.
        let state = state("noage");
        std::fs::create_dir_all(&state).unwrap();
        let old = serde_json::to_string(&copy(0)).unwrap();
        std::fs::write(state.join(FILE_NAME), &old).unwrap();
        assert_eq!(KeptCopy::at(&state).load(), None);
        // The same with half an age, or one that is no number.
        for broken in [
            json!({ "readAt": WALL }),
            json!({ "usedSeconds": 60 }),
            json!({ "readAt": "yesterday", "usedSeconds": 60 }),
        ] {
            let mut value: Value = serde_json::from_str(&old).unwrap();
            for (key, field) in broken.as_object().unwrap() {
                value[key] = field.clone();
            }
            std::fs::write(state.join(FILE_NAME), value.to_string()).unwrap();
            assert_eq!(KeptCopy::at(&state).load(), None, "{broken}");
        }
        // The next answer writes a file with an age, and that one is a copy.
        let mut kept = KeptCopy::at(&state);
        assert!(kept.save(&copy(0), Age::new(WALL)));
        assert_eq!(
            KeptCopy::at(&state).load().map(|(_, age)| age),
            Some(Age::new(WALL))
        );
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_file_that_is_gone_is_written_again() {
        let state = state("gone");
        let mut kept = KeptCopy::at(&state);
        assert!(kept.save(&copy(41_250), Age::new(WALL)));
        let path = state.join(FILE_NAME);
        // Someone removed the file, or the volume came back empty. The same copy with the same
        // age is on the disk again after the next save, since the file would hold nothing.
        std::fs::remove_file(&path).unwrap();
        assert!(kept.save(&copy(9_800), Age::new(WALL)));
        assert!(path.exists());
        assert_eq!(on_disk(&state)["version"], "5d0c2a9e41b7f630");
        // The same for the age the node writes while it runs, also when the folder is gone.
        std::fs::remove_dir_all(&state).unwrap();
        assert!(kept.save_age(used(600)));
        assert_eq!(on_disk(&state)["usedSeconds"], 600);
        // A file that is there and says the same is left alone.
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(kept.save_age(used(600)));
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
        kept.remove();
        assert!(!path.exists());
        assert_eq!(KeptCopy::at(&state).load(), None);
        // With no copy held there is no age to write.
        assert!(kept.save_age(used(900)));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_copy_without_schedules_is_written_once() {
        let state = state("empty");
        let empty = Copy {
            version: "none".into(),
            keep_seconds: 604_800,
            schedules: Vec::new(),
        };
        let mut kept = KeptCopy::at(&state);
        assert!(kept.save(&empty, Age::new(WALL)));
        // The node connects again an hour later and gets the same copy. Nothing in it can run
        // too long, so the file stays as it is, and so it does every ten minutes.
        assert!(kept.save(&empty, Age::new(WALL + 3600)));
        assert!(kept.save_age(used(600)));
        assert_eq!(on_disk(&state)["readAt"], WALL);
        assert_eq!(on_disk(&state)["usedSeconds"], 0);
        // A copy with a schedule in it starts its age at its own answer.
        assert!(kept.save(&copy(0), Age::new(WALL + 7200)));
        assert_eq!(on_disk(&state)["readAt"], WALL + 7200);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_folder_that_can_not_be_written_keeps_the_copy_in_memory_alone() {
        let state = state("nodisk");
        // A file stands where the folder would be.
        std::fs::write(&state, "x").unwrap();
        let mut kept = KeptCopy::at(&state);
        assert!(!kept.save(&copy(0), Age::new(WALL)));
        assert!(!kept.save_age(used(600)));
        assert_eq!(kept.load(), None);
        let _ = std::fs::remove_file(&state);
        // A file that holds no copy is no copy.
        let state = self::state("broken");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join(FILE_NAME), "{\"version\":").unwrap();
        assert_eq!(KeptCopy::at(&state).load(), None);
        let _ = std::fs::remove_dir_all(&state);
    }
}
