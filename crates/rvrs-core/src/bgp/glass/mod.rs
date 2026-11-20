//! Looking glasses: what an operator's own routers or an exchange's route servers hold for a
//! prefix. One front door for every type: check it ([`describe`]), ask it for a prefix
//! ([`routes`]), test it ([`test`]). Alice-LG and hyperglass have a JSON API; lg.cgi and
//! bird-lg-go are pages around a router's own output, which `router_text` reads.

mod alice;
mod birdlg;
mod http;
mod hyperglass;
mod lgcgi;
mod router_text;
mod scan;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The looking glass software rvrs can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GlassKind {
    /// Alice-LG: an internet exchange's route servers, through its JSON API.
    #[default]
    #[serde(rename = "alice-lg")]
    AliceLg,
    /// The classic lg.cgi form in front of an operator's routers.
    #[serde(rename = "lg-cgi")]
    LgCgi,
    /// hyperglass, through its JSON API.
    #[serde(rename = "hyperglass")]
    Hyperglass,
    /// bird-lg-go: pages around BIRD's output.
    #[serde(rename = "bird-lg-go")]
    BirdLg,
}

impl GlassKind {
    pub const ALL: [GlassKind; 4] = [
        GlassKind::AliceLg,
        GlassKind::LgCgi,
        GlassKind::Hyperglass,
        GlassKind::BirdLg,
    ];

    /// The name a kind is saved and typed as.
    pub fn id(self) -> &'static str {
        match self {
            GlassKind::AliceLg => "alice-lg",
            GlassKind::LgCgi => "lg-cgi",
            GlassKind::Hyperglass => "hyperglass",
            GlassKind::BirdLg => "bird-lg-go",
        }
    }

    /// Reads a kind as it was typed: its id, or one of the shorter names people use for it.
    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_lowercase().as_str() {
            "alice-lg" | "alice" => Some(GlassKind::AliceLg),
            "lg-cgi" | "lgcgi" | "lg.cgi" => Some(GlassKind::LgCgi),
            "hyperglass" => Some(GlassKind::Hyperglass),
            "bird-lg-go" | "birdlg" | "bird-lg" => Some(GlassKind::BirdLg),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            GlassKind::AliceLg => "Alice-LG",
            GlassKind::LgCgi => "lg.cgi",
            GlassKind::Hyperglass => "hyperglass",
            GlassKind::BirdLg => "bird-lg-go",
        }
    }

    /// A host of this type, to show where one is asked for.
    pub fn example(self) -> &'static str {
        match self {
            GlassKind::AliceLg => "lg.de-cix.net",
            GlassKind::LgCgi => "lg.ip.tdc.net",
            GlassKind::Hyperglass => "lg.nforce.com",
            GlassKind::BirdLg => "lg.burble.com",
        }
    }

    /// Whether it lists routers to pick from after the check.
    pub fn has_routers(self) -> bool {
        self != GlassKind::AliceLg
    }

    /// What one of its answers comes from: a route server or a router.
    pub fn unit(self) -> &'static str {
        match self {
            GlassKind::AliceLg => "route server",
            GlassKind::LgCgi | GlassKind::Hyperglass | GlassKind::BirdLg => "router",
        }
    }
}

