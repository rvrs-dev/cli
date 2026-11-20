//! Small HTTP helpers the looking glass adapters share. Always HTTPS, 20 seconds.

use std::time::{Duration, Instant};

use ureq::ResponseExt;

/// What a request came back with. `status` is `None` when no answer came at all.
#[derive(Debug, Clone, Default)]
pub(super) struct Response {
    pub status: Option<u16>,
    pub data: Option<Vec<u8>>,
    /// The address as a person would write it, before percent-encoding.
    pub url: String,
    pub milliseconds: u64,
    /// The path after redirects.
    pub final_path: Option<String>,
}

impl Response {
    /// The body as UTF-8, or nothing when it is not.
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(self.data.as_deref()?).ok()
    }

    /// The body as text, with whatever is not UTF-8 replaced.
    pub fn text_lossy(&self) -> Option<String> {
        self.data
            .as_deref()
            .map(|data| String::from_utf8_lossy(data).into_owned())
    }
}

/// What to send: a plain GET, a GET that asks for JSON, or a POST with a body.
#[derive(Debug, Clone, Copy)]
pub(super) enum Request<'a> {
    Get,
    GetJson,
    Post {
        body: &'a str,
        content_type: &'a str,
    },
}

/// One agent for a batch of requests to a looking glass. It keeps its connections open, never
/// leaves HTTPS, and hands back every status so the adapters can read a 4xx answer's message.
pub(super) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .https_only(true)
        .user_agent("rvrs-cli")
        .build()
        .into()
}

/// Asks `https://<host><path>` and waits for the whole body.
pub(super) fn fetch(
    agent: &ureq::Agent,
    host: &str,
    path: &str,
    query: &[(&str, &str)],
    request: Request,
) -> Response {
    let mut address = format!("https://{host}{}", percent_encode(path, PATH_ALLOWED));
    let mut url = format!("https://{host}{path}");
    for (index, (key, value)) in query.iter().enumerate() {
        let separator = if index == 0 { '?' } else { '&' };
        address.push_str(&format!(
            "{separator}{key}={}",
            percent_encode(value, QUERY_ALLOWED)
        ));
        url.push_str(&format!("{separator}{key}={value}"));
    }
    let mut response = Response {
        url,
        ..Response::default()
    };
    let start = Instant::now();
    let result = match request {
        Request::Get => agent.get(&address).call(),
        Request::GetJson => agent
            .get(&address)
            .header("Accept", "application/json")
            .call(),
        Request::Post { body, content_type } => {
            agent.post(&address).content_type(content_type).send(body)
        }
    };
    if let Ok(mut answer) = result {
        response.status = Some(answer.status().as_u16());
        response.final_path = Some(percent_decode(answer.get_uri().path()));
        response.data = answer.body_mut().read_to_vec().ok();
    }
    response.milliseconds = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    response
}

/// What a URL path carries as it is, besides letters and digits.
const PATH_ALLOWED: &str = "!$&'()*+,-./:=@_~";
/// What a query value carries as it is: a prefix keeps its slash and colons.
const QUERY_ALLOWED: &str = "-._~/:";
/// What a form value carries as it is.
pub(super) const FORM_ALLOWED: &str = "-._~";

/// Percent-encodes everything but ASCII letters, digits and the characters in `allowed`.
pub(super) fn percent_encode(text: &str, allowed: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || allowed.as_bytes().contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Turns `%2B` back into `+`, and leaves what is not an escape as it is.
pub(super) fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escape = (bytes[index] == b'%')
            .then(|| text.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escape {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_keeps_its_plus_signs_and_loses_its_spaces() {
        assert_eq!(
            percent_encode("/summary/a+b c", PATH_ALLOWED),
            "/summary/a+b%20c"
        );
        assert_eq!(
            percent_encode("/route_all/r1/2001:db8::/32", PATH_ALLOWED),
            "/route_all/r1/2001:db8::/32"
        );
        assert_eq!(percent_encode("Router #1", FORM_ALLOWED), "Router%20%231");
    }

    #[test]
    fn an_escaped_path_reads_back() {
        assert_eq!(percent_decode("/summary/a%2Bb%20c"), "/summary/a+b c");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
