//! Holds the node protocol's paper together (`schema/node-protocol`): every fixture is an example
//! the OpenAPI document names and matches the schema of the body it stands for, every body and
//! every schema has an example, the page names the routes the document has, and the report inside
//! each result is one the CLI reads. The examples of a schedule tell one story: what a person
//! made, what each of its nodes keeps and the run one of them sends.
//!
//! `rvrs node` speaks the protocol and is tested against these fixtures
//! (`crates/rvrs/src/node/service_tests.rs`), and the rvrs API reads its own copy of them. This
//! file is what keeps the page, the document and the fixtures in step with each other.

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
                "additionalProperties" => {
                    // A map: every key the schema doesn't name as a property has this schema.
                    let (true, Some(fields)) = (rule.is_object(), value.as_object()) else {
                        continue;
                    };
                    let named = rules.get("properties").and_then(Value::as_object);
                    for (key, field) in fields {
                        if !named.is_some_and(|named| named.contains_key(key)) {
                            self.value(rule, field, &format!("{at}.{key}"), file, root);
                        }
                    }
                }
                "propertyNames" => {
                    for key in value
                        .as_object()
                        .into_iter()
                        .flat_map(|fields| fields.keys())
                    {
                        self.value(
                            rule,
                            &Value::String(key.clone()),
                            &format!("{at}.{key}"),
                            file,
                            root,
                        );
                    }
                }
                "maxProperties" => {
                    let (Some(bound), Some(fields)) = (rule.as_u64(), value.as_object()) else {
                        continue;
                    };
                    if fields.len() as u64 > bound {
                        self.problem(
                            at,
                            format!("{} properties break maxProperties {bound}", fields.len()),
                        );
                    }
                }
                "pattern" => {
                    let (Some(pattern), Some(text)) = (rule.as_str(), value.as_str()) else {
                        continue;
                    };
                    match pattern_fits(pattern, text) {
                        Some(true) => {}
                        Some(false) => {
                            self.problem(at, format!("\"{text}\" breaks the pattern {pattern}"))
                        }
                        None => {
                            self.problem(at, format!("this check can't read the pattern {pattern}"))
                        }
                    }
                }
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

