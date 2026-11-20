//! IP-to-network lookups through RIPEstat, RIPE NCC's free public data API. Private addresses
//! are never sent. Answers are kept on disk for a week, so a hop seen before costs nothing.
//! This is the seam for an rvrs lookup API later.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::model::Network;
use crate::{format, is_private, paths};

const LIFETIME: u64 = 7 * 24 * 3600;
const SOURCE_APP: &str = "rvrs-cli";

/// What a lookup came back with. Only answers the API gave are cached; a timeout is retried the
/// next time the address comes up.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    Found(Network),
    /// The API answered and knows no network for this address.
    Unknown,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    network: Option<Network>,
    stored_at: u64,
}

pub struct Cache {
    path: PathBuf,
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl Cache {
    pub fn open() -> Self {
        Self::at(paths::cache_dir().join("networks.json"))
    }

    pub fn at(path: PathBuf) -> Self {
        let now = format::now();
        let mut entries: BTreeMap<String, Entry> = std::fs::read(&path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default();
        entries.retain(|_, entry| now.saturating_sub(entry.stored_at) <= LIFETIME);
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    /// `Some(None)` is a cached "no network".
    pub fn get(&self, address: IpAddr) -> Option<Option<Network>> {
        self.entries
            .lock()
            .ok()?
            .get(&address.to_string())
            .map(|entry| entry.network.clone())
    }

    pub fn store(&self, address: IpAddr, network: Option<Network>) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(
                address.to_string(),
                Entry {
                    network,
                    stored_at: format::now(),
                },
            );
        }
    }

    pub fn flush(&self) {
        let Ok(entries) = self.entries.lock() else {
            return;
        };
        let Ok(data) = serde_json::to_vec(&*entries) else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let temporary = self.path.with_extension("json.tmp");
        if std::fs::write(&temporary, data).is_ok() {
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

#[derive(Deserialize)]
struct PrefixOverview {
    data: PrefixData,
}

#[derive(Deserialize)]
struct PrefixData {
    asns: Vec<Origin>,
    resource: Option<String>,
}

#[derive(Deserialize)]
struct Origin {
    asn: u32,
    holder: String,
}

#[derive(Deserialize)]
struct Geo {
    data: GeoData,
}

#[derive(Deserialize)]
struct GeoData {
    located_resources: Vec<Located>,
}

#[derive(Deserialize)]
struct Located {
    locations: Vec<Location>,
}

#[derive(Deserialize)]
struct Location {
    country: Option<String>,
    city: Option<String>,
}

/// Asks RIPEstat which network announces `address`. Blocks for up to the request timeout.
pub fn lookup(address: IpAddr) -> Lookup {
    lookup_with(&agent(), address)
}

/// One HTTP agent for a batch of lookups. It keeps its connection open, so every lookup after
/// the first skips the TLS handshake.
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(6)))
        .build()
        .into()
}

pub fn lookup_with(agent: &ureq::Agent, address: IpAddr) -> Lookup {
    match announced(agent, address) {
        Lookup::Found(mut network) => {
            locate(agent, address, &mut network);
            Lookup::Found(network)
        }
        other => other,
    }
}

/// The first half of a lookup: which network announces the address. This is what a result
/// needs, so callers that show results as they come ask for it alone and call [`locate`] after.
pub fn announced(agent: &ureq::Agent, address: IpAddr) -> Lookup {
    if is_private(address) {
        return Lookup::Unknown;
    }
    let Some(overview) = get::<PrefixOverview>(agent, "prefix-overview", &address.to_string())
    else {
        return Lookup::Failed;
    };
    let Some(origin) = overview.data.asns.first() else {
        return Lookup::Unknown;
    };
    Lookup::Found(Network {
        number: origin.asn,
        name: clean_holder(&origin.holder),
        country: None,
        city: None,
        prefix: overview.data.resource.filter(|prefix| prefix.contains('/')),
    })
}

/// The second half: where the address is, by MaxMind's reckoning. Leaves the network as it is
/// when the answer doesn't come.
pub fn locate(agent: &ureq::Agent, address: IpAddr, network: &mut Network) {
    let Some(geo) = get::<Geo>(agent, "maxmind-geo-lite", &address.to_string()) else {
        return;
    };
    if let Some(location) = geo
        .data
        .located_resources
        .first()
        .and_then(|located| located.locations.first())
    {
        network.country = location.country.clone().filter(|text| !text.is_empty());
        network.city = location.city.clone().filter(|text| !text.is_empty());
    }
}

fn get<Payload: serde::de::DeserializeOwned>(
    agent: &ureq::Agent,
    endpoint: &str,
    resource: &str,
) -> Option<Payload> {
    let url = format!("https://stat.ripe.net/data/{endpoint}/data.json?resource={resource}&sourceapp={SOURCE_APP}");
    // One retry: a burst of hops can trip a momentary refusal.
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(400));
        }
        let Ok(mut response) = agent.get(&url).call() else {
            continue;
        };
        let Ok(text) = response.body_mut().read_to_string() else {
            continue;
        };
        if let Ok(payload) = serde_json::from_str(&text) {
            return Some(payload);
        }
    }
    None
}

