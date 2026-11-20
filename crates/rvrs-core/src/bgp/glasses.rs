//! The user's looking glasses, in `looking-glasses.json` in the config directory. Every BGP
//! lookup of a route asks each of them for the prefix.

use std::path::PathBuf;

use super::glass::LookingGlass;
use crate::paths;

pub struct Glasses {
    path: PathBuf,
    pub list: Vec<LookingGlass>,
}

impl Glasses {
    pub fn open() -> Self {
        Self::at(paths::config_dir().join("looking-glasses.json"))
    }

    pub fn at(path: PathBuf) -> Self {
        let list = std::fs::read(&path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        Self { path, list }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Hosts compare without regard to case, as names do.
    pub fn find(&self, host: &str) -> Option<&LookingGlass> {
        self.list
            .iter()
            .find(|glass| glass.host.eq_ignore_ascii_case(host.trim()))
    }

    /// Adds the looking glass, or puts it where the one with the same host was. Returns true
    /// when it replaced one.
    pub fn upsert(&mut self, glass: LookingGlass) -> std::io::Result<bool> {
        let place = self
            .list
            .iter()
            .position(|known| known.host.eq_ignore_ascii_case(&glass.host));
        match place {
            Some(index) => self.list[index] = glass,
            None => self.list.push(glass),
        }
        self.write().map(|()| place.is_some())
    }

    /// Removes the looking glass with this host. Returns whether it was there.
    pub fn remove(&mut self, host: &str) -> std::io::Result<bool> {
        let before = self.list.len();
        self.list
            .retain(|glass| !glass.host.eq_ignore_ascii_case(host.trim()));
        if self.list.len() == before {
            return Ok(false);
        }
        self.write().map(|()| true)
    }

    fn write(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_vec_pretty(&self.list).map_err(std::io::Error::other)?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, data)?;
        std::fs::rename(&temporary, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bgp::glass::GlassKind;

    fn glass(host: &str, name: &str) -> LookingGlass {
        serde_json::from_value(serde_json::json!({ "name": name, "host": host })).unwrap()
    }

    #[test]
    fn looking_glasses_are_kept_by_host() {
        let path = std::env::temp_dir().join(format!(
            "rvrs-test-glasses-{}/looking-glasses.json",
            std::process::id()
        ));
        let mut glasses = Glasses::at(path.clone());
        assert!(glasses.list.is_empty());
        assert!(!glasses
            .upsert(glass("lg.example.net", "Example IX"))
            .unwrap());
        assert!(!glasses
            .upsert(glass("lg.example.org", "Example Net"))
            .unwrap());
        assert!(glasses
            .upsert(glass("LG.example.net", "Example IX 2"))
            .unwrap());

        let again = Glasses::at(path.clone());
        assert_eq!(again.list.len(), 2);
        assert_eq!(again.list[0].name, "Example IX 2");
        assert_eq!(again.list[0].kind, GlassKind::AliceLg);
        assert!(again.find("lg.Example.org").is_some());

        assert!(glasses.remove("lg.example.net").unwrap());
        assert!(!glasses.remove("lg.example.net").unwrap());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