/// A looking glass the user added: it is asked on every BGP lookup of a route and shows what
/// its routers or route servers hold. Alice-LG (internet exchanges) has a JSON API; lg.cgi
/// (operators' own routers) is a web form whose router output rvrs reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LookingGlass {
    pub name: String,
    /// The host, as in "lg.de-cix.net". Always asked over HTTPS.
    pub host: String,
    /// Looking glasses saved before there were kinds are Alice-LG.
    #[serde(default)]
    pub kind: GlassKind,
    /// The routers to ask, picked when it was added. Alice-LG has none.
    #[serde(default)]
    pub routers: Vec<String>,
    /// lg.cgi: the family values its form takes.
    #[serde(default)]
    pub protocols: Vec<String>,
    /// The operator's AS number, which its routers leave out of their AS paths.
    #[serde(default)]
    pub asn: Option<u32>,
    /// lg.cgi: where its form posts, "/" or "/lg.cgi".
    #[serde(default)]
    pub form_path: Option<String>,
    /// lg.cgi: the form takes the prefix length in a field of its own.
    #[serde(default)]
    pub splits_prefix: bool,
    /// hyperglass: its API version, 1 or 2.
    #[serde(default)]
    pub api_version: Option<u8>,
    /// hyperglass: the BGP route query each device takes ("bgp_route", or a version 2
    /// directive).
    #[serde(default)]
    pub query_types: BTreeMap<String, String>,
    /// hyperglass 1: the VRF each device is asked in.
    #[serde(default)]
    pub vrfs: BTreeMap<String, String>,
}

impl LookingGlass {
    /// Reads what was typed or pasted: a host, or a URL whose host is kept.
    pub fn host_from(raw: &str) -> Option<String> {
        let text = raw.trim().to_lowercase();
        if text.is_empty() {
            return None;
        }
        let rest = text
            .split_once("://")
            .map_or(text.as_str(), |(_, rest)| rest);
        let authority = rest.split(['/', '?', '#']).next()?;
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let host = host.split_once(':').map_or(host, |(host, _)| host);
        let valid = host.contains('.')
            && !host.ends_with('.')
            && host
                .chars()
                .all(|character| character.is_alphanumeric() || "-._".contains(character));
        valid.then(|| host.to_string())
    }

    /// The looking glass a check found, under the name and with the routers the user picked.
    pub fn from_setup(setup: &GlassSetup, name: &str, host: &str, routers: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            host: host.to_string(),
            kind: setup.kind,
            routers,
            protocols: setup.protocols.clone(),
            asn: setup.asn,
            form_path: setup.form_path.clone(),
            splits_prefix: setup.splits_prefix,
            api_version: setup.api_version,
            query_types: setup.query_types.clone(),
            vrfs: setup.vrfs.clone(),
        }
    }
}

/// What checking a looking glass found, before it is saved: its routers and what it takes to
/// ask them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlassSetup {
    pub kind: GlassKind,
    pub routers: Vec<String>,
    pub name: Option<String>,
    pub asn: Option<u32>,
    pub protocols: Vec<String>,
    pub form_path: Option<String>,
    pub splits_prefix: bool,
    pub api_version: Option<u8>,
    pub query_types: BTreeMap<String, String>,
    pub vrfs: BTreeMap<String, String>,
}

/// One route a looking glass holds for a prefix: who sent it to which route server, and the AS
/// path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlassRoute {
    pub route_server: String,
    pub neighbor: Option<u32>,
    pub neighbor_name: Option<String>,
    /// From the route server's peer to the origin AS, prepending collapsed.
    pub asns: Vec<u32>,
    /// The route server picked this route as its best.
    pub is_best: bool,
    /// The route server received the route and did not accept it.
    pub is_filtered: bool,
    pub communities: Vec<String>,
    /// The router's own origin validation of the route (valid, invalid, unknown, unverified).
    pub rpki: Option<String>,
    /// The prefix the route is for when the router holds none for exactly the one asked.
    pub network: Option<String>,
}

/// The origin AS's BGP sessions with a looking glass's route servers: the evidence behind "no
/// route". No session means the network isn't on the route servers; sessions that send nothing
/// mean it is connected and keeps its routes to itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlassSessions {
    pub asn: u32,
    pub total: usize,
    pub up: usize,
    /// Routes the route servers received over those sessions, summed.
    pub received: u64,
}

/// What one looking glass answered for a prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GlassResult {
    pub host: String,
    pub name: String,
    #[serde(default)]
    pub kind: GlassKind,
    #[serde(default)]
    pub routes: Vec<GlassRoute>,
    #[serde(default)]
    pub sessions: Option<GlassSessions>,
    /// When the looking glass last read its route servers, if it says, in Unix seconds.
    #[serde(default)]
    pub observed_at: Option<u64>,
    #[serde(default)]
    pub failed: bool,
}

