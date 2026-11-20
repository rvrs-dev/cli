//! Derives the version from git, as the app does (`../ios-app/scripts/version.sh`), so every commit
//! is a new version and no build needs a hand edit:
//!
//! - `RVRS_VERSION`: `<VERSION>.<patch>`. `VERSION` at the repo root holds major.minor, bumped by
//!   hand for a bigger change. The patch counts the commits since `VERSION` last changed, and
//!   uncommitted changes count as one more.
//! - `RVRS_COMMIT`: the short commit, with `-dirty` for uncommitted changes.
//!
//! Without git (a source archive), the version is the crate's own and the commit is "unknown".

use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let version_file = root.join("VERSION");
    println!("cargo:rerun-if-changed={}", version_file.display());

    // Run again whenever HEAD moves or the index changes, wherever the git directory is (a
    // submodule keeps it in the parent's .git/modules).
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        for file in ["HEAD", "index", "logs/HEAD"] {
            println!("cargo:rerun-if-changed={dir}/{file}");
        }
    }

    let base = std::fs::read_to_string(&version_file)
        .map(|text| text.trim().to_string())
        .ok();
    let commit = git(&["rev-parse", "--short=7", "HEAD"]);

    let (version, commit) = match (base, commit) {
        (Some(base), Some(commit)) => {
            let mut patch: u64 = git(&["log", "-1", "--format=%H", "--", ":/VERSION"])
                .filter(|since| !since.is_empty())
                .and_then(|since| git(&["rev-list", "--count", &format!("{since}..HEAD")]))
                .and_then(|count| count.parse().ok())
                .unwrap_or(0);
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|status| !status.is_empty());
            if dirty {
                patch += 1;
            }
            let commit = if dirty {
                format!("{commit}-dirty")
            } else {
                commit
            };
            (format!("{base}.{patch}"), commit)
        }
        _ => (env!("CARGO_PKG_VERSION").to_string(), "unknown".to_string()),
    };

    println!("cargo:rustc-env=RVRS_VERSION={version}");
    println!("cargo:rustc-env=RVRS_COMMIT={commit}");
}
