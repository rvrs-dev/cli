//! hyperglass (thatmattlove/hyperglass), the looking glass many operators run today. It has a
//! JSON API: `/api/info`, `/api/devices`, then a query per device. Version 1 takes
//! `query_type: "bgp_route"` with a VRF; version 2 takes the id of a directive, which is
//! `__hyperglass_<platform>_bgp_route__` for the built-in ones. The answer is router text
//! (read by `router_text`) or, where the operator turned it on, structured routes.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use super::http::{self, Request};
use super::router_text;
use super::{GlassKind, GlassResult, GlassRoute, GlassSetup, LookingGlass};

/// The platforms whose built-in directives version 2 is asked for, most common first.
const PLATFORMS: [&str; 12] = [
    "juniper",
    "cisco_ios",
    "cisco_xr",
    "arista_eos",
    "frr",
    "bird",
    "huawei",
    "nokia_sros",
    "mikrotik",
    "openbgpd",
    "vyos",
    "cisco_nxos",
];

/// The prefix a device is asked for to find out which directive it takes.
const PROBE_PREFIX: &str = "1.1.1.0/24";

pub(super) fn describe(agent: &ureq::Agent, host: &str) -> Result<GlassSetup, String> {
    let not_hyperglass = || format!("{host} didn't answer as a hyperglass looking glass.");
    let info = get(agent, host, "/api/info")
        .and_then(|data| serde_json::from_slice::<Map<String, Value>>(&data).ok())
        .ok_or_else(not_hyperglass)?;
    let devices = get(agent, host, "/api/devices")
        .and_then(|data| serde_json::from_slice::<Vec<Map<String, Value>>>(&data).ok())
        .filter(|devices| !devices.is_empty())
        .ok_or_else(not_hyperglass)?;
    let mut setup = read_devices(&info, &devices);
    if setup.api_version == Some(2) {
        // Find each device's built-in BGP route directive; devices of one operator usually
        // share a platform.
        let mut known: Vec<String> = Vec::new();
        for device in &setup.routers {
            if let Some(directive) = find_directive(agent, host, device, &known) {
                setup.query_types.insert(device.clone(), directive.clone());
                if !known.contains(&directive) {
                    known.insert(0, directive);
                }
            }
        }
        let query_types = &setup.query_types;
        setup
            .routers
            .retain(|device| query_types.contains_key(device));
        if setup.routers.is_empty() {
            return Err(format!(
                "None of the devices at {host} offers a BGP route query."
            ));
        }
    }
    Ok(setup)
}

/// What `/api/info` and `/api/devices` say: the version, the operator, and the devices. A
/// version 1 device is asked by its name, a version 2 device by its id.
fn read_devices(info: &Map<String, Value>, devices: &[Map<String, Value>]) -> GlassSetup {
    let version = match info.get("version").and_then(Value::as_str) {
        Some(version) if version.contains("hyperglass 1") || version.starts_with("1.") => 1,
        _ => 2,
    };
    let text = |key: &str| info.get(key).and_then(Value::as_str).map(str::to_string);
    let mut setup = GlassSetup {
        kind: GlassKind::Hyperglass,
        routers: Vec::new(),
        name: text("organization").or_else(|| text("name")),
        asn: info
            .get("primary_asn")
            .and_then(integer)
            .and_then(|asn| u32::try_from(asn).ok())
            .filter(|&asn| router_text::is_public(asn)),
        protocols: Vec::new(),
        form_path: None,
        splits_prefix: false,
        api_version: Some(version),
        query_types: BTreeMap::new(),
        vrfs: BTreeMap::new(),
    };
    for device in devices.iter().take(40) {
        if version == 1 {
            let Some(name) = device.get("name").and_then(Value::as_str) else {
                continue;
            };
            setup.routers.push(name.to_string());
            setup
                .query_types
                .insert(name.to_string(), "bgp_route".to_string());
            let vrf = device
                .get("vrfs")
                .and_then(Value::as_array)
                .filter(|vrfs| vrfs.iter().all(Value::is_object))
                .and_then(|vrfs| vrfs.first())
                .and_then(|vrf| vrf.get("name"))
                .and_then(Value::as_str);
            if let Some(vrf) = vrf {
                setup.vrfs.insert(name.to_string(), vrf.to_string());
            }
        } else if let Some(id) = device.get("id").and_then(Value::as_str) {
            setup.routers.push(id.to_string());
        }
    }
    setup
}

