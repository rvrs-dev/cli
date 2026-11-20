//! Where rvrs keeps its files. XDG locations on every system, so macOS and Linux agree:
//! history in the data directory, lookups in the cache directory, settings in the config
//! directory. `RVRS_HOME` moves all three under one folder, which tests and nodes use.

use std::env;
use std::path::PathBuf;

fn base(variable: &str, fallback: &str, leaf: &str) -> PathBuf {
    if let Some(home) = env::var_os("RVRS_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(leaf);
    }
    if let Some(dir) = env::var_os(variable).filter(|value| !value.is_empty()) {
        return PathBuf::from(dir).join("rvrs");
    }
    let home = env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from);
    home.join(fallback).join("rvrs")
}

pub fn data_dir() -> PathBuf {
    base("XDG_DATA_HOME", ".local/share", "data")
}

pub fn cache_dir() -> PathBuf {
    base("XDG_CACHE_HOME", ".cache", "cache")
}

pub fn config_dir() -> PathBuf {
    base("XDG_CONFIG_HOME", ".config", "config")
}

/// A path with the home directory written as "~", for messages.
pub fn display(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    match env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}
