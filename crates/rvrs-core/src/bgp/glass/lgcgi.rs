//! The classic `lg.cgi` looking glass (cougar's LG, the Perl script many ISPs run): a web form in
//! front of the operator's own routers. It has no API, so rvrs posts the form and reads the
//! router's own output through `router_text` (Junos, Cisco and FRR, BIRD).

use std::collections::BTreeMap;

use super::http::{self, Request, FORM_ALLOWED};
use super::router_text::{self, decode_entities};
use super::scan::{
    find_ci, is_space, named_at, quoted_attribute, skip_digits, starts_with_ci, tag_attributes,
    tags, trim_blanks,
};
use super::{GlassKind, GlassResult, GlassRoute, GlassSetup, LookingGlass};

/// What the form of an lg.cgi looking glass says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Description {
    pub routers: Vec<String>,
    /// The family names the form offers, as "IPv4" and "IPv6".
    pub protocols: Vec<String>,
    /// The operator's AS number, read from the page title ("Example/AS64500 Looking Glass").
    pub asn: Option<u32>,
    /// The title without "Looking Glass", for a default name: "Example/AS64500".
    pub name: Option<String>,
    /// Where the form posts: "/" or "/lg.cgi".
    pub path: String,
    /// The form takes the prefix length in its own field (Zayo's variant).
    pub splits_prefix: bool,
}

/// Where lg.cgi lives on the hosts seen so far.
const PATHS: [&str; 3] = ["/", "/lg.cgi", "/cgi-bin/lg.cgi"];

/// Reads the form: its routers, families and title. Fails when it has no BGP query or no
/// routers.
pub(super) fn describe(agent: &ureq::Agent, host: &str) -> Result<GlassSetup, String> {
    for path in PATHS {
        let Some(page) = fetch(agent, host, path, None) else {
            continue;
        };
        if let Some(description) = describe_page(&page) {
            return Ok(GlassSetup {
                kind: GlassKind::LgCgi,
                routers: description.routers,
                name: description.name,
                asn: description.asn,
                protocols: description.protocols,
                form_path: Some(action(&page, path)),
                splits_prefix: description.splits_prefix,
                api_version: None,
                query_types: BTreeMap::new(),
                vrfs: BTreeMap::new(),
            });
        }
    }
    Err(format!(
        "{host} didn't answer as an lg.cgi looking glass with a BGP query."
    ))
}

pub(super) fn describe_page(page: &str) -> Option<Description> {
    let lower = page.to_lowercase();
    if !lower.contains("name=\"query\"") || !lower.contains("value=\"bgp\"") {
        return None;
    }
    let routers = options("router", page);
    if routers.is_empty() {
        return None;
    }
    let title = title(page).map(decode_entities);
    let asn = title.as_deref().and_then(title_asn);
    let name = title
        .as_deref()
        .map(|title| trim_blanks(&remove_ci(title, "Looking Glass")).to_string())
        .filter(|name| !name.is_empty());
    let mut protocols = options("protocol", page);
    if protocols.is_empty() {
        // Some forms offer the family as radio buttons.
        protocols = radio_values(page, "protocol");
    }
    Some(Description {
        routers,
        protocols,
        asn,
        name,
        path: "/".to_string(),
        splits_prefix: has_input(page, "prefix"),
    })
}

/// The path the BGP form posts to, relative to the page it sits on.
pub(super) fn action(page: &str, path: &str) -> String {
    let actions = tag_attributes(page, "<form", "action");
    let Some(action) = actions
        .into_iter()
        .find(|action| action.to_lowercase().contains("lg") || *action == "/")
    else {
        return path.to_string();
    };
    if action.starts_with('/') {
        return action.to_string();
    }
    if let Some(scheme) = action.find("://") {
        let rest = &action[scheme + 3..];
        let path = rest.find('/').map_or("", |slash| &rest[slash..]);
        let path = path.split(['?', '#']).next().unwrap_or(path);
        return if path.is_empty() { "/" } else { path }.to_string();
    }
    let directory = match path.rfind('/') {
        Some(slash) => &path[..=slash],
        None => "/",
    };
    format!("{directory}{action}")
}

