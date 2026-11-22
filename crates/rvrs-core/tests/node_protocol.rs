//! Holds the node protocol's paper together (`schema/node-protocol`): every fixture is an example
//! the OpenAPI document names and matches the schema of the body it stands for, every body and
//! every schema has an example, the page names the routes the document has, and the report inside
//! each result is one the CLI reads. No node and no API are built to it yet, so this is the only
//! thing that keeps the three files in step.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rvrs_core::model::Run;
use rvrs_core::report::Report;
use serde_json::Value;

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/node-protocol")
}

fn read(path: &Path) -> Value {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn document() -> Value {
    read(&folder().join("openapi.json"))
}

/// Keywords that say something about a schema and check nothing.
const NOTES: [&str; 6] = [
    "description",
    "title",
    "$schema",
    "$id",
    "default",
    "summary",
];

/// A value against a schema, for the keywords these two documents use. A keyword this check
/// doesn't know is a problem of its own, so a schema can't gain a rule that nothing checks.
/// It is stricter than JSON Schema in one way: an object with named properties takes no other
/// key unless it says `additionalProperties: true`, so every field of a fixture is one the
/// schema names.
struct Check {
    problems: Vec<String>,
    /// The component schemas a value was checked against.
    seen: BTreeSet<String>,
}

impl Check {
    fn new() -> Self {
        Check {
            problems: Vec::new(),
            seen: BTreeSet::new(),
        }
    }

    fn problem(&mut self, at: &str, what: impl AsRef<str>) {
        self.problems.push(format!("{at}: {}", what.as_ref()));
    }

    /// `file` is the document `schema` sits in, and `root` is that document, for references.
    fn value(&mut self, schema: &Value, value: &Value, at: &str, file: &Path, root: &Value) {
        let Some(rules) = schema.as_object() else {
            return self.problem(at, "the schema is not an object");
        };
        if let Some(reference) = rules.get("$ref").and_then(Value::as_str) {
            return match reference.strip_prefix("#/") {
                Some(pointer) => {
                    if let Some(name) = pointer.strip_prefix("components/schemas/") {
                        self.seen.insert(name.to_string());
                    }
                    match root.pointer(&format!("/{pointer}")) {
                        Some(target) => self.value(target, value, at, file, root),
                        None => self.problem(at, format!("{reference} leads nowhere")),
                    }
                }
                None => {
                    let other = file.parent().unwrap().join(reference);
                    let target = read(&other);
                    self.value(&target, value, at, &other, &target)
                }
            };
        }
        for (keyword, rule) in rules {
            match keyword.as_str() {
                "type" => {
                    let wanted: Vec<&str> = match rule {
                        Value::String(one) => vec![one.as_str()],
                        Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
                        _ => Vec::new(),
                    };
                    if !wanted.iter().any(|kind| is(value, kind)) {
                        self.problem(at, format!("{value} is not {}", wanted.join(" or ")));
                    }
                }
                "properties" => {
                    let (Some(named), Some(fields)) = (rule.as_object(), value.as_object()) else {
                        continue;
                    };
                    for (key, field) in fields {
                        match named.get(key) {
                            Some(inner) => {
                                self.value(inner, field, &format!("{at}.{key}"), file, root)
                            }
                            None if rules.get("additionalProperties")
                                == Some(&Value::Bool(true)) => {}
                            None => self.problem(at, format!("the schema names no \"{key}\"")),
                        }
                    }
                }
                "additionalProperties" => {}
                "required" => {
                    for key in rule
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        if value.get(key).is_none() {
                            self.problem(at, format!("\"{key}\" is missing"));
                        }
                    }
                }
                "items" => {
                    for (index, item) in value.as_array().into_iter().flatten().enumerate() {
                        self.value(rule, item, &format!("{at}[{index}]"), file, root);
                    }
                }
                "anyOf" => {
                    let fits = rule.as_array().into_iter().flatten().any(|option| {
                        let mut inner = Check::new();
                        inner.value(option, value, at, file, root);
                        self.seen.append(&mut inner.seen);
                        inner.problems.is_empty()
                    });
                    if !fits {
                        self.problem(at, "none of the anyOf schemas fits");
                    }
                }
                "enum" => {
                    if !rule
                        .as_array()
                        .is_some_and(|allowed| allowed.contains(value))
                    {
                        self.problem(at, format!("{value} is not one of {rule}"));
                    }
                }
                "minimum" | "maximum" => {
                    let (Some(bound), Some(number)) = (rule.as_f64(), value.as_f64()) else {
                        continue;
                    };
                    if (keyword == "minimum" && number < bound)
                        || (keyword == "maximum" && number > bound)
                    {
                        self.problem(at, format!("{number} breaks {keyword} {bound}"));
                    }
                }
                "minLength" | "maxLength" => {
                    let (Some(bound), Some(text)) = (rule.as_u64(), value.as_str()) else {
                        continue;
                    };
                    let length = text.chars().count() as u64;
                    if (keyword == "minLength" && length < bound)
                        || (keyword == "maxLength" && length > bound)
                    {
                        self.problem(at, format!("{length} characters break {keyword} {bound}"));
                    }
                }
                "maxItems" => {
                    let (Some(bound), Some(items)) = (rule.as_u64(), value.as_array()) else {
                        continue;
                    };
                    if items.len() as u64 > bound {
                        self.problem(at, format!("{} items break maxItems {bound}", items.len()));
                    }
                }
                "format" => {
                    let Some(text) = value.as_str() else { continue };
                    let fits = match rule.as_str() {
                        Some("uuid") => is_uuid(text),
                        Some("date-time") => is_moment(text),
                        _ => true,
                    };
                    if !fits {
                        self.problem(at, format!("\"{text}\" is not a {rule}"));
                    }
                }
                note if NOTES.contains(&note) => {}
                other => self.problem(at, format!("this check doesn't know \"{other}\"")),
            }
        }
    }
}

