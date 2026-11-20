//! Cases: named sets of saved runs, from any tool, collected to look into one question. A saved
//! run can sit in several cases or in none. `cases.json` in the data directory, beside the saved
//! runs it points at: `{"cases": [{"id", "name", "run_ids", "created_at"}], "last_used": id}`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::history::History;
use crate::model::{new_id, Run};
use crate::{format, paths};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Case {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub run_ids: Vec<String>,
    #[serde(default)]
    pub created_at: u64,
}

impl Case {
    pub fn holds(&self, run_id: &str) -> bool {
        self.run_ids.iter().any(|id| id == run_id)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    cases: Vec<Case>,
    #[serde(default)]
    last_used: Option<String>,
}

/// Why a case could not be made, found or changed.
#[derive(Debug)]
pub enum CaseError {
    /// The name was empty or only spaces.
    Blank,
    /// Another case has this name already.
    Exists(String),
    /// No case has this name.
    Missing(String),
    Io(std::io::Error),
}

impl std::fmt::Display for CaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaseError::Blank => write!(formatter, "a case needs a name"),
            CaseError::Exists(name) => {
                write!(
                    formatter,
                    "a case named \u{201C}{name}\u{201D} already exists"
                )
            }
            CaseError::Missing(name) => {
                write!(formatter, "no case is named \u{201C}{name}\u{201D}")
            }
            CaseError::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl From<std::io::Error> for CaseError {
    fn from(error: std::io::Error) -> Self {
        CaseError::Io(error)
    }
}

pub struct Cases {
    path: PathBuf,
    /// In the order they were made.
    pub cases: Vec<Case>,
    /// The case a run went into last: where saving puts a run when no case is named.
    pub last_used: Option<String>,
}

/// Names compare trimmed and without regard to case.
fn same_name(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

impl Cases {
    pub fn open(saved: &History) -> Self {
        Self::at(paths::data_dir().join("cases.json"), saved)
    }

    /// Reads the cases and drops every run id that is no longer saved.
    pub fn at(path: PathBuf, saved: &History) -> Self {
        let file: File = std::fs::read(&path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        let mut cases = file.cases;
        for case in &mut cases {
            case.run_ids.retain(|id| saved.contains(id));
        }
        let last_used = file
            .last_used
            .filter(|id| cases.iter().any(|case| &case.id == id));
        Self {
            path,
            cases,
            last_used,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn find(&self, name: &str) -> Option<&Case> {
        self.cases.iter().find(|case| same_name(&case.name, name))
    }

    pub fn get(&self, id: &str) -> Option<&Case> {
        self.cases.iter().find(|case| case.id == id)
    }

    pub fn last_used(&self) -> Option<&Case> {
        self.last_used.as_deref().and_then(|id| self.get(id))
    }

    /// The cases that hold a run.
    pub fn holding<'a>(&'a self, run_id: &'a str) -> impl Iterator<Item = &'a Case> + 'a {
        self.cases.iter().filter(move |case| case.holds(run_id))
    }

    /// Makes a case. Returns its id.
    pub fn create(&mut self, name: &str) -> Result<String, CaseError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CaseError::Blank);
        }
        if let Some(existing) = self.find(name) {
            return Err(CaseError::Exists(existing.name.clone()));
        }
        let id = new_id();
        self.cases.push(Case {
            id: id.clone(),
            name: name.to_string(),
            run_ids: Vec::new(),
            created_at: format::now(),
        });
        self.write()?;
        Ok(id)
    }

    /// The case with this name, made when there is none. Returns its id and whether it is new.
    pub fn ensure(&mut self, name: &str) -> Result<(String, bool), CaseError> {
        match self.find(name) {
            Some(case) => Ok((case.id.clone(), false)),
            None => self.create(name).map(|id| (id, true)),
        }
    }

    pub fn rename(&mut self, name: &str, new_name: &str) -> Result<(), CaseError> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err(CaseError::Blank);
        }
        let index = self
            .cases
            .iter()
            .position(|case| same_name(&case.name, name))
            .ok_or_else(|| CaseError::Missing(name.trim().to_string()))?;
        // A change of capitals alone is a rename too.
        if let Some(other) = self
            .cases
            .iter()
            .enumerate()
            .find(|(at, case)| *at != index && same_name(&case.name, new_name))
        {
            return Err(CaseError::Exists(other.1.name.clone()));
        }
        self.cases[index].name = new_name.to_string();
        self.write()?;
        Ok(())
    }

    /// Deletes the case. Its runs stay saved.
    pub fn delete(&mut self, name: &str) -> Result<Case, CaseError> {
        let index = self
            .cases
            .iter()
            .position(|case| same_name(&case.name, name))
            .ok_or_else(|| CaseError::Missing(name.trim().to_string()))?;
        let case = self.cases.remove(index);
        if self.last_used.as_deref() == Some(case.id.as_str()) {
            self.last_used = None;
        }
        self.write()?;
        Ok(case)
    }

    /// Puts a run in a case and makes it the case used last. Returns false when the run was in
    /// it already.
    pub fn add(&mut self, case_id: &str, run_id: &str) -> std::io::Result<bool> {
        let Some(case) = self.cases.iter_mut().find(|case| case.id == case_id) else {
            return Ok(false);
        };
        let added = !case.holds(run_id);
        if added {
            case.run_ids.insert(0, run_id.to_string());
        }
        self.last_used = Some(case_id.to_string());
        self.write().map(|()| added)
    }

    /// Takes a run out of one case. It stays saved. Returns whether it was in it.
    pub fn remove(&mut self, case_id: &str, run_id: &str) -> std::io::Result<bool> {
        let Some(case) = self.cases.iter_mut().find(|case| case.id == case_id) else {
            return Ok(false);
        };
        let before = case.run_ids.len();
        case.run_ids.retain(|id| id != run_id);
        if case.run_ids.len() == before {
            return Ok(false);
        }
        self.write().map(|()| true)
    }

    /// Takes a run out of every case, when it is no longer saved. Returns how many held it.
    pub fn forget(&mut self, run_id: &str) -> std::io::Result<usize> {
        let mut count = 0;
        for case in &mut self.cases {
            if case.holds(run_id) {
                case.run_ids.retain(|id| id != run_id);
                count += 1;
            }
        }
        if count > 0 {
            self.write()?;
        }
        Ok(count)
    }

    fn write(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = File {
            cases: self.cases.clone(),
            last_used: self.last_used.clone(),
        };
        let data = serde_json::to_vec(&file).map_err(std::io::Error::other)?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, data)?;
        std::fs::rename(&temporary, &self.path)
    }
}

