//! Reads the BGP output routers print, whatever looking glass carried it: Junos
//! `show route protocol bgp`, Cisco and FRR `show bgp`, BIRD `show route all`. Every
//! form-based looking glass (lg.cgi, bird-lg-go) and hyperglass's text answers go through here.

use super::scan::{find_ci, is_space, lines, skip_digits, skip_spaces, trim_blanks};
use super::GlassRoute;

/// The router software whose output a text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Format {
    Junos,
    Cisco,
    Bird,
}

impl Format {
    /// The name the test's "Router output" step reads it as.
    pub(super) fn title(self) -> &'static str {
        match self {
            Format::Junos => "Junos",
            Format::Cisco => "Cisco or FRR",
            Format::Bird => "BIRD",
        }
    }
}

pub(super) fn format(text: &str) -> Option<Format> {
    if text.contains("[BGP/") {
        return Some(Format::Junos);
    }
    if text.contains("BGP.as_path") || text.contains("Table master") || text.contains("unicast [") {
        return Some(Format::Bird);
    }
    if text.contains("BGP routing table entry for") || text.contains("Paths: (") {
        return Some(Format::Cisco);
    }
    None
}

/// The routes for `prefix`, or for the closest prefix the router printed when it has none for
/// exactly that one. `local_asn` is the operator's own AS, which routers leave out of their
/// paths.
pub(super) fn routes(
    raw: &str,
    prefix: &str,
    router: &str,
    local_asn: Option<u32>,
) -> Vec<GlassRoute> {
    let text = plain(raw);
    match format(&text) {
        Some(Format::Junos) => junos(&text, prefix, router, local_asn),
        Some(Format::Cisco) => cisco(&text, prefix, router, local_asn),
        Some(Format::Bird) => bird(&text, prefix, router, local_asn),
        None => Vec::new(),
    }
}

/// A route and the prefix the router printed it under.
type Found = (String, GlassRoute);

/// The first word of a line that starts at the margin, when it is a prefix.
fn leading_network(line: &str) -> Option<&str> {
    if line.chars().next().is_none_or(char::is_whitespace) {
        return None;
    }
    line.split(' ')
        .find(|word| !word.is_empty())
        .filter(|word| word.contains('/'))
}

// Junos

pub(super) fn junos(
    text: &str,
    prefix: &str,
    router: &str,
    local_asn: Option<u32>,
) -> Vec<GlassRoute> {
    let mut found: Vec<Found> = Vec::new();
    let mut network: Option<&str> = None;
    let mut pending: Option<(bool, &str)> = None;
    for line in lines(text) {
        if let Some(candidate) = leading_network(line) {
            network = Some(candidate);
        }
        if let Some(at) = line.find("[BGP/") {
            pending = Some((line[..at].contains('*'), network.unwrap_or(prefix)));
        }
        if let (Some(at), Some((active, current))) = (line.find("AS path:"), pending) {
            let rest = &line[at + "AS path:".len()..];
            let path_text = rest
                .split(',')
                .find(|part| !part.is_empty())
                .unwrap_or(rest);
            found.push((
                current.to_string(),
                route(
                    &numbers(path_text),
                    router,
                    local_asn,
                    active,
                    validation_state(rest),
                    current,
                    prefix,
                ),
            ));
            pending = None;
        }
    }
    closest(found, prefix)
}

/// Junos's own origin validation: the word after "validation-state:".
fn validation_state(text: &str) -> Option<String> {
    const LABEL: &str = "validation-state:";
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = find_ci(text, LABEL, from) {
        let start = skip_spaces(bytes, at + LABEL.len());
        let mut end = start;
        while bytes
            .get(end)
            .is_some_and(|&byte| byte.is_ascii_alphabetic() || byte == b'-')
        {
            end += 1;
        }
        if end > start {
            return Some(text[start..end].to_string());
        }
        from = at + 1;
    }
    None
}

// Cisco and FRR

struct CiscoPath {
    path: Vec<u32>,
    best: bool,
    rpki: Option<String>,
}

