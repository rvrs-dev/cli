//! Favorites: hosts kept on purpose, as typed, in `favorites.json`. A favorite is a host and
//! never a run. Newest first.

use std::path::PathBuf;

use crate::paths;

pub struct Favorites {
    path: PathBuf,
    pub hosts: Vec<String>,
}

impl Favorites {
    pub fn open() -> Self {
        Self::at(paths::data_dir().join("favorites.json"))
    }

    pub fn at(path: PathBuf) -> Self {
        let hosts = std::fs::read(&path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        Self { path, hosts }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Hosts compare without regard to case, as names do.
    pub fn contains(&self, host: &str) -> bool {
        self.hosts
            .iter()
            .any(|kept| kept.eq_ignore_ascii_case(host))
    }

    /// Adds the host at the top. Returns false when it was there already.
    pub fn add(&mut self, host: &str) -> std::io::Result<bool> {
        let host = host.trim();
        if host.is_empty() || self.contains(host) {
            return Ok(false);
        }
        self.hosts.insert(0, host.to_string());
        self.write().map(|()| true)
    }

    /// Removes the host. Returns whether it was there.
    pub fn remove(&mut self, host: &str) -> std::io::Result<bool> {
        let before = self.hosts.len();
        self.hosts
            .retain(|kept| !kept.eq_ignore_ascii_case(host.trim()));
        if self.hosts.len() == before {
            return Ok(false);
        }
        self.write().map(|()| true)
    }

    /// Adds the host, or removes it when it is a favorite. Returns whether it is one now.
    pub fn toggle(&mut self, host: &str) -> std::io::Result<bool> {
        if self.contains(host) {
            self.remove(host).map(|_| false)
        } else {
            self.add(host)
        }
    }

    fn write(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_vec(&self.hosts).map_err(std::io::Error::other)?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, data)?;
        std::fs::rename(&temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_added_once_found_whatever_the_case_and_removed() {
        let path =
            std::env::temp_dir().join(format!("rvrs-test-favorites-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let mut favorites = Favorites::at(path.clone());
        assert!(favorites.add("example.com").unwrap());
        assert!(!favorites.add("Example.com").unwrap());
        assert!(favorites.add("1.1.1.1").unwrap());
        assert_eq!(
            Favorites::at(path.clone()).hosts,
            vec!["1.1.1.1", "example.com"]
        );
        assert!(!favorites.toggle("EXAMPLE.COM").unwrap());
        assert!(favorites.toggle("example.com").unwrap());
        assert!(favorites.remove("1.1.1.1").unwrap());
        assert!(!favorites.remove("1.1.1.1").unwrap());
        assert!(!favorites.add("  ").unwrap());
        let _ = std::fs::remove_file(path);
    }
}