/// What saving a run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    /// The run was not saved before.
    pub newly_saved: bool,
    /// The case it went into, if any.
    pub case: Option<KeptIn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptIn {
    pub name: String,
    /// The case was made for this run.
    pub created: bool,
    /// The run was not in the case before.
    pub added: bool,
}

/// Saves a run, as `rvrs save` and the Console do. Without a case name it also goes into the
/// case used last, when that case still exists; a run that was saved already stays where it is.
/// With a name it goes into that case, which is made when the name is new and becomes the case
/// used last; a saved run just gains the membership.
pub fn save(
    saved: &mut History,
    cases: &mut Cases,
    run: &Run,
    case: Option<&str>,
) -> Result<Kept, CaseError> {
    let (case_id, created) = match case {
        Some(name) => {
            let (id, created) = cases.ensure(name)?;
            (Some(id), created)
        }
        None if saved.contains(&run.id) => {
            return Ok(Kept {
                newly_saved: false,
                case: None,
            })
        }
        None => (cases.last_used.clone(), false),
    };
    let newly_saved = !saved.contains(&run.id);
    if newly_saved {
        saved.save(run)?;
    }
    let case = match case_id {
        Some(id) => {
            let added = cases.add(&id, &run.id)?;
            cases.get(&id).map(|case| KeptIn {
                name: case.name.clone(),
                created,
                added,
            })
        }
        None => None,
    };
    Ok(Kept { newly_saved, case })
}

