//! The node's side of the wire: the calls of the node protocol (`schema/node-protocol`), and how
//! an answer is read.
//!
//! An answer is one of three things. The API said yes. The API said no, in its error shape, with
//! a code a node branches on. Or there is no answer from the API: nothing came back, the API
//! failed (5xx), or something on the way answered with a page of its own, as a load balancer does
//! with a 404 when no server behind it has room and a captive portal does with a 200. The last
//! three are one case, and the call is tried again.

use std::io::Read;
use std::time::Duration;

use rvrs_core::node::setup::Secret;
use serde_json::Value;

use crate::help::VERSION;

/// What came back for a call.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// 2xx, with the body, which is a JSON object.
    Taken(Value),
    /// The API refused, in its own words: the status, the error's code and, for a 429, how long
    /// to wait.
    Refused {
        status: u16,
        code: String,
        retry_after: Option<u64>,
    },
    /// No answer from the API. Tried again later.
    None,
}

impl Answer {
    /// Reads a status and a body by the protocol's rule: an answer without the API's error shape
    /// didn't come from the API, whatever its status. The same holds for a good status: every
    /// answer of the API a node reads is a JSON object, and anything else under a 2xx came from
    /// something on the way.
    pub fn read(status: u16, body: &str, retry_after: Option<u64>) -> Answer {
        let json: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            return if json.is_object() {
                Answer::Taken(json)
            } else {
                Answer::None
            };
        }
        match json["error"]["code"].as_str() {
            Some(code) if status < 500 => Answer::Refused {
                status,
                code: code.to_string(),
                retry_after,
            },
            _ => Answer::None,
        }
    }
}

/// The API as one node calls it: its address and the node's token. Nothing else in the binary
/// holds the token, and no line it prints has more than the token's first characters.
pub struct Api {
    base: String,
    token: Secret,
}

impl Api {
    pub fn new(base: &str, token: Secret) -> Api {
        Api {
            base: base.trim_end_matches('/').to_string(),
            token,
        }
    }

    /// The API's address, for a line that says where the node is connected.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The token's first characters, to tell tokens apart in a log.
    pub fn token_prefix(&self) -> String {
        self.token.prefix()
    }

    /// Where the event stream is when no heartbeat's answer named another address.
    pub fn events(&self) -> String {
        format!("{}/v1/nodes/me/events", self.base)
    }

    /// An address the heartbeat's answer named for the stream, when it may be used: one whose
    /// scheme is the API's. An address with another scheme is ignored.
    pub fn events_at(&self, named: Option<&str>) -> String {
        let scheme = |url: &str| url.split_once("://").map(|(scheme, _)| scheme.to_string());
        match named {
            Some(url) if scheme(url).is_some() && scheme(url) == scheme(&self.base) => {
                url.to_string()
            }
            _ => self.events(),
        }
    }