pub(super) fn cisco(
    text: &str,
    prefix: &str,
    router: &str,
    local_asn: Option<u32>,
) -> Vec<GlassRoute> {
    let mut found: Vec<Found> = Vec::new();
    let mut network = prefix.to_string();
    let mut current: Option<CiscoPath> = None;
    let mut in_paths = false;
    let flush = |current: &mut Option<CiscoPath>, network: &str, found: &mut Vec<Found>| {
        if let Some(entry) = current.take() {
            found.push((
                network.to_string(),
                route(
                    &entry.path,
                    router,
                    local_asn,
                    entry.best,
                    entry.rpki,
                    network,
                    prefix,
                ),
            ));
        }
    };
    for line in lines(text) {
        let trimmed = trim_blanks(line);
        if let Some(entry) = table_entry(line) {
            flush(&mut current, &network, &mut found);
            network = entry.to_string();
            in_paths = false;
            continue;
        }
        if trimmed.starts_with("Paths:") {
            in_paths = true;
            continue;
        }
        if !in_paths {
            continue;
        }
        // A path starts on a line indented by two that holds the AS path, or "Local".
        let indent = line.bytes().take_while(|&byte| byte == b' ').count();
        let head = trimmed.split(',').find(|part| !part.is_empty());
        if let Some(head) = head.filter(|head| indent == 2 && is_path_head(head)) {
            flush(&mut current, &network, &mut found);
            current = Some(CiscoPath {
                path: if head == "Local" {
                    Vec::new()
                } else {
                    numbers(head)
                },
                best: false,
                rpki: None,
            });
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        let lower = trimmed.to_lowercase();
        if lower.starts_with("origin ") && lower.contains("best") {
            entry.best = true;
        }
        if let Some(state) = cisco_validation(&lower) {
            entry.rpki = Some(state.to_string());
        }
    }
    flush(&mut current, &network, &mut found);
    closest(found, prefix)
}

/// Whether the start of a line is an AS path: "Local", or numbers with AS sets among them.
fn is_path_head(head: &str) -> bool {
    head == "Local"
        || head.split(' ').filter(|word| !word.is_empty()).all(|word| {
            word.parse::<i64>().is_ok() || word.starts_with('{') || word.starts_with('(')
        })
}

/// The prefix in "BGP routing table entry for 203.0.113.0/24".
fn table_entry(line: &str) -> Option<&str> {
    const LABEL: &str = "BGP routing table entry for ";
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(at) = find_ci(line, LABEL, from) {
        let start = at + LABEL.len();
        let mut slash = start;
        while bytes
            .get(slash)
            .is_some_and(|&byte| byte.is_ascii_hexdigit() || byte == b':' || byte == b'.')
        {
            slash += 1;
        }
        if slash > start && bytes.get(slash) == Some(&b'/') {
            let end = skip_digits(bytes, slash + 1);
            if end > slash + 1 {
                return Some(&line[start..end]);
            }
        }
        from = at + 1;
    }
    None
}

/// The router's origin validation on a lowercased line, in the words rvrs uses: "not found"
/// in either spelling is "unknown".
fn cisco_validation(lower: &str) -> Option<&'static str> {
    const LABELS: [&str; 3] = ["rpki validation-state", "validation state", "path is"];
    const STATES: [(&str, &str); 5] = [
        ("valid", "valid"),
        ("invalid", "invalid"),
        ("not found", "unknown"),
        ("unknown", "unknown"),
        ("notfound", "unknown"),
    ];
    let bytes = lower.as_bytes();
    for at in 0..bytes.len() {
        for label in LABELS {
            if !bytes[at..].starts_with(label.as_bytes()) {
                continue;
            }
            let mut cursor = at + label.len();
            if bytes.get(cursor) == Some(&b':') {
                cursor += 1;
            }
            cursor = skip_spaces(bytes, cursor);
            let state = STATES
                .iter()
                .find(|(word, _)| bytes[cursor..].starts_with(word.as_bytes()));
            if let Some((_, state)) = state {
                return Some(state);
            }
        }
    }
    None
}

// BIRD

struct BirdRoute {
    path: Option<Vec<u32>>,
    header_origin: Option<u32>,
    best: bool,
}

pub(super) fn bird(
    text: &str,
    prefix: &str,
    router: &str,
    local_asn: Option<u32>,
) -> Vec<GlassRoute> {
    let mut found: Vec<Found> = Vec::new();
    let mut network = prefix.to_string();
    let mut current: Option<BirdRoute> = None;
    let flush = |current: &mut Option<BirdRoute>, network: &str, found: &mut Vec<Found>| {
        if let Some(entry) = current.take() {
            let path = entry
                .path
                .or(entry.header_origin.map(|origin| vec![origin]))
                .unwrap_or_default();
            found.push((
                network.to_string(),
                route(&path, router, local_asn, entry.best, None, network, prefix),
            ));
        }
    };
    for line in lines(text) {
        if line.contains("unicast [") || is_bird_one_header(line) {
            flush(&mut current, &network, &mut found);
            if let Some(candidate) = leading_network(line) {
                network = candidate.to_string();
            }
            // "] * (100/?) [AS64501i]": the star after the protocol marks the best route.
            let after_protocol = line.find(']').map_or("", |at| &line[at + 1..]);
            current = Some(BirdRoute {
                path: None,
                header_origin: header_origin(line),
                best: trim_blanks(after_protocol).starts_with('*'),
            });
            continue;
        }
        if let (Some(at), Some(entry)) = (line.find("BGP.as_path:"), current.as_mut()) {
            entry.path = Some(numbers(&line[at + "BGP.as_path:".len()..]));
        }
    }
    flush(&mut current, &network, &mut found);
    closest(found, prefix)
}