fn find_directive(
    agent: &ureq::Agent,
    host: &str,
    device: &str,
    try_first: &[String],
) -> Option<String> {
    let built_in = PLATFORMS
        .iter()
        .map(|platform| format!("__hyperglass_{platform}_bgp_route__"))
        .filter(|directive| !try_first.contains(directive));
    try_first.iter().cloned().chain(built_in).find(|candidate| {
        let probe = query(agent, host, 2, device, candidate, None, PROBE_PREFIX);
        probe.is_success()
            || probe
                .message
                .is_some_and(|message| !message.contains("not found"))
    })
}

/// What a device answered a query with.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Answer {
    pub status: Option<u16>,
    /// The router's own text.
    pub output: Option<String>,
    /// Structured routes, where the operator turned them on.
    pub structured: Option<Vec<Value>>,
    /// Why the query was refused, in lowercase.
    pub message: Option<String>,
    pub url: String,
    pub milliseconds: u64,
}

impl Answer {
    /// Version 2 answers a query with HTTP 201, so every 2xx is an answer.
    pub(super) fn is_success(&self) -> bool {
        self.status
            .is_some_and(|status| (200..300).contains(&status))
    }

    /// Fills in what the body says: text or structured routes from an answer, the message from
    /// a refusal.
    fn read_body(mut self, data: Option<&[u8]>) -> Self {
        let Some(object) =
            data.and_then(|data| serde_json::from_slice::<Map<String, Value>>(data).ok())
        else {
            return self;
        };
        let output = object.get("output");
        if self.is_success() {
            match output {
                Some(Value::String(text)) => self.output = Some(text.clone()),
                Some(Value::Object(output)) => {
                    self.structured = output
                        .get("routes")
                        .and_then(Value::as_array)
                        .filter(|routes| routes.iter().all(Value::is_object))
                        .cloned();
                }
                _ => {}
            }
        } else {
            self.message = output.and_then(Value::as_str).map(str::to_lowercase);
        }
        self
    }
}

/// The body of a query, in the words each version takes.
fn body(version: u8, device: &str, query_type: &str, vrf: Option<&str>, target: &str) -> Value {
    if version == 1 {
        json!({
            "query_location": device,
            "query_type": query_type,
            "query_vrf": vrf.unwrap_or("default"),
            "query_target": target,
        })
    } else {
        json!({
            "queryLocation": device,
            "queryType": query_type,
            "queryTarget": [target],
        })
    }
}

pub(super) fn query(
    agent: &ureq::Agent,
    host: &str,
    version: u8,
    device: &str,
    query_type: &str,
    vrf: Option<&str>,
    target: &str,
) -> Answer {
    let path = if version == 1 {
        "/api/query/"
    } else {
        "/api/query"
    };
    let body = body(version, device, query_type, vrf, target).to_string();
    let request = Request::Post {
        body: &body,
        content_type: "application/json",
    };
    let response = http::fetch(agent, host, path, &[], request);
    Answer {
        status: response.status,
        url: response.url.clone(),
        milliseconds: response.milliseconds,
        ..Answer::default()
    }
    .read_body(response.data.as_deref())
}

pub(super) fn routes(agent: &ureq::Agent, prefix: &str, glass: &LookingGlass) -> GlassResult {
    super::fan_out(glass, GlassKind::Hyperglass, |device| {
        let query_type = glass.query_types.get(device)?;
        let answer = query(
            agent,
            &glass.host,
            glass.api_version.unwrap_or(2),
            device,
            query_type,
            glass.vrfs.get(device).map(String::as_str),
            prefix,
        );
        answer
            .is_success()
            .then(|| read(&answer, prefix, device, glass.asn))
    })
}