/// Asks every chosen router for the prefix, three at a time, and keeps its BGP routes.
pub(super) fn routes(agent: &ureq::Agent, prefix: &str, glass: &LookingGlass) -> GlassResult {
    super::fan_out(glass, GlassKind::LgCgi, |router| {
        let page = ask(agent, prefix, glass, router)?;
        Some(read(&page, prefix, glass, router))
    })
}

/// One router's page for the prefix, or nothing when the request failed.
pub(super) fn ask(
    agent: &ureq::Agent,
    prefix: &str,
    glass: &LookingGlass,
    router: &str,
) -> Option<String> {
    let body = encode(&form(prefix, glass, router));
    let path = glass.form_path.as_deref().unwrap_or("/");
    fetch(agent, &glass.host, path, Some(&body))
}

/// The routes on one router's page.
pub(super) fn read(
    page: &str,
    prefix: &str,
    glass: &LookingGlass,
    router: &str,
) -> Vec<GlassRoute> {
    router_text::routes(page, prefix, router, glass.asn)
}

/// The fields the form takes for a BGP query, the prefix length on its own where the form
/// wants it so.
pub(super) fn form(
    prefix: &str,
    glass: &LookingGlass,
    router: &str,
) -> BTreeMap<&'static str, String> {
    let family = if prefix.contains(':') { "IPv6" } else { "IPv4" };
    let protocol = glass
        .protocols
        .iter()
        .find(|protocol| protocol.to_lowercase().contains(&family.to_lowercase()))
        .map_or(family, String::as_str);
    let mut form = BTreeMap::from([
        ("query", "bgp".to_string()),
        ("protocol", protocol.to_string()),
        ("addr", prefix.to_string()),
        ("router", router.to_string()),
    ]);
    if let Some((address, length)) = prefix.split_once('/').filter(|_| glass.splits_prefix) {
        form.insert("addr", address.to_string());
        form.insert("prefix", length.to_string());
    }
    form
}

/// One router's answer as text, for the test.
pub(super) fn raw_output(page: &str) -> String {
    let text = router_text::plain(page);
    // The output starts where the router's name is printed; the form above it is noise.
    match text.find("Router:") {
        Some(at) => text[at..].trim().to_string(),
        None => text,
    }
}

// HTTP and HTML

fn fetch(agent: &ureq::Agent, host: &str, path: &str, form: Option<&str>) -> Option<String> {
    let request = match form {
        Some(body) => Request::Post {
            body,
            content_type: "application/x-www-form-urlencoded",
        },
        None => Request::Get,
    };
    let response = http::fetch(agent, host, path, &[], request);
    if response.status != Some(200) {
        return None;
    }
    response.text_lossy()
}

/// The form as a request body, its fields in the order of their names.
pub(super) fn encode(form: &BTreeMap<&str, String>) -> String {
    form.iter()
        .map(|(key, value)| format!("{key}={}", http::percent_encode(value, FORM_ALLOWED)))
        .collect::<Vec<_>>()
        .join("&")
}

/// The values of a `<select name="…">`'s options, in order. A radio button of the same name
/// is no select, so the search moves past it.
pub(super) fn options(select: &str, page: &str) -> Vec<String> {
    let name = format!("name=\"{select}\"");
    let mut from = 0;
    while let Some(at) = find_ci(page, &name, from) {
        from = at + name.len();
        let in_select = page[..at]
            .rfind('<')
            .is_some_and(|tag| starts_with_ci(page, tag, "<select"));
        if !in_select {
            continue;
        }
        let Some(end) = find_ci(page, "</select>", from) else {
            break;
        };
        return tag_attributes(&page[from..end], "<option", "value")
            .into_iter()
            .map(decode_entities)
            .collect();
    }
    Vec::new()
}

/// The values of the `<input name="…" value="…">` tags with this name, in order.
fn radio_values(page: &str, name: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut consumed = 0;
    for span in tags(page, "<input") {
        if span.start < consumed {
            continue;
        }
        let positions = || span.clone().chain(std::iter::once(span.end)).rev();
        let hit = positions().find_map(|at| {
            let named = named_at(page, at, name)?;
            positions()
                .take_while(|&value| value >= named)
                .find_map(|value| quoted_attribute(page, value, "value"))
        });
        if let Some((value, end)) = hit {
            values.push(page[value].to_string());
            consumed = end;
        }
    }
    values
}

