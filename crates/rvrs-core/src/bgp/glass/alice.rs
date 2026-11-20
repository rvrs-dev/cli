//! Alice-LG's API: `/api/v1/routeservers` lists the route servers, `/api/v1/lookup/prefix?q=`
//! finds routes on all of them. The lookup matches the start of the network, so asking for a
//! whole prefix returns that prefix and nothing else.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::http::{self, Request};
use super::scan::trim_blanks;
use super::{GlassKind, GlassResult, GlassRoute, GlassSessions, GlassSetup, LookingGlass};

/// Asks the looking glass for its route servers; fails when it isn't an Alice-LG. The name is
/// the exchange's when the route servers share one group, as "DE-CIX Frankfurt".
pub(super) fn describe(agent: &ureq::Agent, host: &str) -> Result<GlassSetup, String> {
    let list = get(agent, host, "/api/v1/routeservers", &[])
        .and_then(|data| serde_json::from_slice::<RouteServers>(&data).ok())
        .filter(|list| !list.routeservers.is_empty())
        .ok_or_else(|| format!("{host} didn't answer as an Alice-LG looking glass."))?;
    let groups: BTreeSet<&str> = list
        .routeservers
        .iter()
        .filter_map(|server| server.group.as_deref())
        .filter(|group| !group.is_empty())
        .collect();
    let name = (groups.len() == 1)
        .then(|| groups.first().map(|group| group.to_string()))
        .flatten();
    Ok(GlassSetup {
        kind: GlassKind::AliceLg,
        routers: Vec::new(),
        name,
        asn: None,
        protocols: Vec::new(),
        form_path: None,
        splits_prefix: false,
        api_version: None,
        query_types: BTreeMap::new(),
        vrfs: BTreeMap::new(),
    })
}

/// Every route the looking glass holds for exactly this prefix, accepted and filtered.
pub(super) fn routes(agent: &ureq::Agent, prefix: &str, glass: &LookingGlass) -> GlassResult {
    let result = GlassResult::new(glass, GlassKind::AliceLg);
    let lookup = get(
        agent,
        &glass.host,
        "/api/v1/lookup/prefix",
        &[("q", prefix)],
    )
    .and_then(|data| serde_json::from_slice::<Lookup>(&data).ok());
    match lookup {
        Some(lookup) => read(&lookup, prefix, result),
        None => GlassResult {
            failed: true,
            ..result
        },
    }
}

/// The sessions an AS has with the looking glass's route servers; nothing when it doesn't say.
pub(super) fn sessions(
    agent: &ureq::Agent,
    asn: u32,
    glass: &LookingGlass,
) -> Option<GlassSessions> {
    let number = asn.to_string();
    let data = get(
        agent,
        &glass.host,
        "/api/v1/lookup/neighbors",
        &[("asn", &number)],
    )?;
    let list = serde_json::from_slice::<Neighbors>(&data).ok()?;
    Some(read_sessions(&list, asn))
}

fn read_sessions(list: &Neighbors, asn: u32) -> GlassSessions {
    let neighbors: Vec<&Neighbor> = list
        .neighbors
        .iter()
        .flatten()
        .filter(|neighbor| neighbor.asn.is_none() || neighbor.asn == Some(i64::from(asn)))
        .collect();
    // BIRD says "up", OpenBGPD and GoBGP say "established".
    let up = neighbors
        .iter()
        .filter(|neighbor| {
            let state = neighbor.state.as_deref().unwrap_or("").to_lowercase();
            state == "up" || state == "established"
        })
        .count();
    GlassSessions {
        asn,
        total: neighbors.len(),
        up,
        received: neighbors
            .iter()
            .map(|neighbor| u64::try_from(neighbor.routes_received.unwrap_or(0)).unwrap_or(0))
            .sum(),
    }
}