/// One distinct AS path a looking glass holds, with the route servers or routers that hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlassPath {
    pub asns: Vec<u32>,
    pub neighbor_name: Option<String>,
    pub route_servers: Vec<String>,
    pub is_best: bool,
    pub is_filtered: bool,
    pub communities: Vec<String>,
    pub rpki: Option<String>,
    pub network: Option<String>,
}

impl GlassResult {
    /// An answer with nothing in it yet.
    fn new(glass: &LookingGlass, kind: GlassKind) -> Self {
        Self {
            host: glass.host.clone(),
            name: glass.name.clone(),
            kind,
            routes: Vec::new(),
            sessions: None,
            observed_at: None,
            failed: false,
        }
    }

    pub fn accepted(&self) -> Vec<&GlassRoute> {
        self.routes
            .iter()
            .filter(|route| !route.is_filtered)
            .collect()
    }

    pub fn filtered(&self) -> Vec<&GlassRoute> {
        self.routes
            .iter()
            .filter(|route| route.is_filtered)
            .collect()
    }

    pub fn route_server_count(&self) -> usize {
        self.routes
            .iter()
            .map(|route| route.route_server.as_str())
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn unit(&self) -> &'static str {
        self.kind.unit()
    }

    /// "8 routes · 4 route servers", "3 routes · 2 routers", "No route", "No session" or
    /// "No answer".
    pub fn summary(&self) -> String {
        if self.failed {
            return "No answer".to_string();
        }
        if self.routes.is_empty() {
            let no_session = self
                .sessions
                .as_ref()
                .is_some_and(|sessions| sessions.total == 0);
            return if no_session { "No session" } else { "No route" }.to_string();
        }
        let routes = self.routes.len();
        let servers = self.route_server_count();
        format!(
            "{routes} route{} \u{00B7} {servers} {}{}",
            plural(routes),
            self.unit(),
            plural(servers)
        )
    }

    /// The distinct AS paths, the ones on the most route servers first, filtered ones last.
    pub fn paths(&self) -> Vec<GlassPath> {
        let mut paths: Vec<(String, GlassPath)> = Vec::new();
        for route in &self.routes {
            let key = format!(
                "{}{}{}",
                joined(&route.asns),
                if route.is_filtered { " f" } else { "" },
                route.network.as_deref().unwrap_or("")
            );
            match paths.iter_mut().find(|(known, _)| *known == key) {
                Some((_, path)) => {
                    if !path.route_servers.contains(&route.route_server) {
                        path.route_servers.push(route.route_server.clone());
                    }
                    path.is_best = path.is_best || route.is_best;
                }
                None => paths.push((
                    key,
                    GlassPath {
                        asns: route.asns.clone(),
                        neighbor_name: route.neighbor_name.clone(),
                        route_servers: vec![route.route_server.clone()],
                        is_best: route.is_best,
                        is_filtered: route.is_filtered,
                        communities: route.communities.clone(),
                        rpki: route.rpki.clone(),
                        network: route.network.clone(),
                    },
                )),
            }
        }
        let mut paths: Vec<GlassPath> = paths.into_iter().map(|(_, path)| path).collect();
        // The sort is stable, so paths on as many route servers stay in the order they came.
        paths.sort_by_key(|path| {
            (
                path.is_filtered,
                std::cmp::Reverse(path.route_servers.len()),
            )
        });
        paths
    }
}

/// Checks that a host runs a looking glass of this kind and reads what it takes to ask it. The
/// error is a sentence to show as it is.
pub fn describe(kind: GlassKind, host: &str) -> Result<GlassSetup, String> {
    let agent = http::agent();
    match kind {
        GlassKind::AliceLg => alice::describe(&agent, host),
        GlassKind::LgCgi => lgcgi::describe(&agent, host),
        GlassKind::Hyperglass => hyperglass::describe(&agent, host),
        GlassKind::BirdLg => birdlg::describe(&agent, host),
    }
}