/// Removes a saved run and its place in every case. Returns whether it was saved.
pub fn unsave(saved: &mut History, cases: &mut Cases, run_id: &str) -> std::io::Result<bool> {
    let removed = saved.remove(run_id)?;
    cases.forget(run_id)?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Tool;
    use crate::FamilyChoice;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("rvrs-test-cases-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn saved(&self) -> History {
            History::saved_in(&self.0)
        }

        fn cases(&self, saved: &History) -> Cases {
            Cases::at(self.0.join("cases.json"), saved)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run(id: &str) -> Run {
        let mut run = Run::new(Tool::Traceroute, "a.example", FamilyChoice::Auto, 3, 30);
        run.id = id.into();
        run
    }

    #[test]
    fn names_are_unique_without_regard_to_case_and_never_blank() {
        let scratch = Scratch::new("names");
        let saved = scratch.saved();
        let mut cases = scratch.cases(&saved);
        let id = cases.create("  vpn-drop ").unwrap();
        assert_eq!(cases.find("VPN-DROP").unwrap().id, id);
        assert_eq!(cases.find("vpn-drop").unwrap().name, "vpn-drop");
        assert!(
            matches!(cases.create("Vpn-Drop"), Err(CaseError::Exists(name)) if name == "vpn-drop")
        );
        assert!(matches!(cases.create("   "), Err(CaseError::Blank)));
        assert_eq!(cases.ensure("vpn-drop").unwrap(), (id.clone(), false));
        let (other, created) = cases.ensure("home").unwrap();
        assert!(created && other != id);
        // Renaming to another case's name is refused; a change of capitals is not.
        assert!(matches!(
            cases.rename("home", "VPN-drop"),
            Err(CaseError::Exists(_))
        ));
        cases.rename("home", "Home").unwrap();
        assert!(matches!(
            cases.rename("nothing", "x"),
            Err(CaseError::Missing(_))
        ));
        let reloaded = scratch.cases(&saved);
        assert_eq!(
            reloaded
                .cases
                .iter()
                .map(|case| case.name.as_str())
                .collect::<Vec<_>>(),
            vec!["vpn-drop", "Home"]
        );
    }

    #[test]
    fn saving_follows_the_case_used_last() {
        let scratch = Scratch::new("save");
        let mut saved = scratch.saved();
        let mut cases = scratch.cases(&saved);
        // No case yet: the run is saved on its own.
        let kept = save(&mut saved, &mut cases, &run("aaa222"), None).unwrap();
        assert_eq!(
            kept,
            Kept {
                newly_saved: true,
                case: None
            }
        );
        // A named case is made, holds the run, and becomes the case used last.
        let kept = save(&mut saved, &mut cases, &run("bbb333"), Some("vpn-drop")).unwrap();
        assert!(kept.newly_saved);
        assert_eq!(
            kept.case,
            Some(KeptIn {
                name: "vpn-drop".into(),
                created: true,
                added: true
            })
        );
        assert_eq!(cases.last_used().unwrap().name, "vpn-drop");
        // Without a name the next run goes into the case used last.
        let kept = save(&mut saved, &mut cases, &run("ccc444"), None).unwrap();
        assert_eq!(kept.case.unwrap().name, "vpn-drop");
        // A saved run without a name stays where it is.
        let kept = save(&mut saved, &mut cases, &run("aaa222"), None).unwrap();
        assert_eq!(
            kept,
            Kept {
                newly_saved: false,
                case: None
            }
        );
        assert!(!cases.find("vpn-drop").unwrap().holds("aaa222"));
        // With a name it just gains the membership.
        let kept = save(&mut saved, &mut cases, &run("aaa222"), Some("VPN-drop")).unwrap();
        assert!(!kept.newly_saved);
        assert_eq!(
            kept.case,
            Some(KeptIn {
                name: "vpn-drop".into(),
                created: false,
                added: true
            })
        );
        let kept = save(&mut saved, &mut cases, &run("aaa222"), Some("vpn-drop")).unwrap();
        assert!(!kept.case.unwrap().added);
        assert_eq!(saved.runs.len(), 3);
        assert!(matches!(
            save(&mut saved, &mut cases, &run("ddd555"), Some(" ")),
            Err(CaseError::Blank)
        ));
        assert!(!saved.contains("ddd555"));
    }

    #[test]
    fn a_run_sits_in_several_cases_and_leaves_them_one_by_one() {
        let scratch = Scratch::new("membership");
        let mut saved = scratch.saved();
        let mut cases = scratch.cases(&saved);
        save(&mut saved, &mut cases, &run("aaa222"), Some("one")).unwrap();
        save(&mut saved, &mut cases, &run("aaa222"), Some("two")).unwrap();
        assert_eq!(cases.holding("aaa222").count(), 2);
        assert_eq!(cases.last_used().unwrap().name, "two");
        let one = cases.find("one").unwrap().id.clone();
        assert!(cases.remove(&one, "aaa222").unwrap());
        assert!(!cases.remove(&one, "aaa222").unwrap());
        assert!(saved.contains("aaa222"));
        assert_eq!(scratch.cases(&saved).holding("aaa222").count(), 1);
    }

    #[test]
    fn deleting_a_case_keeps_its_runs_saved() {
        let scratch = Scratch::new("delete");
        let mut saved = scratch.saved();
        let mut cases = scratch.cases(&saved);
        save(&mut saved, &mut cases, &run("aaa222"), Some("vpn-drop")).unwrap();
        let deleted = cases.delete("VPN-DROP").unwrap();
        assert_eq!(deleted.run_ids, vec!["aaa222"]);
        assert!(cases.cases.is_empty() && cases.last_used.is_none());
        assert!(saved.contains("aaa222"));
        assert!(matches!(
            cases.delete("vpn-drop"),
            Err(CaseError::Missing(_))
        ));
        // With the case used last gone, saving puts the run in no case.
        let kept = save(&mut saved, &mut cases, &run("bbb333"), None).unwrap();
        assert!(kept.case.is_none());
    }

    #[test]
    fn a_run_no_longer_saved_leaves_every_case() {
        let scratch = Scratch::new("unsave");
        let mut saved = scratch.saved();
        let mut cases = scratch.cases(&saved);
        save(&mut saved, &mut cases, &run("aaa222"), Some("one")).unwrap();
        save(&mut saved, &mut cases, &run("aaa222"), Some("two")).unwrap();
        assert!(unsave(&mut saved, &mut cases, "aaa222").unwrap());
        assert!(!saved.contains("aaa222"));
        assert_eq!(cases.holding("aaa222").count(), 0);
        assert!(!unsave(&mut saved, &mut cases, "aaa222").unwrap());
        assert_eq!(cases.cases.len(), 2);
    }

    #[test]
    fn loading_drops_runs_that_are_no_longer_saved() {
        let scratch = Scratch::new("load");
        let mut saved = scratch.saved();
        let mut cases = scratch.cases(&saved);
        save(&mut saved, &mut cases, &run("aaa222"), Some("one")).unwrap();
        save(&mut saved, &mut cases, &run("bbb333"), Some("one")).unwrap();
        // Removed behind the cases' back, as an older version would.
        saved.remove("aaa222").unwrap();
        let reloaded = scratch.cases(&saved);
        assert_eq!(reloaded.find("one").unwrap().run_ids, vec!["bbb333"]);
        assert_eq!(reloaded.last_used().unwrap().name, "one");
        // A broken file reads as no cases.
        std::fs::write(scratch.0.join("cases.json"), b"not json").unwrap();
        assert!(scratch.cases(&saved).cases.is_empty());
    }
}