#[derive(Deserialize)]
struct WhatsMyIp {
    data: WhatsMyIpData,
}

#[derive(Deserialize)]
struct WhatsMyIpData {
    ip: String,
}

/// The address the internet sees this device as, by RIPEstat's account. Kept for five minutes
/// in the process, so the Console asks once for a row of runs.
pub fn public_address(agent: &ureq::Agent) -> Option<IpAddr> {
    use std::sync::Mutex;
    use std::time::Instant;
    static KNOWN: Mutex<Option<(Instant, IpAddr)>> = Mutex::new(None);
    if let Ok(known) = KNOWN.lock() {
        if let Some((asked, address)) = *known {
            if asked.elapsed() < Duration::from_secs(300) {
                return Some(address);
            }
        }
    }
    let url = format!("https://stat.ripe.net/data/whats-my-ip/data.json?sourceapp={SOURCE_APP}");
    let mut response = agent.get(&url).call().ok()?;
    let text = response.body_mut().read_to_string().ok()?;
    let address: IpAddr = serde_json::from_str::<WhatsMyIp>(&text)
        .ok()?
        .data
        .ip
        .parse()
        .ok()?;
    if let Ok(mut known) = KNOWN.lock() {
        *known = Some((Instant::now(), address));
    }
    Some(address)
}

/// RIPE holders read "CLOUDFLARENET - Cloudflare, Inc."; keep the human half.
pub fn clean_holder(holder: &str) -> String {
    match holder.split_once(" - ") {
        Some((_, tail)) if !tail.trim().is_empty() => tail.trim().to_string(),
        _ => holder.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holders_keep_the_human_half() {
        assert_eq!(
            clean_holder("CLOUDFLARENET - Cloudflare, Inc."),
            "Cloudflare, Inc."
        );
        assert_eq!(clean_holder("DTAG"), "DTAG");
        assert_eq!(clean_holder("X - "), "X - ");
    }

    #[test]
    fn private_addresses_are_never_sent() {
        assert_eq!(lookup("192.168.1.1".parse().unwrap()), Lookup::Unknown);
    }

    #[test]
    fn the_cache_remembers_a_missing_network_too() {
        let path =
            std::env::temp_dir().join(format!("rvrs-test-networks-{}.json", std::process::id()));
        let cache = Cache::at(path.clone());
        let address: IpAddr = "203.0.113.9".parse().unwrap();
        assert_eq!(cache.get(address), None);
        cache.store(address, None);
        cache.flush();
        assert_eq!(Cache::at(path.clone()).get(address), Some(None));
        let _ = std::fs::remove_file(path);
    }
}