pub(super) fn read(
    answer: &Answer,
    prefix: &str,
    router: &str,
    local_asn: Option<u32>,
) -> Vec<GlassRoute> {
    match &answer.output {
        Some(text) => router_text::routes(text, prefix, router, local_asn),
        None => structured(
            answer.structured.as_deref().unwrap_or(&[]),
            prefix,
            router,
            local_asn,
        ),
    }
}

/// hyperglass's structured routes: `as_path`, `active` (or `winning`), `rpki_state`
/// (0 invalid, 1 valid, 2 not found, 3 not checked).
pub(super) fn structured(
    routes: &[Value],
    prefix: &str,
    router: &str,
    local_asn: Option<u32>,
) -> Vec<GlassRoute> {
    routes
        .iter()
        .filter(|route| {
            route
                .get("prefix")
                .and_then(Value::as_str)
                .is_none_or(|network| network == prefix)
        })
        .map(|route| {
            let raw: Vec<u32> = route
                .get("as_path")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|asn| match asn {
                    Value::String(text) => text.parse().ok(),
                    other => integer(other).and_then(|asn| u32::try_from(asn).ok()),
                })
                .collect();
            let mut asns: Vec<u32> = local_asn.into_iter().collect();
            for &asn in &raw {
                if asns.last() != Some(&asn) {
                    asns.push(asn);
                }
            }
            let rpki = match route.get("rpki_state").and_then(integer) {
                Some(0) => Some("invalid"),
                Some(1) => Some("valid"),
                Some(2) => Some("unknown"),
                _ => None,
            };
            let flag = |key: &str| route.get(key).and_then(Value::as_bool);
            GlassRoute {
                route_server: router.to_string(),
                neighbor: raw.first().copied(),
                neighbor_name: None,
                asns,
                is_best: flag("active").or_else(|| flag("winning")).unwrap_or(false),
                is_filtered: false,
                communities: route
                    .get("communities")
                    .and_then(Value::as_array)
                    .and_then(|list| {
                        list.iter()
                            .map(|community| community.as_str().map(str::to_string))
                            .collect::<Option<Vec<_>>>()
                    })
                    .unwrap_or_default(),
                rpki: rpki.map(str::to_string),
                network: None,
            }
        })
        .collect()
}

/// A JSON number as a whole number, also when it is written as 64500.0.
fn integer(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|number| number.fract() == 0.0 && number.abs() < 9.0e15)
            .map(|number| number as i64)
    })
}