/// Whether the page has an `<input>` with exactly this name.
fn has_input(page: &str, name: &str) -> bool {
    let bytes = page.as_bytes();
    let closes = |at: usize| {
        bytes
            .get(at)
            .is_some_and(|&byte| is_space(byte) || byte == b'>')
    };
    tags(page, "<input").any(|span| {
        span.clone()
            .filter_map(|at| named_at(page, at, name))
            .any(|end| closes(end) || (bytes.get(end) == Some(&b'"') && closes(end + 1)))
    })
}

/// The page's title, without the whitespace around it.
fn title(page: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(at) = find_ci(page, "<title>", from) {
        let start = at + "<title>".len();
        let end = page[start..].find('<').map(|offset| start + offset)?;
        if starts_with_ci(page, end, "</title>") {
            return Some(page[start..end].trim());
        }
        from = at + 1;
    }
    None
}

/// The AS number in a title, as in "Example/AS64500 Looking Glass" or "AS 64500".
fn title_asn(title: &str) -> Option<u32> {
    let bytes = title.as_bytes();
    let mut from = 0;
    while let Some(at) = find_ci(title, "as", from) {
        let mut start = at + 2;
        if bytes.get(start).is_some_and(|&byte| is_space(byte)) {
            start += 1;
        }
        let end = skip_digits(bytes, start).min(start + 10);
        if end > start {
            return title[start..end].parse().ok();
        }
        from = at + 1;
    }
    None
}

/// The text without any occurrence of an ASCII phrase, whatever its case.
fn remove_ci(text: &str, phrase: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut from = 0;
    while let Some(at) = find_ci(text, phrase, from) {
        result.push_str(&text[from..at]);
        from = at + phrase.len();
    }
    result.push_str(&text[from..]);
    result
}

#[cfg(test)]
mod tests {
    use super::super::router_text::JUNOS_PAGE;
    use super::*;

    /// The form of an lg.cgi looking glass, cut down from a live one.
    const FORM: &str = r##"<HTML><HEAD><TITLE>Example/AS64500 Looking Glass</TITLE></HEAD><BODY>
<FORM METHOD="POST" ACTION="/">
<INPUT TYPE="radio" NAME="query" VALUE="bgp" CHECKED>
<SELECT NAME="protocol"><OPTION VALUE="IPv4" SELECTED> IPv4<OPTION VALUE="IPv6"> IPv6</SELECT>
<INPUT NAME="addr" SIZE="30">
<SELECT NAME="router"><OPTION VALUE="Copenhagen #1"> Copenhagen #1<OPTION VALUE="Frankfurt &amp; Co"> Frankfurt</SELECT>
</FORM></BODY></HTML>
"##;

    /// A variant with the family as radio buttons and the prefix length in its own field.
    const SPLIT_FORM: &str = r##"<html><head><title>
  Example Looking Glass
</title></head><body>
<form method="post" action="lg.cgi" name="lgform">
<input type="radio" name="query" value="bgp">
<input type="radio" name="protocol" value="ipv4-unicast" checked> IPv4
<input type=radio name="protocol" value="ipv6-unicast"> IPv6
<input name="addr" size="30"> / <input name="prefix" size="3">
<select name="router"><option value="r1.example">r1</option></select>
</form></body></html>
"##;

    fn glass(protocols: &[&str], splits_prefix: bool) -> LookingGlass {
        LookingGlass {
            name: "Example".to_string(),
            host: "lg.example.net".to_string(),
            kind: GlassKind::LgCgi,
            routers: vec!["Copenhagen #1".to_string()],
            protocols: protocols.iter().map(|text| text.to_string()).collect(),
            asn: Some(64500),
            form_path: Some("/".to_string()),
            splits_prefix,
            api_version: None,
            query_types: BTreeMap::new(),
            vrfs: BTreeMap::new(),
        }
    }