fn is(value: &Value, kind: &str) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.as_f64().is_some_and(|number| number.fract() == 0.0),
        _ => false,
    }
}

fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.iter().map(|group| group.len()).collect::<Vec<_>>() == [8, 4, 4, 4, 12]
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `2026-11-22T20:15:00Z`, with or without a fraction of a second.
fn is_moment(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && text.ends_with('Z')
}

/// One example the document names: the file, what kind of body it is, and the schema it must fit.
struct Example {
    place: String,
    file: String,
    media: String,
    schema: Value,
    /// For an event stream: the schema of each event's data, by the event's name.
    events: Value,
}

fn examples(document: &Value) -> Vec<Example> {
    let mut found = Vec::new();
    let paths = document["paths"].as_object().unwrap();
    for (path, methods) in paths {
        for (method, operation) in methods.as_object().unwrap() {
            let route = format!("{} {path}", method.to_uppercase());
            let mut bodies: Vec<(String, &Value)> = Vec::new();
            if let Some(content) = operation.pointer("/requestBody/content") {
                bodies.push((format!("{route} body"), content));
            }
            for (status, response) in operation["responses"].as_object().unwrap() {
                if let Some(content) = response.get("content") {
                    bodies.push((format!("{route} {status}"), content));
                }
            }
            for (place, content) in bodies {
                for (media, body) in content.as_object().unwrap() {
                    let named = body.get("examples").and_then(Value::as_object);
                    for example in named.into_iter().flat_map(|examples| examples.values()) {
                        let file = example["externalValue"]
                            .as_str()
                            .unwrap_or_else(|| panic!("{place}: an example without a file"));
                        found.push(Example {
                            place: place.clone(),
                            file: file.to_string(),
                            media: media.clone(),
                            schema: body["schema"].clone(),
                            events: body.get("x-events").cloned().unwrap_or(Value::Null),
                        });
                    }
                }
            }
        }
    }
    found
}

/// The events of a stream as (name, data), in order. Comment lines and blank lines carry none.
fn events(text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let (mut name, mut data) = (String::new(), String::new());
    for line in text.lines().chain([""]) {
        if line.is_empty() {
            if !name.is_empty() || !data.is_empty() {
                found.push((std::mem::take(&mut name), std::mem::take(&mut data)));
            }
        } else if let Some(value) = line.strip_prefix("event:") {
            name = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.trim());
        } else {
            assert!(
                line.starts_with(':'),
                "a line that is no field and no comment: {line}"
            );
        }
    }
    found
}

