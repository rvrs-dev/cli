//! RIPEstat's data API: one GET per data call, JSON with a `data` object and the time the server
//! answered. Everything the BGP tool shows about collectors, RPKI and registries comes through
//! here, so an rvrs API can take its place later without the rest noticing.

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Deserialize;

const SOURCE_APP: &str = "rvrs-cli";

pub struct Answer<Payload> {
    pub data: Payload,
    /// When the server answered: the retrieval time, not the time the data describes.
    pub time: Option<u64>,
}

#[derive(Deserialize)]
struct Envelope<Payload> {
    data: Payload,
    time: Option<String>,
}

/// One agent for a lookup. Its connections stay open, so the calls after the first skip the
/// TLS handshake.
pub fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into()
}

/// The URL of a data call, its parameters in alphabetical order.
pub fn url(call: &str, query: &[(&str, &str)]) -> String {
    let mut query: Vec<(&str, &str)> = query.to_vec();
    query.sort();
    let mut url = format!("https://stat.ripe.net/data/{call}/data.json?");
    for (key, value) in query {
        url.push_str(&format!("{key}={}&", encode(value)));
    }
    url.push_str(&format!("sourceapp={SOURCE_APP}"));
    url
}

fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b':' | b'/' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Asks one data call. `None` when the server didn't answer in `timeout` seconds, answered
/// with an error, or sent something the call's shape doesn't fit.
pub fn get<Payload: DeserializeOwned>(
    agent: &ureq::Agent,
    call: &str,
    query: &[(&str, &str)],
    timeout: u64,
) -> Option<Answer<Payload>> {
    let mut response = agent
        .get(url(call, query))
        .config()
        .timeout_global(Some(Duration::from_secs(timeout)))
        .build()
        .call()
        .ok()?;
    let text = response.body_mut().read_to_string().ok()?;
    decode(&text)
}

pub fn decode<Payload: DeserializeOwned>(text: &str) -> Option<Answer<Payload>> {
    let envelope: Envelope<Payload> = serde_json::from_str(text).ok()?;
    Some(Answer {
        data: envelope.data,
        time: envelope.time.as_deref().and_then(date),
    })
}

/// RIPEstat writes UTC without a zone: "2026-11-15T08:00:00", sometimes with fractions.
pub fn date(text: &str) -> Option<u64> {
    let whole = text.split('.').next().unwrap_or(text);
    let parts: Vec<i64> = whole
        .split(['-', 'T', ':', 'Z', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().ok())
        .collect::<Option<_>>()?;
    if parts.len() < 3 {
        return None;
    }
    let (year, month, day) = (parts[0], parts[1], parts[2]);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Days since 1970-01-01 from a civil date (Howard Hinnant's algorithm).
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let part = |index: usize| parts.get(index).copied().unwrap_or(0);
    let seconds = days * 86_400 + part(3) * 3600 + part(4) * 60 + part(5);
    u64::try_from(seconds).ok()
}

/// "2026-11-08T00:00" for a `starttime` parameter.
pub fn stamp(seconds: u64) -> String {
    format!("{}:00", &crate::format::iso8601(seconds)[..13])
}

/// An AS number that the API writes as 64500 in one call and "64500" in the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Number(pub u32);

impl<'de> Deserialize<'de> for Number {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Whole(u64),
            Float(f64),
            Text(String),
        }
        let number = match Raw::deserialize(deserializer)? {
            Raw::Whole(number) => u32::try_from(number).ok(),
            Raw::Float(number) if number >= 0.0 && number <= f64::from(u32::MAX) => {
                Some(number as u32)
            }
            Raw::Float(_) => None,
            Raw::Text(text) => text.trim().parse().ok(),
        };
        number
            .map(Number)
            .ok_or_else(|| serde::de::Error::custom("not an AS number"))
    }
}

#[derive(Deserialize)]
pub struct NetworkInfo {
    pub asns: Vec<Number>,
    pub prefix: Option<String>,
}

#[derive(Deserialize)]
pub struct Holder {
    pub asn: Number,
    pub holder: Option<String>,
}

#[derive(Deserialize)]
pub struct Block {
    pub desc: Option<String>,
}