fn read(lookup: &Lookup, prefix: &str, mut result: GlassResult) -> GlassResult {
    let routes = |page: &Option<Page>, filtered: bool| -> Vec<GlassRoute> {
        page.iter()
            .flat_map(|page| page.routes.iter().flatten())
            .filter(|route| route.network == prefix)
            .map(|route| {
                let attributes = route.bgp.as_ref();
                let mut asns: Vec<u32> = Vec::new();
                for &asn in attributes
                    .and_then(|bgp| bgp.as_path.as_ref())
                    .into_iter()
                    .flatten()
                {
                    if asns.last() != Some(&asn) {
                        asns.push(asn);
                    }
                }
                let server = route.routeserver.as_ref();
                let neighbor = route.neighbor.as_ref();
                GlassRoute {
                    route_server: server
                        .and_then(|server| server.name.clone().or(server.id.clone()))
                        .unwrap_or_else(|| "Route server".to_string()),
                    neighbor: neighbor.and_then(|neighbor| neighbor.asn),
                    neighbor_name: neighbor
                        .and_then(|neighbor| neighbor.description.as_deref())
                        .map(clean_neighbor),
                    asns,
                    is_best: route.primary.unwrap_or(false),
                    is_filtered: filtered,
                    communities: attributes
                        .and_then(|bgp| bgp.communities.as_ref())
                        .into_iter()
                        .flatten()
                        .take(16)
                        .map(|community| {
                            community
                                .iter()
                                .map(i64::to_string)
                                .collect::<Vec<_>>()
                                .join(":")
                        })
                        .collect(),
                    rpki: None,
                    network: None,
                }
            })
            .collect()
    };
    result.routes = routes(&lookup.imported, false);
    result.routes.extend(routes(&lookup.filtered, true));
    result.observed_at = lookup
        .api
        .as_ref()
        .and_then(|api| api.cache_status.as_ref())
        .and_then(|cache| cache.cached_at.as_deref())
        .and_then(super::parse_time);
    result
}

/// "AS64496 - Example Peering" names the neighbor twice; the AS number is shown already.
fn clean_neighbor(description: &str) -> String {
    let Some((_, tail)) = description.split_once(" - ") else {
        return description.to_string();
    };
    let tail = trim_blanks(tail);
    if !description.to_lowercase().starts_with("as") || tail.is_empty() {
        return description.to_string();
    }
    tail.to_string()
}

fn get(agent: &ureq::Agent, host: &str, path: &str, query: &[(&str, &str)]) -> Option<Vec<u8>> {
    let response = http::fetch(agent, host, path, query, Request::GetJson);
    if response.status != Some(200) {
        return None;
    }
    response.data
}

#[derive(Deserialize)]
struct RouteServers {
    routeservers: Vec<Server>,
}

#[derive(Deserialize)]
struct Server {
    /// Every route server has an id; a list without one is not Alice-LG's.
    #[allow(dead_code)]
    id: String,
    group: Option<String>,
}

#[derive(Deserialize)]
struct Neighbors {
    neighbors: Option<Vec<Neighbor>>,
}

#[derive(Deserialize)]
struct Neighbor {
    asn: Option<i64>,
    state: Option<String>,
    routes_received: Option<i64>,
}

#[derive(Deserialize)]
struct Lookup {
    imported: Option<Page>,
    filtered: Option<Page>,
    api: Option<Api>,
}

#[derive(Deserialize)]
struct Page {
    routes: Option<Vec<Route>>,
}

#[derive(Deserialize)]
struct Route {
    network: String,
    bgp: Option<Attributes>,
    primary: Option<bool>,
    neighbor: Option<RouteNeighbor>,
    routeserver: Option<RouteServer>,
}

#[derive(Deserialize)]
struct Attributes {
    as_path: Option<Vec<u32>>,
    communities: Option<Vec<Vec<i64>>>,
}

#[derive(Deserialize)]
struct RouteNeighbor {
    asn: Option<u32>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct RouteServer {
    id: Option<String>,
    name: Option<String>,
}

#[derive(Deserialize)]
struct Api {
    cache_status: Option<Cache>,
}

#[derive(Deserialize)]
struct Cache {
    cached_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> GlassResult {
        GlassResult {
            host: "lg.example.net".to_string(),
            name: "Example IX".to_string(),
            kind: GlassKind::AliceLg,
            routes: Vec::new(),
            sessions: None,
            observed_at: None,
            failed: false,
        }
    }