    #[test]
    fn the_form_names_its_routers_families_and_operator() {
        let description = describe_page(FORM).expect("the form is an lg.cgi form");
        assert_eq!(description.routers, ["Copenhagen #1", "Frankfurt & Co"]);
        assert_eq!(description.protocols, ["IPv4", "IPv6"]);
        assert_eq!(description.asn, Some(64500));
        assert_eq!(description.name.as_deref(), Some("Example/AS64500"));
        assert!(!description.splits_prefix);
        assert_eq!(
            describe_page("<html><form><input name=q></form></html>"),
            None
        );
    }

    #[test]
    fn radio_buttons_and_a_prefix_length_field_are_read() {
        let description = describe_page(SPLIT_FORM).expect("the form is an lg.cgi form");
        assert_eq!(description.routers, ["r1.example"]);
        assert_eq!(description.protocols, ["ipv4-unicast", "ipv6-unicast"]);
        assert_eq!(description.asn, None);
        assert_eq!(description.name.as_deref(), Some("Example"));
        assert!(description.splits_prefix);
        // "prefixlen" is another field, not the prefix length.
        assert!(!has_input(r#"<input name="prefixlen">"#, "prefix"));
    }

    #[test]
    fn the_form_posts_to_its_action() {
        assert_eq!(action(FORM, "/"), "/");
        assert_eq!(
            action(
                r#"<FORM METHOD="POST" ACTION="lg.cgi" NAME="lgform">"#,
                "/lg.cgi"
            ),
            "/lg.cgi"
        );
        assert_eq!(action(SPLIT_FORM, "/cgi-bin/lg.cgi"), "/cgi-bin/lg.cgi");
        assert_eq!(
            action(
                r#"<form action="https://lg.example.net/cgi-bin/lg.cgi?x=1">"#,
                "/"
            ),
            "/cgi-bin/lg.cgi"
        );
        // A search form on the same page is not the looking glass.
        assert_eq!(action(r#"<form action="/search">"#, "/lg.cgi"), "/lg.cgi");
    }

    #[test]
    fn the_form_is_encoded_in_the_order_of_its_fields() {
        let form = BTreeMap::from([
            ("router", "Copenhagen #1".to_string()),
            ("addr", "203.0.113.0/24".to_string()),
        ]);
        assert_eq!(
            encode(&form),
            "addr=203.0.113.0%2F24&router=Copenhagen%20%231"
        );
    }

    #[test]
    fn the_query_takes_the_forms_own_family_name() {
        let plain = encode(&form("203.0.113.0/24", &glass(&[], false), "r1"));
        assert_eq!(
            plain,
            "addr=203.0.113.0%2F24&protocol=IPv4&query=bgp&router=r1"
        );
        let named = form(
            "2001:db8::/32",
            &glass(&["ipv4-unicast", "ipv6-unicast"], false),
            "r1",
        );
        assert_eq!(named["protocol"], "ipv6-unicast");
        assert_eq!(named["addr"], "2001:db8::/32");
    }

    #[test]
    fn a_split_form_gets_the_prefix_length_on_its_own() {
        let split = form("203.0.113.0/24", &glass(&[], true), "r1");
        assert_eq!(split["addr"], "203.0.113.0");
        assert_eq!(split["prefix"], "24");
    }

    #[test]
    fn a_routers_page_gives_its_routes_and_its_text() {
        let glass = glass(&[], false);
        let routes = read(JUNOS_PAGE, "203.0.113.0/24", &glass, "Copenhagen #1");
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].asns, [64500, 64501]);
        let page = format!("<form>noise</form>\n{JUNOS_PAGE}");
        let raw = raw_output(&page);
        assert!(raw.starts_with("Router: Copenhagen #1"));
        assert!(raw.ends_with(">  to 192.0.2.3 via ae1.0"));
    }

    #[test]
    fn an_as_number_is_read_from_a_title() {
        assert_eq!(title_asn("Example/AS64500"), Some(64500));
        assert_eq!(title_asn("Glass of as 64501"), Some(64501));
        assert_eq!(title_asn("Glass"), None);
    }
}