/// BIRD 1 has no "unicast": a route's first line is "via", "blackhole" or "unreachable" as a
/// word of its own, with the protocol in brackets after it.
fn is_bird_one_header(line: &str) -> bool {
    const WORDS: [&str; 3] = ["via", "blackhole", "unreachable"];
    let bytes = line.as_bytes();
    (0..bytes.len()).any(|at| {
        is_space(bytes[at])
            && WORDS.iter().any(|word| {
                let end = at + 1 + word.len();
                bytes[at + 1..].starts_with(word.as_bytes())
                    && !bytes
                        .get(end)
                        .is_some_and(|&byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    && bytes[end..].contains(&b'[')
            })
    })
}

/// The origin AS BIRD prints at the end of a route's first line, as in "[AS64501i]".
fn header_origin(line: &str) -> Option<u32> {
    let bytes = line.as_bytes();
    let mut from = 0;
    while let Some(at) = find_ci(line, "[as", from) {
        let start = at + 3;
        let mut end = skip_digits(bytes, start);
        if end > start {
            let digits = &line[start..end];
            if bytes
                .get(end)
                .is_some_and(|&byte| matches!(byte, b'i' | b'e' | b'I' | b'E' | b'?'))
            {
                end += 1;
            }
            if bytes.get(end) == Some(&b']') {
                return digits.parse().ok();
            }
        }
        from = at + 1;
    }
    None
}

// Shared

fn route(
    raw: &[u32],
    router: &str,
    local_asn: Option<u32>,
    active: bool,
    rpki: Option<String>,
    network: &str,
    prefix: &str,
) -> GlassRoute {
    let mut asns: Vec<u32> = local_asn.into_iter().collect();
    for &asn in raw {
        if asns.last() != Some(&asn) {
            asns.push(asn);
        }
    }
    GlassRoute {
        route_server: router.to_string(),
        neighbor: raw.first().copied(),
        neighbor_name: None,
        asns,
        is_best: active,
        is_filtered: false,
        communities: Vec::new(),
        rpki,
        network: (network != prefix).then(|| network.to_string()),
    }
}

/// The routes for exactly `prefix`, or the ones for the first prefix the router printed.
fn closest(found: Vec<Found>, prefix: &str) -> Vec<GlassRoute> {
    let wanted = if found.iter().any(|(network, _)| network == prefix) {
        prefix.to_string()
    } else {
        match found.first() {
            Some((network, _)) => network.clone(),
            None => return Vec::new(),
        }
    };
    found
        .into_iter()
        .filter(|(network, _)| *network == wanted)
        .map(|(_, route)| route)
        .collect()
}

/// AS numbers in a path, AS sets in braces left out.
pub(super) fn numbers(text: &str) -> Vec<u32> {
    let mut depth = 0u32;
    let mut result = Vec::new();
    for token in text.split([' ', '\t']).filter(|token| !token.is_empty()) {
        if token.starts_with('{') || token.starts_with('(') {
            depth += 1;
        }
        if depth == 0 {
            if let Ok(number) = token.parse() {
                result.push(number);
            }
        }
        if token.ends_with('}') || token.ends_with(')') {
            depth = depth.saturating_sub(1);
        }
    }
    result
}

/// The text without tags, with entities decoded and BIRD's tabs kept.
pub(super) fn plain(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        match rest[open + 1..].find('>') {
            // "<>" is no tag: both characters stay.
            Some(0) => {
                text.push_str(&rest[..open + 1]);
                rest = &rest[open + 1..];
            }
            Some(length) => {
                text.push_str(&rest[..open]);
                rest = &rest[open + length + 2..];
            }
            None => break,
        }
    }
    text.push_str(rest);
    decode_entities(&text)
}