/// Whether `text` fits a pattern made of bracket classes with an optional `{min,max}` each, as in
/// `^[a-z][a-z0-9._-]{0,62}$`. `None` is a pattern this check can't read.
fn pattern_fits(pattern: &str, text: &str) -> Option<bool> {
    type Atom = (Vec<(char, char)>, usize, usize);
    let body: Vec<char> = pattern
        .strip_prefix('^')?
        .strip_suffix('$')?
        .chars()
        .collect();
    let mut atoms: Vec<Atom> = Vec::new();
    let mut at = 0;
    while at < body.len() {
        if body[at] != '[' {
            return None;
        }
        at += 1;
        let mut ranges = Vec::new();
        while *body.get(at)? != ']' {
            let first = body[at];
            if body.get(at + 1) == Some(&'-') && body.get(at + 2).is_some_and(|last| *last != ']') {
                ranges.push((first, body[at + 2]));
                at += 3;
            } else {
                ranges.push((first, first));
                at += 1;
            }
        }
        at += 1;
        let (least, most) = if body.get(at) == Some(&'{') {
            let end = body[at..].iter().position(|c| *c == '}')? + at;
            let inner: String = body[at + 1..end].iter().collect();
            at = end + 1;
            let (least, most) = inner.split_once(',')?;
            (least.parse().ok()?, most.parse().ok()?)
        } else {
            (1, 1)
        };
        atoms.push((ranges, least, most));
    }
    fn go(atoms: &[Atom], text: &[char]) -> bool {
        let Some(((ranges, least, most), rest)) = atoms.split_first() else {
            return text.is_empty();
        };
        (*least..=(*most).min(text.len())).any(|count| {
            text[..count]
                .iter()
                .all(|c| ranges.iter().any(|(from, to)| (*from..=*to).contains(c)))
                && go(rest, &text[count..])
        })
    }
    Some(go(&atoms, &text.chars().collect::<Vec<_>>()))
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
fn a_declaration_that_breaks_a_rule_is_caught() {
    let document = document();
    let file = folder().join("openapi.json");
    let schema = &document["components"]["schemas"]["HeartbeatInput"];
    let base = |extra: &str| -> Value {
        serde_json::from_str(&format!(
            r#"{{"version":"0.14.2","boot":"k3v9q2ta","features":[],{extra}}}"#
        ))
        .unwrap()
    };
    let twenty_one = (0..21)
        .map(|n| format!("\"k{n}\":\"v\""))
        .collect::<Vec<_>>()
        .join(",");
    for (extra, says) in [
        (r#""tags":{"Isp":"telia"}"#, "breaks the pattern"),
        (r#""tags":{"isp":"Telia"}"#, "breaks the pattern"),
        (r#""tags":{"isp":"-telia"}"#, "breaks the pattern"),
        (r#""tags":{"5g":"yes"}"#, "breaks the pattern"),
        (r#""tags":{"isp":1}"#, "is not string"),
        (r#""tags":["isp=telia"]"#, "is not object"),
        (
            &format!(r#""tags":{{{twenty_one}}}"#),
            "break maxProperties 20",
        ),
        (r#""location":{"lat":91,"lon":8}"#, "breaks maximum 90"),
        (r#""location":{"lat":55,"lon":-181}"#, "breaks minimum -180"),
        (r#""location":{"lat":55}"#, "\"lon\" is missing"),
        (
            r#""location":{"lat":55,"lon":8,"precision":"an unreasonably long precision"}"#,
            "break maxLength 20",
        ),
        (
            r#""location":{"lat":55,"lon":8,"place":""}"#,
            "break minLength 1",
        ),
        (
            r#""location":{"lat":55,"lon":8,"zip":"6700"}"#,
            "names no \"zip\"",
        ),
    ] {
        let mut check = Check::new();
        check.value(schema, &base(extra), "heartbeat", &file, &document);
        assert!(
            check.problems.iter().any(|problem| problem.contains(says)),
            "{extra} gave {:?}",
            check.problems
        );
    }
    // A precision this version doesn't know is still a bounded string, never a closed list.
    let mut check = Check::new();
    let open = base(r#""location":{"lat":55,"lon":8,"precision":"block"}"#);
    check.value(schema, &open, "heartbeat", &file, &document);
    assert!(check.problems.is_empty(), "{:?}", check.problems);
}

#[test]
fn the_declaration_in_the_fixtures_is_one_rvrs_core_reads() {
    use rvrs_core::node_config::{is_key, is_value, round_location, DeclaredLocation};
    let fixture = |name: &str| read(&folder().join("fixtures").join(name));
    let (enroll, heartbeat) = (
        fixture("enroll.request.json"),
        fixture("heartbeat.request.json"),
    );
    // The node sends its whole declaration every time, so the two say the same.
    assert_eq!(enroll["tags"], heartbeat["tags"]);
    assert_eq!(enroll["location"], heartbeat["location"]);
    for (key, value) in heartbeat["tags"].as_object().unwrap() {
        assert!(is_key(key) && is_value(value.as_str().unwrap()), "{key}");
    }
    // What the API stores for a city is what the node declared, since the fixture is rounded.
    let location: DeclaredLocation = serde_json::from_value(heartbeat["location"].clone()).unwrap();
    assert_eq!(round_location(&location), location);
}

#[test]
fn the_page_states_the_rules_of_a_declaration() {
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();
    for rule in [
        "A heartbeat or an enroll that leaves out `tags` or `location` changes nothing.",
        "`tags: {}` says the node declares no tags.",
        "A tag set on the web is never undone by a declaration.",
    ] {
        assert!(page.contains(rule), "the page lacks: {rule}");
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
        "result.scheduled.request.json",
        "run-request.ended.json",
    ] {
        let body = read(&folder().join("fixtures").join(name));
        let report: Report = serde_json::from_value(body["report"].clone())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(report.origin.kind, "node", "{name}");
        assert_eq!(report.origin.node.as_deref(), Some("fra-1"), "{name}");
        // The name is a snapshot. The id is the one the API answered the node's heartbeat with.
        let node = read(&folder().join("fixtures/heartbeat.response.json"));
        assert_eq!(
            body["report"]["origin"]["nodeId"], node["node"]["id"],
            "{name}"
        );
        let run = Run::from_report(&report, "abc123", Some("fra-1".to_string()))
            .unwrap_or_else(|| panic!("{name}: no run from its report"));
        assert_eq!(run.summary(), report.summary, "{name}");
        read_some += 1;
    }
    assert_eq!(read_some, 4);
}

#[test]
fn a_schedules_fixtures_tell_one_story() {
    let fixture = |name: &str| read(&folder().join("fixtures").join(name));
    let asked = fixture("schedule.request.json");
    let made = fixture("schedule.response.json");
    let by_selector = fixture("schedule.selector.response.json");
    let first = fixture("node-schedules.response.json");
    let other = fixture("node-schedules.other.response.json");

    // The schedule is what the person asked for, on the nodes they named.
    for field in ["name", "tool", "target", "options", "intervalSeconds"] {
        assert_eq!(asked[field], made[field], "{field}");
    }
    let named: BTreeSet<&str> = made["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect();
    let wanted: BTreeSet<&str> = asked["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(named, wanted);
    assert_eq!(made["nodeCount"], 2);
    let selected = fixture("schedule.selector.request.json");
    for field in ["tool", "target", "options", "intervalSeconds", "selector"] {
        assert_eq!(selected[field], by_selector[field], "{field}");
    }

    // Each node's copy says about a schedule what the schedule says: the tool, the target, the
    // options, and the interval the nodes keep.
    let entry = |copy: &Value, id: &Value| -> Value {
        copy["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| &entry["schedule"] == id)
            .unwrap_or_else(|| panic!("no entry for {id}"))
            .clone()
    };
    let mut entries = 0;
    for copy in [&first, &other] {
        for kept in copy["schedules"].as_array().unwrap() {
            let schedule = [&made, &by_selector]
                .into_iter()
                .find(|schedule| schedule["id"] == kept["schedule"])
                .unwrap_or_else(|| panic!("no schedule for {}", kept["schedule"]));
            for field in ["tool", "target", "options", "everySeconds"] {
                assert_eq!(kept[field], schedule[field], "{field}");
            }
            // A place in the interval is never further away than the interval is long.
            assert!(
                kept["startsInMilliseconds"].as_u64().unwrap()
                    < kept["everySeconds"].as_u64().unwrap() * 1000
            );
            entries += 1;
        }
    }
    assert_eq!(entries, 3);

    // The two nodes of the shared schedule get different starts, so they never run it in the
    // same second. The page tells the same numbers.
    let (here, there) = (entry(&first, &made["id"]), entry(&other, &made["id"]));
    assert_ne!(here["startsInMilliseconds"], there["startsInMilliseconds"]);
    assert_eq!(here["startsInMilliseconds"], 41_250);
    assert_eq!(there["startsInMilliseconds"], 9_800);
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();
    assert!(page
        .contains("`fra-1` is told its next run starts in 41,250 ms and `cpe-4471` in 9,800 ms"));
    // The copies differ, so their versions do. The heartbeat's answer names the version of the
    // copy its node should hold.
    assert_ne!(first["version"], other["version"]);
    assert_eq!(
        fixture("heartbeat.response.json")["schedules"],
        first["version"]
    );

    // The cap per target made the selector's schedule longer than the person asked for, and its
    // nodes keep the longer interval.
    assert!(
        by_selector["everySeconds"].as_u64().unwrap()
            > by_selector["intervalSeconds"].as_u64().unwrap()
    );
    assert_eq!(made["everySeconds"], made["intervalSeconds"]);

    // Pausing changes that and when it was changed, and nothing else.
    let paused = fixture("schedule.paused.response.json");
    assert_eq!(fixture("schedule.change.request.json")["paused"], true);
    assert_eq!(paused["paused"], true);
    for (field, value) in made.as_object().unwrap() {
        if !["paused", "updatedAt", "lastRunAt"].contains(&field.as_str()) {
            assert_eq!(&paused[field], value, "{field}");
        }
    }
    // The list has both, each as its own answer has it but for when it last ran.
    let listed = fixture("schedules.response.json");
    let ids: Vec<&Value> = listed["schedules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|schedule| &schedule["id"])
        .collect();
    assert_eq!(ids, [&by_selector["id"], &made["id"]]);
}

#[test]
fn a_scheduled_run_names_its_schedule_and_no_claim() {
    let fixture = |name: &str| read(&folder().join("fixtures").join(name));
    let result = fixture("result.scheduled.request.json");
    let schedule = fixture("schedule.response.json");
    // The run is one of the schedule the two nodes share: its id beside the report, and the
    // tool and the target the schedule names, the tool without regard to case.
    assert_eq!(result["schedule"], schedule["id"]);
    assert!(result.get("claim").is_none());
    assert!(result["startedAgoMilliseconds"].is_u64());
    let report: Report = serde_json::from_value(result["report"].clone()).unwrap();
    assert_eq!(
        report.tool.to_lowercase(),
        schedule["tool"].as_str().unwrap()
    );
    assert_eq!(report.target, schedule["target"].as_str().unwrap());
    assert_eq!(report.origin.kind, "node");
    // A report's id never repeats on its node, so it is no id a claimed run's report has.
    assert_ne!(
        result["report"]["id"],
        fixture("result.request.json")["report"]["id"]
    );

    let document = document();
    let input = &document["components"]["schemas"]["ResultInput"];
    assert_eq!(input["properties"]["schedule"]["format"], "uuid");
    let required = input["required"].as_array().unwrap();
    assert!(!required.iter().any(|field| field == "schedule"));
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();
    assert!(page.contains("A run with `claim` carries no `schedule`."));
}

#[test]
fn a_node_keeps_schedules_at_revision_2() {
    use rvrs_core::node::schedules::{Copy, AT_MOST};
    let fixture = |name: &str| read(&folder().join("fixtures").join(name));
    let document = document();
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();

    // The heartbeat says the revision this build speaks, and the page says what it means.
    assert_eq!(
        fixture("heartbeat.request.json")["protocol"],
        rvrs_core::node::PROTOCOL
    );
    assert!(page.contains("Revision 2 is a node that keeps schedules"));

    // An API from before schedules names no version and answers the read with 404, so a node
    // can count on neither.
    let node = &document["components"]["schemas"]["Node"];
    assert_eq!(node["properties"]["schedules"]["type"], "string");
    let required = node["required"].as_array().unwrap();
    assert!(!required.iter().any(|field| field == "schedules"));
    assert!(page.contains("An API from before schedules answers 404."));

    // The stream's example wakes the node for its schedules, with nothing about them.
    let stream = std::fs::read_to_string(folder().join("fixtures/events.txt")).unwrap();
    let woken: Vec<(String, String)> = events(&stream)
        .into_iter()
        .filter(|(name, _)| name == "schedules")
        .collect();
    assert_eq!(woken, [("schedules".to_string(), "{}".to_string())]);

    // The numbers the page promises are the ones the document and the examples carry, and the
    // ones the node reads a copy by.
    assert!(page.contains(
        "| A node's copy of its schedules | 50 schedules, good for 604,800 s | `keepSeconds` in the copy |"
    ));
    let copies = &document["components"]["schemas"]["NodeSchedules"];
    assert_eq!(copies["properties"]["schedules"]["maxItems"], AT_MOST);
    for name in [
        "node-schedules.response.json",
        "node-schedules.other.response.json",
    ] {
        let answer = fixture(name);
        assert_eq!(answer["keepSeconds"], 604_800, "{name}");
        // What rvrs node reads from the example is the example.
        let copy = Copy::read(&answer).unwrap_or_else(|| panic!("{name}: no copy"));
        assert_eq!(copy.version, answer["version"].as_str().unwrap(), "{name}");
        assert_eq!(
            copy.schedules.len(),
            answer["schedules"].as_array().unwrap().len(),
            "{name}"
        );
        for (read, written) in copy
            .schedules
            .iter()
            .zip(answer["schedules"].as_array().unwrap())
        {
            assert_eq!(read.schedule, written["schedule"].as_str().unwrap());
            assert_eq!(
                read.starts_in_milliseconds,
                written["startsInMilliseconds"].as_u64().unwrap()
            );
        }
    }
    let interval =
        &document["components"]["schemas"]["ScheduleInput"]["properties"]["intervalSeconds"];
    assert_eq!(
        (interval["minimum"].as_u64(), interval["maximum"].as_u64()),
        (Some(60), Some(86_400))
    );
    assert!(page.contains("| A schedule's interval | 60 to 86,400 s |"));
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

    // The API's clock says when it saw things, and works the start out from what the node
    // counted. Nothing here reads the report's own startedAt, which is the node's clock.
    assert!(requested["claimedAt"].is_null() && requested["receivedAt"].is_null());
    assert!(ended["claimedAt"].is_string());
    assert_eq!(ended["receivedAt"], stored["receivedAt"]);
    let ago = result["startedAgoMilliseconds"].as_i64().unwrap();
    let received = milliseconds_of_day(ended["receivedAt"].as_str().unwrap());
    let observed = milliseconds_of_day(ended["observedStartedAt"].as_str().unwrap());
    assert_eq!(received - observed, ago);
    assert!(milliseconds_of_day(ended["claimedAt"].as_str().unwrap()) <= observed);
    assert_eq!(ended["node"]["id"], ended["report"]["origin"]["nodeId"]);

    // A run that was never claimed has no attempt, and one that expired says why.
    assert_eq!(requested["attempt"], 0);
    let expired = fixture("run-request.expired.json");
    assert_eq!(expired["state"], "expired");
    assert_eq!(expired["reason"], "unclaimed");
    assert_eq!(expired["node"]["online"], false);
}

/// The time of day of `2026-11-22T20:15:16.412Z` in milliseconds. The fixtures share one day.
fn milliseconds_of_day(moment: &str) -> i64 {
    let time = &moment[11..23];
    let part = |from: usize, to: usize| time[from..to].parse::<i64>().unwrap();
    ((part(0, 2) * 60 + part(3, 5)) * 60 + part(6, 8)) * 1000 + part(9, 12)
}

#[test]
fn a_second_enroll_shows_the_same_answer_as_the_first() {
    // Enrolling again with the same key gets the same node and the same token, so two answers
    // that arrive out of order say the same thing. On paper that is one example for both.
    let document = document();
    let example = |status: &str| {
        document
            .pointer(&format!(
                "/paths/~1v1~1nodes/post/responses/{status}/content/application~1json"
            ))
            .unwrap_or_else(|| panic!("enroll has no {status}"))
            .clone()
    };
    let (first, again) = (example("201"), example("200"));
    assert_eq!(first["schema"], again["schema"]);
    let file = |body: &Value| -> Vec<String> {
        body["examples"]
            .as_object()
            .unwrap()
            .values()
            .map(|example| example["externalValue"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(file(&first), file(&again));
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();
    assert!(page.contains("No enroll revokes or replaces a token."));
}

#[test]
fn an_unknown_verdict_is_never_read_as_keep() {
    // The page and the document have to say the same about a word a node doesn't know.
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();
    assert!(page.contains("A verdict the node doesn't know is never read as `keep`."));
    let document = document();
    let verdict = document["components"]["schemas"]["Verdict"]["properties"]["verdict"]
        ["description"]
        .as_str()
        .unwrap();
    assert!(verdict.contains("never read as keep"), "{verdict}");
    // And the numbers the page promises are the ones the stream's example carries.
    let stream = std::fs::read_to_string(folder().join("fixtures/events.txt")).unwrap();
    assert!(stream.contains("\"claimSeconds\":1800"));
    assert!(page.contains("| A claim while idle | At random between 900 and 1,800 s |"));
}

#[test]
fn an_answer_without_the_apis_error_is_read_as_a_5xx() {
    // A load balancer answers 404 with a page of its own when no server behind it has room. Read
    // as the API's 404, it would make a node drop a result it should send again.
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();
    assert!(page.contains(
        "A node treats an answer without the API's error, whatever its status, as it treats a 5xx."
    ));
    assert!(page.contains("| 5xx, an answer without the API's error, or no answer |"));
}

#[test]
fn the_stream_can_move_without_updating_a_node() {
    let document = document();
    let node = &document["components"]["schemas"]["Node"];
    assert_eq!(node["properties"]["events"]["type"], "string");
    // An API from before the field names no address, so a node can't count on one.
    let required = node["required"].as_array().unwrap();
    assert!(!required.iter().any(|field| field == "events"));
    let answer = read(&folder().join("fixtures/heartbeat.response.json"));
    assert!(answer["events"].as_str().unwrap().starts_with("https://"));
}

#[test]
fn a_claim_sent_again_costs_no_attempt() {
    // A lost answer and a restart look the same in `holding`. The key tells them apart, and a
    // node from before it sends none, so it stays optional.
    let document = document();
    let claim = &document["components"]["schemas"]["ClaimInput"];
    assert_eq!(claim["properties"]["key"]["type"], "string");
    let required = claim["required"].as_array().unwrap();
    assert!(!required.iter().any(|field| field == "key"));
    for fixture in ["claim.request.json", "claim.holding.request.json"] {
        let body = read(&folder().join("fixtures").join(fixture));
        assert!(
            body["key"].as_str().is_some_and(|key| !key.is_empty()),
            "{fixture}"
        );
    }
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();
    assert!(page.contains("its runs come back as the same attempts"));
    assert!(page.contains("A node makes a new key for every new claim and after every start"));
}

#[test]
fn a_result_is_the_run_that_was_asked_for() {
    let page = std::fs::read_to_string(folder().join("README.md")).unwrap();
    assert!(page.contains("422 says the report is not the run that was asked for"));
    assert!(page.contains("422 also says the report's `id` is one the node used for another run"));
    // The examples keep the rule: the refused result names the tool it was asked with.
    let refused = read(&folder().join("fixtures/result.refused.request.json"));
    assert_eq!(
        refused["report"]["tool"].as_str().unwrap().to_lowercase(),
        "iperf3"
    );
}