    #[test]
    fn accepted_and_filtered_routes_are_read_for_the_exact_prefix() {
        // The lookup matches the start of the network, so a longer prefix can ride along.
        let json = r#"
        {"api":{"cache_status":{"cached_at":"2026-11-15T19:41:59.637264838Z"}},
         "imported":{"routes":[
           {"network":"198.51.100.0/23","primary":true,"bgp":{"as_path":[64496,64497,64497,64498],"communities":[[0,64510],[64511,1000]]},
            "neighbor":{"asn":64496,"description":"AS64496 - Open Peering"},"routeserver":{"id":"rs1","name":"rs1.fra (IPv4)"}},
           {"network":"198.51.100.0/23","primary":true,"bgp":{"as_path":[64496,64497,64498]},
            "neighbor":{"asn":64496,"description":"AS64496 - Open Peering"},"routeserver":{"id":"rs2","name":"rs2.fra (IPv4)"}},
           {"network":"198.51.100.0/24","primary":false,"bgp":{"as_path":[64500]},"neighbor":null,"routeserver":{"id":"rs1","name":"rs1.fra (IPv4)"}}]},
         "filtered":{"routes":[
           {"network":"198.51.100.0/23","bgp":{"as_path":[64501,64498]},"neighbor":{"asn":64501,"description":"Example"},"routeserver":{"id":"rs1","name":"rs1.fra (IPv4)"}}]}}
        "#;
        let lookup: Lookup = serde_json::from_str(json).expect("the lookup decodes");
        let result = read(&lookup, "198.51.100.0/23", empty());
        assert_eq!(result.routes.len(), 3);
        assert_eq!(result.accepted().len(), 2);
        assert_eq!(result.filtered().len(), 1);
        assert_eq!(result.route_server_count(), 2);
        assert_eq!(result.summary(), "3 routes \u{00B7} 2 route servers");
        assert_eq!(result.observed_at, Some(1_794_771_719));
        // Prepending collapses, so both route servers hold the same path.
        let paths = result.paths();
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].asns, [64496, 64497, 64498]);
        assert_eq!(paths[0].route_servers.len(), 2);
        assert!(paths[0].is_best);
        assert_eq!(paths[0].neighbor_name.as_deref(), Some("Open Peering"));
        assert!(paths[1].is_filtered);
        assert_eq!(result.routes[0].communities, ["0:64510", "64511:1000"]);
        assert_eq!(result.routes[0].neighbor, Some(64496));
        assert_eq!(result.routes[0].route_server, "rs1.fra (IPv4)");
    }

    #[test]
    fn the_origins_sessions_with_the_route_servers_are_counted() {
        let json = r#"
        {"neighbors":[
          {"asn":64496,"state":"up","routes_received":0,"routeserver_id":"rs1"},
          {"asn":64496,"state":"down","routes_received":0,"routeserver_id":"rs2"},
          {"asn":64496,"state":"Established","routes_received":12,"routeserver_id":"rs3"},
          {"asn":64497,"state":"up","routes_received":40,"routeserver_id":"rs3"}]}
        "#;
        let list: Neighbors = serde_json::from_str(json).expect("the neighbors decode");
        let sessions = read_sessions(&list, 64496);
        assert_eq!(
            sessions,
            GlassSessions {
                asn: 64496,
                total: 3,
                up: 2,
                received: 12
            }
        );
        let none: Neighbors = serde_json::from_str(r#"{"neighbors":[]}"#).expect("it decodes");
        let none = read_sessions(&none, 64496);
        assert_eq!(none.total, 0);

        let mut glass = empty();
        glass.sessions = Some(none);
        assert_eq!(glass.summary(), "No session");
        glass.sessions = Some(sessions);
        assert_eq!(glass.summary(), "No route");
    }

    #[test]
    fn a_neighbor_is_named_once() {
        assert_eq!(clean_neighbor("AS64496 - Open Peering"), "Open Peering");
        assert_eq!(clean_neighbor("Example"), "Example");
        assert_eq!(clean_neighbor("Open - Peering"), "Open - Peering");
        assert_eq!(clean_neighbor("AS64496 - "), "AS64496 - ");
    }

    #[test]
    fn a_route_server_list_needs_ids() {
        let list = r#"{"routeservers":[{"id":"rs1","name":"rs1","group":"Example IX"}]}"#;
        assert!(serde_json::from_str::<RouteServers>(list).is_ok());
        assert!(
            serde_json::from_str::<RouteServers>(r#"{"routeservers":[{"name":"x"}]}"#).is_err()
        );
    }
}