/// What the looking glass holds for a prefix. Blocks until every chosen router has answered or
/// timed out; routers are asked three at a time.
pub fn routes(prefix: &str, glass: &LookingGlass) -> GlassResult {
    let agent = http::agent();
    match glass.kind {
        GlassKind::AliceLg => alice::routes(&agent, prefix, glass),
        GlassKind::LgCgi => lgcgi::routes(&agent, prefix, glass),
        GlassKind::Hyperglass => hyperglass::routes(&agent, prefix, glass),
        GlassKind::BirdLg => birdlg::routes(&agent, prefix, glass),
    }
}

/// The sessions an AS has with the looking glass's route servers. Only Alice-LG says; the
/// others give nothing.
pub fn sessions(asn: u32, glass: &LookingGlass) -> Option<GlassSessions> {
    match glass.kind {
        GlassKind::AliceLg => alice::sessions(&http::agent(), asn, glass),
        GlassKind::LgCgi | GlassKind::Hyperglass | GlassKind::BirdLg => None,
    }
}

/// One step of a test: what was done, what came back, and whether that is good.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestStep {
    pub title: String,
    pub detail: String,
    pub passed: bool,
}

/// The result of testing a looking glass: valid or not, the steps it took, and the raw answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestResult {
    pub passed: bool,
    pub headline: String,
    pub steps: Vec<TestStep>,
    pub raw: Option<String>,
}