    fn agent(seconds: Option<u64>) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(seconds.map(Duration::from_secs))
            .user_agent(format!("rvrs/{VERSION}"))
            // A 4xx is an answer to read, with the reason in its body.
            .http_status_as_error(false)
            .build()
            .into()
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.token.expose())
    }

    fn post(&self, path: &str, body: &Value, seconds: u64) -> Answer {
        let sent = Self::agent(Some(seconds))
            .post(format!("{}{path}", self.base))
            .header("content-type", "application/json")
            .header("authorization", self.bearer())
            .send(body.to_string());
        let Ok(mut response) = sent else {
            return Answer::None;
        };
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse().ok());
        let text = response.body_mut().read_to_string().unwrap_or_default();
        Answer::read(status, &text, retry_after)
    }

    /// Says what the node is. The answer names the node and where its stream is.
    pub fn heartbeat(&self, beat: &Value) -> Answer {
        self.post("/v1/nodes/me/heartbeats", beat, 20)
    }

    /// Takes the runs that wait and keeps the ones the node holds.
    pub fn claim(&self, claim: &Value) -> Answer {
        self.post("/v1/nodes/me/claims", claim, 20)
    }

    /// Sends a finished run.
    pub fn result(&self, result: &Value) -> Answer {
        self.post("/v1/runs", result, 30)
    }

    /// Opens the event stream at `url` and hands back its body to read. A read fails once
    /// nothing arrived for `silent`, so the thread that reads a connection that died without a
    /// word ends by itself and gives its socket back. The stream is cut after `longest` in any
    /// case.
    pub fn stream(
        &self,
        url: &str,
        longest: Duration,
        silent: Duration,
    ) -> Result<Box<dyn Read + Send>, Answer> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(20)))
            .timeout_recv_body(Some(longest))
            .timeout_per_read(Some(silent))
            .user_agent(format!("rvrs/{VERSION}"))
            .http_status_as_error(false)
            .build()
            .into();
        let opened = agent
            .get(url)
            .header("accept", "text/event-stream")
            .header("authorization", self.bearer())
            .call();
        let Ok(mut response) = opened else {
            return Err(Answer::None);
        };
        let status = response.status().as_u16();
        let is_stream = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|kind| kind.starts_with("text/event-stream"));
        if status == 200 && is_stream {
            return Ok(Box::new(response.into_body().into_reader()));
        }
        let text = response.body_mut().read_to_string().unwrap_or_default();
        match Answer::read(status, &text, None) {
            // A 200 that is no stream didn't come from the API either.
            Answer::Taken(_) => Err(Answer::None),
            other => Err(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_without_the_apis_error_is_no_answer() {
        // A load balancer's page, whatever its status.
        assert_eq!(
            Answer::read(404, "<html>Not Found</html>", None),
            Answer::None
        );
        assert_eq!(Answer::read(403, "", None), Answer::None);
        // A captive portal's page comes with a good status, and so may a proxy's.
        assert_eq!(
            Answer::read(200, "<html>Sign in</html>", None),
            Answer::None
        );
        assert_eq!(Answer::read(200, "", None), Answer::None);
        assert_eq!(Answer::read(201, "\"ok\"", None), Answer::None);
        // The API's own failure is tried again as well.
        let failed = r#"{"error":{"code":"INTERNAL_ERROR","message":"x"}}"#;
        assert_eq!(Answer::read(500, failed, None), Answer::None);
        assert_eq!(Answer::read(503, failed, None), Answer::None);
    }

    #[test]
    fn the_apis_refusal_carries_its_code() {
        let lost = r#"{"error":{"code":"CLAIM_LOST","message":"x"}}"#;
        assert_eq!(
            Answer::read(409, lost, None),
            Answer::Refused {
                status: 409,
                code: "CLAIM_LOST".into(),
                retry_after: None
            }
        );
        let limited = r#"{"error":{"code":"RATE_LIMITED","message":"x"}}"#;
        assert_eq!(
            Answer::read(429, limited, Some(12)),
            Answer::Refused {
                status: 429,
                code: "RATE_LIMITED".into(),
                retry_after: Some(12)
            }
        );
        assert_eq!(
            Answer::read(201, r#"{"id":"a"}"#, None),
            Answer::Taken(serde_json::json!({ "id": "a" }))
        );
    }

    #[test]
    fn the_stream_is_opened_where_the_api_says_when_the_scheme_is_the_apis() {
        let api = Api::new("https://api.rvrs.dev/", Secret::new("rvrs_abc"));
        assert_eq!(api.events(), "https://api.rvrs.dev/v1/nodes/me/events");
        let named = "https://events.rvrs.dev/v1/nodes/me/events";
        assert_eq!(api.events_at(Some(named)), named);
        assert_eq!(api.events_at(None), api.events());
        // Another scheme is ignored.
        assert_eq!(
            api.events_at(Some("http://events.rvrs.dev/v1/nodes/me/events")),
            api.events()
        );
        assert_eq!(api.events_at(Some("events.rvrs.dev")), api.events());
    }
}