#[derive(Deserialize)]
pub struct PrefixOverview {
    pub announced: Option<bool>,
    pub asns: Vec<Holder>,
    pub block: Option<Block>,
    pub query_time: Option<String>,
}

#[derive(Deserialize)]
pub struct AsOverview {
    pub holder: Option<String>,
    pub announced: Option<bool>,
    pub block: Option<Block>,
    pub query_endtime: Option<String>,
}

#[derive(Deserialize)]
pub struct ValidatingRoa {
    pub origin: Number,
    pub prefix: String,
    pub validity: String,
    pub max_length: u32,
}

#[derive(Deserialize)]
pub struct Validation {
    pub status: String,
    pub validating_roas: Option<Vec<ValidatingRoa>>,
}

#[derive(Deserialize)]
pub struct Seen {
    pub time: Option<String>,
}

#[derive(Deserialize)]
pub struct FamilyVisibility {
    pub ris_peers_seeing: u64,
    pub total_ris_peers: u64,
}

#[derive(Deserialize)]
pub struct Visibility {
    pub v4: Option<FamilyVisibility>,
    pub v6: Option<FamilyVisibility>,
}

#[derive(Deserialize)]
pub struct StatusOrigin {
    pub origin: Number,
    pub route_objects: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct Specific {
    pub prefix: String,
    pub origin: Option<Number>,
}

#[derive(Deserialize)]
pub struct RoutingStatus {
    pub first_seen: Option<Seen>,
    pub visibility: Option<Visibility>,
    pub origins: Option<Vec<StatusOrigin>>,
    pub less_specifics: Option<Vec<Specific>>,
    pub more_specifics: Option<Vec<Specific>>,
    pub query_time: Option<String>,
}

#[derive(Deserialize)]
pub struct Peer {
    pub as_path: String,
    pub community: Option<String>,
    pub last_updated: Option<String>,
    pub prefix: Option<String>,
}

#[derive(Deserialize)]
pub struct Collector {
    pub rrc: String,
    pub location: Option<String>,
    pub peers: Vec<Peer>,
}

#[derive(Deserialize)]
pub struct LookingGlass {
    pub rrcs: Vec<Collector>,
    pub latest_time: Option<String>,
}

#[derive(Deserialize)]
pub struct Timeline {
    pub starttime: String,
    pub endtime: String,
    pub full_peers_seeing: Option<f64>,
}

#[derive(Deserialize)]
pub struct HistoryPrefix {
    pub prefix: String,
    pub timelines: Vec<Timeline>,
}

#[derive(Deserialize)]
pub struct HistoryOrigin {
    pub origin: Number,
    pub prefixes: Vec<HistoryPrefix>,
}

#[derive(Deserialize)]
pub struct RoutingHistory {
    pub by_origin: Vec<HistoryOrigin>,
    pub query_endtime: Option<String>,
}

#[derive(Deserialize)]
pub struct Bucket {
    pub announcements: Option<u64>,
    pub withdrawals: Option<u64>,
    pub starttime: String,
}

#[derive(Deserialize)]
pub struct UpdateActivity {
    pub updates: Vec<Bucket>,
    pub query_endtime: Option<String>,
}

#[derive(Deserialize)]
pub struct NeighbourCounts {
    pub left: Option<u64>,
    pub right: Option<u64>,
    pub uncertain: Option<u64>,
}

#[derive(Deserialize)]
pub struct Neighbour {
    pub asn: Number,
    #[serde(rename = "type")]
    pub side: String,
    pub power: Option<u64>,
}

#[derive(Deserialize)]
pub struct Neighbours {
    pub neighbour_counts: Option<NeighbourCounts>,
    pub neighbours: Vec<Neighbour>,
    pub latest_time: Option<String>,
}

#[derive(Deserialize)]
pub struct Originating<Value> {
    pub originating: Option<Value>,
}

#[derive(Deserialize)]
pub struct Families<Value> {
    pub v4: Option<Originating<Value>>,
    pub v6: Option<Originating<Value>>,
}

#[derive(Deserialize)]
pub struct RisPrefixes {
    pub counts: Option<Families<u64>>,
    pub prefixes: Option<Families<Vec<String>>>,
    pub query_time: Option<String>,
}

#[derive(Deserialize)]
pub struct RegistryEntry {
    pub rir: String,
}

#[derive(Deserialize)]
pub struct Registry {
    pub rirs: Vec<RegistryEntry>,
}

#[derive(Deserialize)]
pub struct Located {
    pub location: Option<String>,
}

#[derive(Deserialize)]
pub struct Country {
    pub located_resources: Vec<Located>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read_as_utc() {
        assert_eq!(date("1970-01-01T00:00:00"), Some(0));
        assert_eq!(date("2027-01-15T08:00:00"), Some(1_800_000_000));
        assert_eq!(date("2027-01-15T08:00:00.869209753Z"), Some(1_800_000_000));
        assert_eq!(date("2027-01-15"), Some(1_799_971_200));
        assert_eq!(date("soon"), None);
        assert_eq!(stamp(1_800_000_000 + 1234), "2027-01-15T08:00");
    }