/// Asks one router (or the route servers) for a prefix everyone carries and says what came
/// back.
pub fn test(glass: &LookingGlass, router: Option<&str>, prefix: &str) -> TestResult {
    let agent = http::agent();
    let mut steps: Vec<TestStep> = Vec::new();
    let mut raw: Option<String> = None;
    let routes: Vec<GlassRoute>;
    let dot = '\u{00B7}';
    let step = |title: &str, detail: String, passed: bool| TestStep {
        title: title.to_string(),
        detail,
        passed,
    };
    let host = &glass.host;
    match (glass.kind, router) {
        (GlassKind::AliceLg, _) => {
            let start = Instant::now();
            let result = alice::routes(&agent, prefix, glass);
            let ms = start.elapsed().as_millis();
            steps.push(step(
                "Request",
                format!("GET https://{host}/api/v1/lookup/prefix?q={prefix} {dot} {ms} ms"),
                !result.failed,
            ));
            routes = result.routes;
        }
        (_, None) => {
            return TestResult {
                passed: false,
                headline: format!("Invalid {dot} no router picked"),
                steps: Vec::new(),
                raw: None,
            };
        }
        (GlassKind::Hyperglass, Some(router)) => {
            let answer = hyperglass::query(
                &agent,
                host,
                glass.api_version.unwrap_or(2),
                router,
                glass
                    .query_types
                    .get(router)
                    .map_or("bgp_route", String::as_str),
                glass.vrfs.get(router).map(String::as_str),
                prefix,
            );
            let status = answer
                .status
                .map_or("none".to_string(), |status| status.to_string());
            steps.push(step(
                "Request",
                format!(
                    "POST {} {dot} HTTP {status} {dot} {} ms",
                    answer.url, answer.milliseconds
                ),
                answer.is_success(),
            ));
            raw = answer
                .output
                .clone()
                .or_else(|| {
                    answer
                        .structured
                        .as_ref()
                        .and_then(|routes| serde_json::to_string(routes).ok())
                })
                .or_else(|| answer.message.clone());
            routes = hyperglass::read(&answer, prefix, router, glass.asn);
        }
        (GlassKind::BirdLg, Some(router)) => {
            let response = birdlg::page(&agent, host, router, prefix);
            let status = response
                .status
                .map_or("none".to_string(), |status| status.to_string());
            steps.push(step(
                "Request",
                format!(
                    "GET {} {dot} HTTP {status} {dot} {} ms",
                    response.url, response.milliseconds
                ),
                response.status == Some(200),
            ));
            raw = response
                .text()
                .map(|html| router_text::plain(birdlg::preformatted(html)));
            routes = raw.as_deref().map_or_else(Vec::new, |text| {
                router_text::routes(text, prefix, router, glass.asn)
            });
        }
        (GlassKind::LgCgi, Some(router)) => {
            let start = Instant::now();
            let page = lgcgi::ask(&agent, prefix, glass, router);
            let ms = start.elapsed().as_millis();
            let path = glass.form_path.as_deref().unwrap_or("/");
            steps.push(step(
                "Request",
                format!("POST https://{host}{path} {dot} {ms} ms"),
                page.is_some(),
            ));
            routes = page
                .as_deref()
                .map_or_else(Vec::new, |page| lgcgi::read(page, prefix, glass, router));
            raw = page.as_deref().map(lgcgi::raw_output);
        }
    }
    if glass.kind != GlassKind::AliceLg {
        let format = raw.as_deref().and_then(router_text::format);
        if glass.kind != GlassKind::Hyperglass || raw.is_some() {
            steps.push(step(
                "Router output",
                format.map_or_else(
                    || "Not Junos, Cisco, FRR or BIRD output".to_string(),
                    |format| format!("Read as {}", format.title()),
                ),
                format.is_some() || (glass.kind == GlassKind::Hyperglass && !routes.is_empty()),
            ));
        }
    }
    let count = routes.len();
    let paths = routes
        .iter()
        .map(|route| joined(&route.asns))
        .collect::<BTreeSet<_>>()
        .len();
    // Route servers carry only what members send them, so no route is an answer there. A
    // router with a full table carries every routed prefix, so none means it wasn't read.
    steps.push(step(
        "Routes",
        if routes.is_empty() {
            format!("No route for {prefix}")
        } else {
            format!(
                "{count} route{}, {paths} distinct AS path{}",
                plural(count),
                plural(paths)
            )
        },
        !routes.is_empty() || glass.kind == GlassKind::AliceLg,
    ));
    let failed = steps.iter().find(|step| !step.passed);
    let headline = match failed {
        None if routes.is_empty() => format!("Valid {dot} no route for {prefix}"),
        None => format!("Valid {dot} {count} route{} for {prefix}", plural(count)),
        Some(step) => format!("Invalid {dot} {} failed", step.title.to_lowercase()),
    };
    TestResult {
        passed: failed.is_none(),
        headline,
        steps,
        raw: raw.map(|text| text.chars().take(4000).collect()),
    }
}

/// Reads a time written as UTC without a zone, as "2026-11-15T08:00:00", with or without
/// fractions and a "Z", into Unix seconds. Alice-LG writes more fraction digits than most
/// parsers take, so the fraction is dropped.
pub fn parse_time(text: &str) -> Option<u64> {
    let whole = text
        .split('.')
        .find(|part| !part.is_empty())
        .unwrap_or(text);
    let parts: Vec<i64> = whole
        .split(['-', 'T', ':', 'Z', ' '])
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    if parts.len() < 3 || parts.iter().any(|part| !(0..=9999).contains(part)) {
        return None;
    }
    let part = |index: usize| parts.get(index).copied().unwrap_or(0);
    // A month past December runs into the next year, as a calendar reads it.
    let months = part(0) * 12 + part(1) - 1;
    let days = days_from_civil(months.div_euclid(12), months.rem_euclid(12) + 1, 1) + part(2) - 1;
    u64::try_from(days * 86400 + part(3) * 3600 + part(4) * 60 + part(5)).ok()
}