fn get(agent: &ureq::Agent, host: &str, path: &str) -> Option<Vec<u8>> {
    let response = http::fetch(agent, host, path, &[], Request::Get);
    if response.status != Some(200) {
        return None;
    }
    response.data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: u16, body: &str) -> Answer {
        Answer {
            status: Some(status),
            ..Answer::default()
        }
        .read_body(Some(body.as_bytes()))
    }

    fn object(text: &str) -> Map<String, Value> {
        serde_json::from_str(text).expect("the fixture is a JSON object")
    }

    fn objects(text: &str) -> Vec<Map<String, Value>> {
        serde_json::from_str(text).expect("the fixture is a list of JSON objects")
    }

    #[test]
    fn structured_routes_are_read_for_the_prefix() {
        let routes = json!([
            {"prefix": "203.0.113.0/24", "as_path": [64502, 64501], "active": true, "rpki_state": 1,
             "communities": ["64500:100", "64500:200"]},
            {"prefix": "203.0.113.0/24", "as_path": ["64503", 64503, 64501], "winning": false, "rpki_state": 2},
            {"prefix": "198.51.100.0/24", "as_path": [64504], "active": true},
        ]);
        let routes = structured(
            routes.as_array().expect("a list"),
            "203.0.113.0/24",
            "r1",
            Some(64500),
        );
        let paths: Vec<&[u32]> = routes.iter().map(|route| route.asns.as_slice()).collect();
        assert_eq!(paths, [[64500, 64502, 64501], [64500, 64503, 64501]]);
        let rpki: Vec<_> = routes.iter().map(|route| route.rpki.as_deref()).collect();
        assert_eq!(rpki, [Some("valid"), Some("unknown")]);
        assert!(routes[0].is_best);
        assert!(!routes[1].is_best);
        assert_eq!(routes[0].communities, ["64500:100", "64500:200"]);
        assert_eq!(routes[1].neighbor, Some(64503));
    }

    #[test]
    fn version_two_answers_with_http_201() {
        let text = answer(
            201,
            r#"{"output":"BGP.as_path: 64501","id":"x","cached":false}"#,
        );
        assert!(text.is_success());
        assert_eq!(text.output.as_deref(), Some("BGP.as_path: 64501"));
        assert_eq!(text.message, None);

        let routes = answer(
            200,
            r#"{"output":{"vrf":"default","count":1,"routes":[{"prefix":"203.0.113.0/24","as_path":[64501],"active":true}]}}"#,
        );
        assert_eq!(routes.output, None);
        let read = read(&routes, "203.0.113.0/24", "r1", None);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].asns, [64501]);
    }

    #[test]
    fn a_refusal_keeps_its_message_in_lowercase() {
        let refused = answer(
            404,
            r#"{"output":"Directive __hyperglass_frr_bgp_route__ Not Found","level":"danger"}"#,
        );
        assert!(!refused.is_success());
        assert_eq!(refused.output, None);
        assert_eq!(
            refused.message.as_deref(),
            Some("directive __hyperglass_frr_bgp_route__ not found")
        );
        assert_eq!(answer(502, "<html>Bad gateway</html>").message, None);
    }

    #[test]
    fn each_version_is_asked_in_its_own_words() {
        assert_eq!(
            body(1, "r1", "bgp_route", None, "203.0.113.0/24"),
            json!({"query_location": "r1", "query_type": "bgp_route", "query_vrf": "default", "query_target": "203.0.113.0/24"})
        );
        assert_eq!(
            body(1, "r1", "bgp_route", Some("global"), "203.0.113.0/24")["query_vrf"],
            "global"
        );
        assert_eq!(
            body(
                2,
                "r1",
                "__hyperglass_bird_bgp_route__",
                None,
                "203.0.113.0/24"
            ),
            json!({"queryLocation": "r1", "queryType": "__hyperglass_bird_bgp_route__", "queryTarget": ["203.0.113.0/24"]})
        );
    }

    #[test]
    fn version_one_devices_are_asked_by_name_in_their_vrf() {
        let info = object(
            r#"{"name":"Example LG","organization":"Example Net","primary_asn":64495,"version":"hyperglass 1.0.4"}"#,
        );
        let devices = objects(
            r#"[{"name":"fra1","network":"Example","display_name":"Frankfurt","vrfs":[{"id":"default","name":"global"}]},
                {"name":"ams1","vrfs":[]},
                {"display_name":"No name"}]"#,
        );
        let setup = read_devices(&info, &devices);
        assert_eq!(setup.api_version, Some(1));
        assert_eq!(setup.name.as_deref(), Some("Example Net"));
        assert_eq!(setup.asn, Some(64495));
        assert_eq!(setup.routers, ["fra1", "ams1"]);
        assert_eq!(setup.query_types["fra1"], "bgp_route");
        assert_eq!(setup.query_types["ams1"], "bgp_route");
        assert_eq!(
            setup.vrfs,
            BTreeMap::from([("fra1".to_string(), "global".to_string())])
        );
    }

    #[test]
    fn version_two_devices_are_asked_by_id() {
        let info = object(r#"{"name":"Example LG","primary_asn":"65001","version":"2.0.4"}"#);
        let devices = objects(r#"[{"id":"zrh1","name":"Zurich"},{"id":"gva1","name":"Geneva"}]"#);
        let setup = read_devices(&info, &devices);
        assert_eq!(setup.api_version, Some(2));
        assert_eq!(setup.name.as_deref(), Some("Example LG"));
        // A private AS number says nothing about the operator, and text is no number.
        assert_eq!(setup.asn, None);
        assert_eq!(setup.routers, ["zrh1", "gva1"]);
        assert!(setup.query_types.is_empty());
        // No version at all is the current one.
        assert_eq!(read_devices(&object("{}"), &devices).api_version, Some(2));
    }
}