/// The few HTML entities looking glass pages use, "&amp;" last so nothing is decoded twice.
pub(super) fn decode_entities(text: &str) -> String {
    text.replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// Private and documentation AS numbers say nothing about who runs a looking glass.
pub(super) fn is_public(asn: u32) -> bool {
    !(asn == 0 || (64496..=65551).contains(&asn) || asn >= 4_200_000_000 || asn == 23456)
}

/// Junos output as lg.cgi wraps it.
#[cfg(test)]
pub(super) const JUNOS_PAGE: &str = r##"<B>Router:</B> Copenhagen #1
<P><PRE><CODE>
<FONT COLOR="0">inet.0: 1130194 destinations, 4017320 routes (1129816 active, 2 holddown, 958772 hidden)</FONT>
<FONT COLOR="0">+ = Active Route, - = Last Active, * = Both</FONT>

<FONT COLOR="#FF0000"><B><A HREF="/?query=bgp"><B>203.0.113.0/24</B></A></B>         *[BGP/170] 1w3d 18:46:20, MED 0, localpref 100</FONT>
<FONT COLOR="#FF0000">                      AS path: 64501 I, validation-state: valid</FONT>
<FONT COLOR="#FF0000">                    &gt;  to 192.0.2.1 via ae29.0</FONT>
                    [BGP/170] 1w3d 18:46:29, MED 0, localpref 100
                      AS path: 64502 64502 64501 I, validation-state: valid
                    &gt;  to 192.0.2.2 via ae10.0
203.0.113.0/25     *[BGP/170] 1w3d 18:45:06, MED 0, localpref 200, from 192.0.2.9
                      AS path: I, validation-state: unverified
                    &gt;  to 192.0.2.3 via ae1.0
</CODE></PRE>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// FRR's `show bgp` as a hyperglass 1 answered it, addresses swapped for documentation ones.
    const FRR: &str = "\
BGP routing table entry for 203.0.113.0/24
Paths: (2 available, best #1, table default)
  Advertised to non peer-group peers:
  192.0.2.252
  64501, (aggregated by 64501 192.0.2.80)
    192.0.2.252 from 192.0.2.252 (198.51.100.1)
      Origin IGP, localpref 0, valid, external, atomic-aggregate, best (First path received)
      Community: 64500:22401 graceful-shutdown
  64502 64501
    192.0.2.253 from 192.0.2.253 (198.51.100.2)
      Origin IGP, localpref 0, valid, external
";

    /// BIRD 2 `show route for … all`, as hyperglass 2 and bird-lg-go print it.
    const BIRD: &str = "\
BIRD 2.14 ready.
Table master4:
203.0.113.0/24       unicast [peer_a 2026-10-23 07:21:26 from 192.0.2.230] * (100/?) [AS64501i]
\tvia 192.0.2.129 on ens3
\tType: BGP univ
\tBGP.origin: IGP
\tBGP.as_path: 64501
\tBGP.next_hop: 192.0.2.192
                     unicast [peer_b 2026-10-23 07:21:27 from 192.0.2.235] (100/?) [AS64501i]
\tvia 192.0.2.129 on ens3
\tType: BGP univ
\tBGP.as_path: 64503 64503 64501
";

    fn paths(routes: &[GlassRoute]) -> Vec<Vec<u32>> {
        routes.iter().map(|route| route.asns.clone()).collect()
    }

    fn best(routes: &[GlassRoute]) -> Vec<bool> {
        routes.iter().map(|route| route.is_best).collect()
    }

    #[test]
    fn each_format_is_recognized() {
        assert_eq!(format(FRR), Some(Format::Cisco));
        assert_eq!(format(BIRD), Some(Format::Bird));
        assert_eq!(format("  [BGP/170] 1w3d"), Some(Format::Junos));
        assert_eq!(format("% Network not in table"), None);
    }

    #[test]
    fn junos_routes_are_read_for_the_prefix() {
        let routes = routes(JUNOS_PAGE, "203.0.113.0/24", "Copenhagen #1", Some(64500));
        assert_eq!(routes.len(), 2);
        // The operator's own AS leads, and prepending collapses.
        assert_eq!(routes[0].asns, [64500, 64501]);
        assert!(routes[0].is_best);
        assert_eq!(routes[0].rpki.as_deref(), Some("valid"));
        assert_eq!(routes[0].network, None);
        assert_eq!(routes[0].neighbor, Some(64501));
        assert_eq!(routes[0].route_server, "Copenhagen #1");
        assert_eq!(routes[1].asns, [64500, 64502, 64501]);
        assert!(!routes[1].is_best);
    }

    #[test]
    fn a_route_the_operator_originates_is_only_its_own_as() {
        let own = routes(JUNOS_PAGE, "203.0.113.0/25", "Copenhagen #1", Some(64500));
        assert_eq!(paths(&own), [[64500]]);
        assert_eq!(own[0].rpki.as_deref(), Some("unverified"));
        assert_eq!(own[0].neighbor, None);
    }

    #[test]
    fn without_an_exact_match_the_closest_prefix_is_kept_and_labeled() {
        let closest = routes(JUNOS_PAGE, "203.0.112.0/23", "Copenhagen #1", Some(64500));
        assert_eq!(closest.len(), 2);
        assert_eq!(closest[0].network.as_deref(), Some("203.0.113.0/24"));
        assert!(junos(&plain("<PRE></PRE>"), "203.0.113.0/24", "r", None).is_empty());
    }

    #[test]
    fn cisco_and_frr_paths_are_read() {
        let routes = routes(FRR, "203.0.113.0/24", "r1", Some(64500));
        assert_eq!(
            paths(&routes),
            [vec![64500, 64501], vec![64500, 64502, 64501]]
        );
        assert_eq!(best(&routes), [true, false]);
        assert!(routes.iter().all(|route| route.network.is_none()));
    }

    #[test]
    fn cisco_origin_validation_is_read_in_rvrs_words() {
        let text = "\
BGP routing table entry for 2001:db8::/32, version 12
Paths: (2 available, best #2)
  64501 64502
    2001:db8::1 from 2001:db8::1 (192.0.2.1)
      Origin IGP, metric 0, localpref 100, valid, external
      path 7F0A1B2C RPKI State not found
      RPKI validation-state: not found
  Local
    2001:db8::2 from 2001:db8::2 (192.0.2.2)
      Origin IGP, localpref 100, valid, internal, best
      Origin validation state: valid
";
        let routes = routes(text, "2001:db8::/32", "r1", None);
        assert_eq!(paths(&routes), [vec![64501, 64502], vec![]]);
        assert_eq!(best(&routes), [false, true]);
        assert_eq!(routes[0].rpki.as_deref(), Some("unknown"));
        assert_eq!(routes[1].rpki.as_deref(), Some("valid"));
        assert_eq!(cisco_validation("path is invalid"), Some("invalid"));
        assert_eq!(cisco_validation("origin igp, valid, external"), None);
    }

    #[test]
    fn bird_routes_are_read() {
        let routes = routes(BIRD, "203.0.113.0/24", "r1", None);
        assert_eq!(paths(&routes), [vec![64501], vec![64503, 64501]]);
        assert_eq!(best(&routes), [true, false]);
        assert_eq!(routes[1].neighbor, Some(64503));
    }

    #[test]
    fn bird_one_routes_are_read_from_their_via_lines() {
        let text = "\
BIRD 1.6.8 ready.
203.0.113.0/24     via 192.0.2.1 on eth0 [peer_a 2026-10-23] * (100) [AS64501i]
\tType: BGP unicast univ
\tBGP.as_path: 64502 64501
                   via 192.0.2.2 on eth0 [peer_b 2026-10-23] (100) [AS64501?]
";
        let routes = routes(text, "203.0.113.0/24", "r1", Some(64500));
        // The second route prints no path, so the origin in its header stands in.
        assert_eq!(
            paths(&routes),
            [vec![64500, 64502, 64501], vec![64500, 64501]]
        );
        assert_eq!(best(&routes), [true, false]);
        assert!(!is_bird_one_header("\tvia 192.0.2.129 on ens3"));
        assert!(!is_bird_one_header(" viaduct [x]"));
    }

    #[test]
    fn as_sets_are_left_out_of_a_path() {
        assert_eq!(numbers("64500 {64510 64511} 64501"), [64500, 64501]);
        assert_eq!(numbers("64501 I"), [64501]);
        assert_eq!(numbers(" I"), Vec::<u32>::new());
    }

    #[test]
    fn private_and_documentation_as_numbers_are_not_public() {
        assert!(!is_public(65001));
        assert!(!is_public(64496));
        assert!(!is_public(23456));
        assert!(!is_public(0));
        assert!(!is_public(4_200_000_000));
        // The numbers on either side of the reserved block.
        assert!(is_public(64495));
        assert!(is_public(65552));
    }

    #[test]
    fn tags_go_and_entities_are_decoded() {
        assert_eq!(
            plain("<b>a</b> &gt; b &amp;amp; <> c\td"),
            "a > b &amp; <> c\td"
        );
        assert_eq!(plain("1 < 2"), "1 < 2");
        assert_eq!(decode_entities("Frankfurt &amp; Co"), "Frankfurt & Co");
    }
}
