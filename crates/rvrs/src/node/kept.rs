//! The node's copy of its schedules, kept where the outbox is kept.
//!
//! A node with a disk writes the copy the API handed it into its state folder, as
//! `schedules.json`. After a restart it starts its schedules from that file, so it goes on
//! measuring when the API is out of reach at the start. Where the folder can't be written the
//! copy lives in memory alone, and a restart begins without schedules until the API answers.
//!
//! The file holds what to run and how often. When a schedule's next run starts is different in
//! every answer and is never written, so a copy that didn't change costs no write.

use std::path::{Path, PathBuf};

use rvrs_core::node::schedules::Copy;
use serde_json::Value;

use super::outbox::write_whole;

/// The file's name in the node's state folder.
const FILE_NAME: &str = "schedules.json";

pub struct KeptCopy {
    /// None keeps the copy in memory alone.
    path: Option<PathBuf>,
    /// What the file holds, as far as this process knows.
    written: Option<String>,
}

impl KeptCopy {
    /// The copy in `state`, the folder that also holds the outbox.
    pub fn at(state: &Path) -> KeptCopy {
        KeptCopy {
            path: Some(state.join(FILE_NAME)),
            written: None,
        }
    }

    /// The copy the node kept from before this start. None when there is no file, or when it
    /// holds no copy that can be read.
    pub fn load(&mut self) -> Option<Copy> {
        let text = std::fs::read_to_string(self.path.as_ref()?).ok()?;
        let copy = Copy::read(&serde_json::from_str::<Value>(&text).ok()?)?;
        self.written = Some(text);
        Some(copy)
    }

    /// Keeps a copy. The file is written only when it would say something else than it does,
    /// and it is whole or not there, as a result in the outbox is. False when the copy couldn't
    /// be written, so it lives in memory alone.
    pub fn save(&mut self, copy: &Copy) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let Ok(text) = serde_json::to_string(copy) else {
            return false;
        };
        if self.written.as_deref() == Some(text.as_str()) {
            return true;
        }
        if let Some(folder) = path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        let kept = write_whole(path, &text).is_ok();
        self.written = kept.then_some(text);
        kept
    }

    /// The node holds no schedules any more, so it keeps none.
    pub fn remove(&mut self) {
        if let Some(path) = &self.path {
            let _ = std::fs::remove_file(path);
        }
        self.written = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    #[test]
    fn a_copy_is_there_after_a_restart_without_when_its_runs_start() {
        let state = state("restart");
        let mut kept = KeptCopy::at(&state);
        assert_eq!(kept.load(), None);
        assert!(kept.save(&copy(41_250)));
        // Nothing is left beside the file.
        assert_eq!(std::fs::read_dir(&state).unwrap().count(), 1);

        let mut again = KeptCopy::at(&state);
        let read = again.load().unwrap();
        assert_eq!(read.version, "5d0c2a9e41b7f630");
        assert_eq!(read.keep_seconds, 604_800);
        assert!(read.schedules[0].says_the_same(&copy(0).schedules[0]));
        assert_eq!(read.schedules[0].starts_in_milliseconds, 0);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_copy_that_says_the_same_is_not_written_again() {
        let state = state("same");
        let mut kept = KeptCopy::at(&state);
        assert!(kept.save(&copy(41_250)));
        let path = state.join(FILE_NAME);
        // Another answer with the same schedules and another start leaves the file alone, also
        // when the file is gone.
        std::fs::remove_file(&path).unwrap();
        assert!(kept.save(&copy(9_800)));
        assert!(!path.exists());
        // A copy that says something else is written.
        let mut other = copy(0);
        other.version = "b81e07c93a2f4d16".into();
        assert!(kept.save(&other));
        assert!(path.exists());
        kept.remove();
        assert!(!path.exists());
        assert_eq!(KeptCopy::at(&state).load(), None);
        let _ = std::fs::remove_dir_all(&state);
    }

    #[test]
    fn a_folder_that_can_not_be_written_keeps_the_copy_in_memory_alone() {
        let state = state("nodisk");
        // A file stands where the folder would be.
        std::fs::write(&state, "x").unwrap();
        let mut kept = KeptCopy::at(&state);
        assert!(!kept.save(&copy(0)));
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