/// Days from 1970-01-01 to a date of the Gregorian calendar (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Asks a looking glass's chosen routers, three at a time, and gathers what they hold. `ask`
/// gives nothing when the request failed; a router that printed no route answered too.
fn fan_out<Ask>(glass: &LookingGlass, kind: GlassKind, ask: Ask) -> GlassResult
where
    Ask: Fn(&str) -> Option<Vec<GlassRoute>> + Sync,
{
    let mut result = GlassResult::new(glass, kind);
    let routers = &glass.routers;
    let next = AtomicUsize::new(0);
    let answers: Mutex<Vec<(usize, Vec<GlassRoute>)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..routers.len().min(3) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(router) = routers.get(index) else {
                    break;
                };
                if let Some(routes) = ask(router) {
                    answers
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push((index, routes));
                }
            });
        }
    });
    let mut answers = answers.into_inner().unwrap_or_else(PoisonError::into_inner);
    // The routers' own order, whichever answered first, so a result reads the same every time.
    answers.sort_by_key(|(index, _)| *index);
    let answered = answers.len();
    result.routes = answers.into_iter().flat_map(|(_, routes)| routes).collect();
    result.failed = answered == 0 && !routers.is_empty();
    result.observed_at = (answered > 0).then(now);
    result
}

/// The time in Unix seconds.
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// AS numbers as one text, for telling paths apart.
fn joined(asns: &[u32]) -> String {
    asns.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glass(kind: GlassKind, routers: &[&str]) -> LookingGlass {
        LookingGlass {
            name: "Example".to_string(),
            host: "lg.example.net".to_string(),
            kind,
            routers: routers.iter().map(|router| router.to_string()).collect(),
            protocols: Vec::new(),
            asn: Some(64500),
            form_path: Some("/".to_string()),
            splits_prefix: false,
            api_version: None,
            query_types: BTreeMap::new(),
            vrfs: BTreeMap::new(),
        }
    }

    fn route(server: &str, asns: &[u32]) -> GlassRoute {
        GlassRoute {
            route_server: server.to_string(),
            neighbor: asns.first().copied(),
            neighbor_name: None,
            asns: asns.to_vec(),
            is_best: false,
            is_filtered: false,
            communities: Vec::new(),
            rpki: None,
            network: None,
        }
    }

    #[test]
    fn a_host_is_read_from_whatever_was_typed() {
        let host = LookingGlass::host_from;
        assert_eq!(host("lg.de-cix.net").as_deref(), Some("lg.de-cix.net"));
        assert_eq!(
            host(" HTTPS://LG.BCIX.de/routeservers/rs01 ").as_deref(),
            Some("lg.bcix.de")
        );
        assert_eq!(
            host("lg.example.net:8443/x?y=1").as_deref(),
            Some("lg.example.net")
        );
        assert_eq!(host("localhost"), None);
        assert_eq!(host("lg.example.net."), None);
        assert_eq!(host("lg example.net"), None);
        assert_eq!(host(""), None);
    }

    #[test]
    fn every_kind_reads_back_from_its_id() {
        for kind in GlassKind::ALL {
            assert_eq!(GlassKind::from_id(kind.id()), Some(kind));
            let saved = serde_json::to_string(&kind).expect("a kind encodes");
            assert_eq!(saved, format!("\"{}\"", kind.id()));
            assert_eq!(serde_json::from_str::<GlassKind>(&saved).ok(), Some(kind));
        }
        assert_eq!(GlassKind::from_id("Alice"), Some(GlassKind::AliceLg));
        assert_eq!(GlassKind::from_id("LG.CGI"), Some(GlassKind::LgCgi));
        assert_eq!(GlassKind::from_id("lgcgi"), Some(GlassKind::LgCgi));
        assert_eq!(GlassKind::from_id("birdlg"), Some(GlassKind::BirdLg));
        assert_eq!(GlassKind::from_id("bird-lg"), Some(GlassKind::BirdLg));
        assert_eq!(GlassKind::from_id("bgplg"), None);
        assert!(!GlassKind::AliceLg.has_routers());
        assert!(GlassKind::Hyperglass.has_routers());
        assert_eq!(GlassKind::AliceLg.unit(), "route server");
        assert_eq!(GlassKind::BirdLg.unit(), "router");
    }

    #[test]
    fn a_looking_glass_saved_without_a_kind_is_alice_lg() {
        let old: LookingGlass = serde_json::from_str(r#"{"name":"DE-CIX","host":"lg.de-cix.net"}"#)
            .expect("an old looking glass loads");
        assert_eq!(old.kind, GlassKind::AliceLg);
        assert!(old.routers.is_empty());
        assert_eq!(old.asn, None);
        assert!(!old.splits_prefix);

        let saved = glass(GlassKind::LgCgi, &["Copenhagen #1"]);
        let text = serde_json::to_string(&saved).expect("a looking glass encodes");
        assert_eq!(
            serde_json::from_str::<LookingGlass>(&text).ok(),
            Some(saved)
        );
    }

    #[test]
    fn a_checked_looking_glass_keeps_what_the_check_found() {
        let setup = GlassSetup {
            kind: GlassKind::Hyperglass,
            routers: vec!["fra1".to_string(), "ams1".to_string()],
            name: Some("Example Net".to_string()),
            asn: Some(64500),
            protocols: Vec::new(),
            form_path: None,
            splits_prefix: false,
            api_version: Some(1),
            query_types: BTreeMap::from([("fra1".to_string(), "bgp_route".to_string())]),
            vrfs: BTreeMap::from([("fra1".to_string(), "global".to_string())]),
        };
        let glass = LookingGlass::from_setup(
            &setup,
            "Example",
            "lg.example.net",
            vec!["fra1".to_string()],
        );
        assert_eq!(glass.kind, GlassKind::Hyperglass);
        assert_eq!(glass.routers, ["fra1"]);
        assert_eq!(glass.api_version, Some(1));
        assert_eq!(glass.query_types, setup.query_types);
        assert_eq!(glass.vrfs, setup.vrfs);
        assert_eq!(glass.asn, Some(64500));
    }

    #[test]
    fn a_summary_counts_routes_and_where_they_came_from() {
        let mut result = GlassResult::new(&glass(GlassKind::AliceLg, &[]), GlassKind::AliceLg);
        assert_eq!(result.summary(), "No route");
        result.routes = vec![route("rs1", &[64501])];
        assert_eq!(result.summary(), "1 route \u{00B7} 1 route server");
        result.routes.push(route("rs2", &[64501]));
        result.routes.push(route("rs2", &[64502, 64501]));
        assert_eq!(result.summary(), "3 routes \u{00B7} 2 route servers");
        result.kind = GlassKind::LgCgi;
        assert_eq!(result.summary(), "3 routes \u{00B7} 2 routers");
        result.failed = true;
        assert_eq!(result.summary(), "No answer");
    }

    #[test]
    fn a_result_saved_with_only_its_host_and_name_still_loads() {
        let old: GlassResult =
            serde_json::from_str(r#"{"host":"lg.example.net","name":"Example IX"}"#)
                .expect("an old result loads");
        assert_eq!(old.kind, GlassKind::AliceLg);
        assert_eq!(old.unit(), "route server");
        assert_eq!(old.summary(), "No route");
        assert_eq!(old.observed_at, None);
    }

    #[test]
    fn paths_are_folded_with_the_widest_first_and_filtered_ones_last() {
        let mut result = GlassResult::new(&glass(GlassKind::AliceLg, &[]), GlassKind::AliceLg);
        let mut filtered = route("rs1", &[64509, 64501]);
        filtered.is_filtered = true;
        let mut best = route("rs2", &[64502, 64501]);
        best.is_best = true;
        let mut longer = route("rs1", &[64503, 64501]);
        longer.network = Some("203.0.113.0/25".to_string());
        result.routes = vec![
            filtered,
            route("rs1", &[64503, 64501]),
            route("rs1", &[64502, 64501]),
            best,
            route("rs2", &[64502, 64501]),
            longer,
        ];
        let paths = result.paths();
        let asns: Vec<&[u32]> = paths.iter().map(|path| path.asns.as_slice()).collect();
        assert_eq!(
            asns,
            [
                [64502, 64501],
                [64503, 64501],
                [64503, 64501],
                [64509, 64501]
            ]
        );
        assert_eq!(paths[0].route_servers, ["rs1", "rs2"]);
        assert!(paths[0].is_best);
        assert_eq!(paths[1].network, None);
        // The same path for another prefix is a path of its own.
        assert_eq!(paths[2].network.as_deref(), Some("203.0.113.0/25"));
        assert!(paths[3].is_filtered);
        assert_eq!(result.accepted().len(), 5);
        assert_eq!(result.filtered().len(), 1);
    }

    #[test]
    fn times_are_read_as_utc() {
        assert_eq!(parse_time("2026-11-15T08:00:00"), Some(1_794_729_600));
        assert_eq!(parse_time("2026-11-15T08:00:00Z"), Some(1_794_729_600));
        assert_eq!(
            parse_time("2026-11-15T08:00:00.869209753Z"),
            Some(1_794_729_600)
        );
        assert_eq!(
            parse_time("2026-11-15T06:37:27.240000"),
            Some(1_794_724_647)
        );
        assert_eq!(parse_time("2026-11-15"), Some(1_794_700_800));
        assert_eq!(parse_time("1970-01-01T00:00:00"), Some(0));
        assert_eq!(parse_time("2000-02-29T12:00:00"), Some(951_825_600));
        assert_eq!(parse_time("soon"), None);
        assert_eq!(parse_time(""), None);
    }

    #[test]
    fn routers_are_asked_three_at_a_time_and_answer_in_their_own_order() {
        let routers = ["r1", "r2", "r3", "r4", "r5", "r6", "r7"];
        let running = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let result = fan_out(
            &glass(GlassKind::LgCgi, &routers),
            GlassKind::LgCgi,
            |router| {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                most.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(15));
                running.fetch_sub(1, Ordering::SeqCst);
                // One request fails, one router holds no route.
                match router {
                    "r2" => None,
                    "r5" => Some(Vec::new()),
                    _ => Some(vec![route(router, &[64501])]),
                }
            },
        );
        assert!(most.load(Ordering::SeqCst) <= 3);
        let servers: Vec<&str> = result
            .routes
            .iter()
            .map(|route| route.route_server.as_str())
            .collect();
        assert_eq!(servers, ["r1", "r3", "r4", "r6", "r7"]);
        assert!(!result.failed);
        assert!(result.observed_at.is_some());
        assert_eq!(result.kind, GlassKind::LgCgi);
    }

    #[test]
    fn a_looking_glass_whose_routers_all_fail_gave_no_answer() {
        let result = fan_out(
            &glass(GlassKind::BirdLg, &["r1", "r2"]),
            GlassKind::BirdLg,
            |_| None,
        );
        assert!(result.failed);
        assert_eq!(result.observed_at, None);
        assert_eq!(result.summary(), "No answer");
        // No routers picked is nothing to ask, not a failure.
        let none = fan_out(&glass(GlassKind::BirdLg, &[]), GlassKind::BirdLg, |_| None);
        assert!(!none.failed);
        assert_eq!(none.summary(), "No route");
    }

    #[test]
    fn a_test_without_a_router_is_invalid_before_anything_is_asked() {
        let result = test(
            &glass(GlassKind::Hyperglass, &["r1"]),
            None,
            "203.0.113.0/24",
        );
        assert!(!result.passed);
        assert_eq!(result.headline, "Invalid \u{00B7} no router picked");
        assert!(result.steps.is_empty());
        assert_eq!(result.raw, None);
    }
}