#[test]
fn every_example_matches_the_schema_of_its_body() {
    let document = document();
    let file = folder().join("openapi.json");
    let mut check = Check::new();
    let examples = examples(&document);
    assert!(!examples.is_empty());
    for example in &examples {
        let at = format!("{} ({})", example.file, example.place);
        let path = folder().join(&example.file);
        if example.media == "text/event-stream" {
            let text = std::fs::read_to_string(&path).unwrap();
            let stream = events(&text);
            assert_eq!(
                stream.first().map(|(name, _)| name.as_str()),
                Some("hello"),
                "{at}"
            );
            assert!(
                text.lines().any(|line| line.starts_with(':')),
                "{at}: no keep-alive"
            );
            for (name, data) in stream {
                let Some(schema) = example.events.get(&name) else {
                    check.problem(&at, format!("the document names no event \"{name}\""));
                    continue;
                };
                let data: Value = serde_json::from_str(&data)
                    .unwrap_or_else(|error| panic!("{at}: {name}: {error}"));
                check.value(schema, &data, &format!("{at} {name}"), &file, &document);
            }
        } else {
            check.value(&example.schema, &read(&path), &at, &file, &document);
        }
    }
    assert!(check.problems.is_empty(), "{}", check.problems.join("\n"));

    // An example reaches every schema the document has, so none is written down and never shown.
    let named: BTreeSet<String> = document["components"]["schemas"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let unseen: Vec<&String> = named.difference(&check.seen).collect();
    assert!(unseen.is_empty(), "no example reaches {unseen:?}");
}

#[test]
fn a_fixture_that_breaks_its_schema_is_caught() {
    let document = document();
    let file = folder().join("openapi.json");
    let schema = &document["components"]["schemas"]["ClaimInput"];
    for (broken, says) in [
        (r#"{"room":1}"#, "\"holding\" is missing"),
        (r#"{"room":-1,"holding":[]}"#, "breaks minimum"),
        (
            r#"{"room":1,"holding":[],"extra":true}"#,
            "names no \"extra\"",
        ),
        (
            r#"{"room":1,"holding":[{"run":"nope","attempt":1}]}"#,
            "is not a \"uuid\"",
        ),
        (r#"{"room":"one","holding":[]}"#, "is not integer"),
    ] {
        let mut check = Check::new();
        let value: Value = serde_json::from_str(broken).unwrap();
        check.value(schema, &value, "claim", &file, &document);
        assert!(
            check.problems.iter().any(|problem| problem.contains(says)),
            "{broken} gave {:?}",
            check.problems
        );
    }
}

#[test]
fn every_fixture_is_an_example_and_every_body_has_one() {
    let document = document();
    let named: BTreeSet<String> = examples(&document)
        .into_iter()
        .map(|example| example.file)
        .collect();
    let on_disk: BTreeSet<String> = std::fs::read_dir(folder().join("fixtures"))
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .map(|name| format!("fixtures/{name}"))
        .collect();
    assert_eq!(named, on_disk);

    for (path, methods) in document["paths"].as_object().unwrap() {
        for (method, operation) in methods.as_object().unwrap() {
            for content in [
                operation.pointer("/requestBody/content"),
                operation.pointer("/responses/200/content"),
                operation.pointer("/responses/201/content"),
            ] {
                let Some(content) = content else { continue };
                let shown = content
                    .as_object()
                    .unwrap()
                    .values()
                    .any(|body| body.get("examples").is_some());
                // A 200 that repeats the 201's shape, for a retry, needs no example of its own.
                let repeats = operation.pointer("/responses/201/content").is_some()
                    && Some(content) == operation.pointer("/responses/200/content");
                assert!(
                    shown || repeats,
                    "{method} {path} has a body without an example"
                );
            }
        }
    }
}

#[test]
fn the_page_and_the_document_name_the_same_routes() {
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();
    let section = page
        .split("## The routes")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the page has a section named The routes");
    let on_page: BTreeSet<String> = section
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|line| line.split('`').next())
        .map(str::to_string)
        .collect();
    let document = document();
    let in_document: BTreeSet<String> = document["paths"]
        .as_object()
        .unwrap()
        .iter()
        .flat_map(|(path, methods)| {
            methods
                .as_object()
                .unwrap()
                .keys()
                .map(move |method| format!("{} {path}", method.to_uppercase()))
        })
        .collect();
    assert_eq!(on_page, in_document);
}

#[test]
fn the_report_in_every_result_is_one_the_cli_reads() {
    let mut read_some = 0;
    for name in [
        "result.request.json",
        "result.refused.request.json",
        "run-request.ended.json",
    ] {
        let body = read(&folder().join("fixtures").join(name));
        let report: Report = serde_json::from_value(body["report"].clone())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(report.origin.kind, "node", "{name}");
        assert_eq!(report.origin.node.as_deref(), Some("fra-1"), "{name}");
        let run = Run::from_report(&report, "abc123", Some("fra-1".to_string()))
            .unwrap_or_else(|| panic!("{name}: no run from its report"));
        assert_eq!(run.summary(), report.summary, "{name}");
        read_some += 1;
    }
    assert_eq!(read_some, 3);
}

#[test]
fn the_fixtures_tell_one_story() {
    let fixture = |name: &str| read(&folder().join("fixtures").join(name));
    let asked = fixture("run-request.request.json");
    let requested = fixture("run-request.requested.json");
    let claimed = &fixture("claim.response.json")["runs"][0];
    let result = fixture("result.request.json");
    let stored = fixture("result.response.json");
    let ended = fixture("run-request.ended.json");

    // The run keeps one id from the ask to the result.
    let id = &requested["id"];
    assert_eq!(&claimed["run"], id);
    assert_eq!(&result["claim"]["run"], id);
    assert_eq!(&stored["id"], id);
    assert_eq!(&ended["id"], id);

    // What the node is handed is what the person asked, and the report is the one it sent.
    for field in ["tool", "target", "options"] {
        assert_eq!(asked[field], claimed[field], "{field}");
        assert_eq!(asked[field], requested[field], "{field}");
    }
    assert_eq!(result["claim"]["attempt"], claimed["attempt"]);
    assert_eq!(ended["report"], result["report"]);
    assert_eq!(ended["state"], "ended");
    assert_eq!(ended["attempt"], claimed["attempt"]);

    // A run that was never claimed has no attempt, and one that expired says why.
    assert_eq!(requested["attempt"], 0);
    let expired = fixture("run-request.expired.json");
    assert_eq!(expired["state"], "expired");
    assert_eq!(expired["reason"], "unclaimed");
    assert_eq!(expired["node"]["online"], false);
}
