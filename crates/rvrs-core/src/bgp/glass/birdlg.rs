//! bird-lg-go (xddxdd/bird-lg-go): server pages around BIRD's own output. The front page links
//! `/summary/<a>+<b>+…`, which names the servers; `/route_all/<server>/<prefix>` runs
//! `show route for <prefix> all` and prints it in a `<pre>`.

use std::collections::BTreeMap;

use super::http::{self, Request, Response};
use super::router_text::{self, decode_entities};
use super::scan::{find_ci, skip_spaces, starts_with_ci};
use super::{GlassKind, GlassResult, GlassSetup, LookingGlass};

pub(super) fn describe(agent: &ureq::Agent, host: &str) -> Result<GlassSetup, String> {
    let not_bird_lg = || format!("{host} didn't answer as a bird-lg-go looking glass.");
    let response = http::fetch(agent, host, "/", &[], Request::Get);
    let page = response
        .text()
        .filter(|_| response.status == Some(200))
        .ok_or_else(not_bird_lg)?;
    let routers = servers(response.final_path.as_deref(), page);
    if routers.is_empty() {
        return Err(not_bird_lg());
    }
    Ok(GlassSetup {
        kind: GlassKind::BirdLg,
        routers,
        name: title(page).map(str::to_string),
        asn: None,
        protocols: Vec::new(),
        form_path: None,
        splits_prefix: false,
        api_version: None,
        query_types: BTreeMap::new(),
        vrfs: BTreeMap::new(),
    })
}

/// The servers the looking glass names. The front page either links "/summary/a+b+c" or
/// redirects there.
fn servers(final_path: Option<&str>, page: &str) -> Vec<String> {
    let from_path = final_path.and_then(|path| path.strip_prefix("/summary/"));
    let Some(list) = from_path.or_else(|| summary_link(page)) else {
        return Vec::new();
    };
    list.split('+')
        .map(|part| decode_entities(part).replace("&#43;", "+"))
        .flat_map(|part| {
            part.split('+')
                .filter(|server| !server.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The server list in the first `href="/summary/…"` that has no further path.
fn summary_link(page: &str) -> Option<&str> {
    const LINK: &str = "href=\"/summary/";
    let bytes = page.as_bytes();
    let mut from = 0;
    while let Some(at) = find_ci(page, LINK, from) {
        let start = at + LINK.len();
        let length = bytes[start..]
            .iter()
            .position(|&byte| byte == b'"' || byte == b'/')?;
        if length > 0 && bytes[start + length] == b'"' {
            return Some(&page[start..start + length]);
        }
        from = at + 1;
    }
    None
}

/// The page title up to its first dash: "Example" from "Example - Bird-lg Go". An empty one is
/// no name.
fn title(page: &str) -> Option<&str> {
    let bytes = page.as_bytes();
    let mut from = 0;
    while let Some(at) = find_ci(page, "<title>", from) {
        let start = skip_spaces(bytes, at + "<title>".len());
        let end = bytes[start..]
            .iter()
            .position(|&byte| byte == b'-' || byte == b'<')
            .map(|offset| start + offset)?;
        if bytes[end] == b'-' || starts_with_ci(page, end, "</title>") {
            return Some(page[start..end].trim_end()).filter(|title| !title.is_empty());
        }
        from = at + 1;
    }
    None
}

/// Asks one server for `show route for <prefix> all`.
pub(super) fn page(agent: &ureq::Agent, host: &str, server: &str, prefix: &str) -> Response {
    let path = format!("/route_all/{server}/{prefix}");
    http::fetch(agent, host, &path, &[], Request::Get)
}

pub(super) fn routes(agent: &ureq::Agent, prefix: &str, glass: &LookingGlass) -> GlassResult {
    super::fan_out(glass, GlassKind::BirdLg, |server| {
        let response = page(agent, &glass.host, server, prefix);
        let html = response.text().filter(|_| response.status == Some(200))?;
        Some(router_text::routes(
            preformatted(html),
            prefix,
            server,
            glass.asn,
        ))
    })
}

/// The router output inside the page's `<pre>`, or the whole page when there is none.
pub(super) fn preformatted(html: &str) -> &str {
    let inside = || {
        let start = find_ci(html, "<pre", 0)? + "<pre".len();
        let open = start + html[start..].find('>')? + 1;
        let end = find_ci(html, "</pre>", open)?;
        Some(&html[open..end])
    };
    inside().unwrap_or(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The front page of a bird-lg-go, cut down from a live one.
    const FRONT: &str = r#"<!DOCTYPE html>
<html><head><title>Example Net - Bird-lg Go</title></head><body>
<nav><a class="navbar-brand" href="/">Example Net</a>
<a class="nav-link" href="/summary/fra1&#43;ams1&#43;zrh1/">Summary</a>
<a class="nav-link" href="/summary/fra1&#43;ams1&#43;zrh1">All servers</a>
<a class="nav-link" href="/summary/fra1">fra1</a></nav>
</body></html>
"#;

    #[test]
    fn the_front_page_names_the_servers() {
        // The link with a further path is passed over for the one without.
        assert_eq!(servers(None, FRONT), ["fra1", "ams1", "zrh1"]);
        assert_eq!(servers(Some("/"), FRONT), ["fra1", "ams1", "zrh1"]);
        assert_eq!(title(FRONT), Some("Example Net"));
        assert!(servers(None, "<html><title>Welcome</title></html>").is_empty());
    }

    #[test]
    fn a_redirect_to_the_summary_names_the_servers_too() {
        assert_eq!(
            servers(Some("/summary/fra1+ams1"), "<html></html>"),
            ["fra1", "ams1"]
        );
        assert_eq!(servers(Some("/summary/fra1+"), FRONT), ["fra1"]);
    }

    #[test]
    fn a_title_ends_at_its_first_dash() {
        assert_eq!(title("<TITLE>\n  Example \n</TITLE>"), Some("Example"));
        assert_eq!(title("<title>- Bird-lg Go</title>"), None);
        assert_eq!(title("<p>no title</p>"), None);
    }

    #[test]
    fn bird_output_is_pulled_out_of_a_page() {
        assert_eq!(
            preformatted("<html><pre class=x>Table master4:\n203.0.113.0/24</pre></html>"),
            "Table master4:\n203.0.113.0/24"
        );
        assert_eq!(preformatted("<html>nothing</html>"), "<html>nothing</html>");
    }
}