    #[test]
    fn a_data_call_names_its_parameters_in_order() {
        assert_eq!(
            url(
                "rpki-validation",
                &[("resource", "AS64500"), ("prefix", "2001:db8::/32")]
            ),
            "https://stat.ripe.net/data/rpki-validation/data.json?prefix=2001:db8::/32&resource=AS64500&sourceapp=rvrs-cli"
        );
    }

    #[test]
    fn an_as_number_is_read_as_a_number_or_as_text() {
        let answer: Answer<NetworkInfo> = decode(
            r#"{"data":{"asns":["64500",64501,64502.0],"prefix":"192.0.2.0/24"},"time":"2027-01-15T08:00:00.123456"}"#,
        )
        .unwrap();
        assert_eq!(
            answer.data.asns,
            [Number(64500), Number(64501), Number(64502)]
        );
        assert_eq!(answer.time, Some(1_800_000_000));
        assert!(decode::<NetworkInfo>(r#"{"data":{"asns":["x"]}}"#).is_none());
        assert!(decode::<NetworkInfo>("<html>").is_none());
    }

    #[test]
    fn validation_and_visibility_answers_are_read() {
        let validation: Answer<Validation> = decode(
            r#"{"data":{"status":"invalid_length","validator":"routinator","validating_roas":[{"origin":"64500","prefix":"192.0.2.0/24","validity":"invalid_length","source":"RIPE NCC","max_length":24}]}}"#,
        )
        .unwrap();
        assert_eq!(validation.data.status, "invalid_length");
        assert_eq!(validation.data.validating_roas.unwrap()[0].max_length, 24);

        let status: Answer<RoutingStatus> = decode(
            r#"{"data":{"first_seen":{"prefix":"192.0.2.0/24","origin":"64500","time":"2017-07-14T02:40:00"},"visibility":{"v4":{"ris_peers_seeing":318,"total_ris_peers":320},"v6":{"ris_peers_seeing":0,"total_ris_peers":330}},"origins":[{"origin":64500,"route_objects":["radb"]}],"less_specifics":[],"more_specifics":[{"prefix":"192.0.2.128/25","origin":64496}],"query_time":"2027-01-15T00:00:00"}}"#,
        )
        .unwrap();
        assert_eq!(
            status.data.visibility.unwrap().v4.unwrap().ris_peers_seeing,
            318
        );
        assert_eq!(
            status.data.more_specifics.unwrap()[0].origin,
            Some(Number(64496))
        );

        let prefixes: Answer<RisPrefixes> = decode(
            r#"{"data":{"counts":{"v4":{"originating":2,"transiting":0},"v6":{"originating":1,"transiting":0}},"prefixes":{"v4":{"originating":["198.51.100.0/24","192.0.2.0/24"],"transiting":[]},"v6":{"originating":["2001:db8::/32"],"transiting":[]}},"query_time":"2027-01-15T00:00:00"}}"#,
        )
        .unwrap();
        assert_eq!(
            prefixes.data.counts.unwrap().v4.unwrap().originating,
            Some(2)
        );
    }
}
