//! What a node says about itself in its configuration: tags and a location. Parsing and checking
//! only. No node reads it yet (`rvrs node` refuses until initiative B5), and nothing here touches
//! the network or the process's environment: the caller hands over the environment as a reader,
//! so every rule is tested without one. `schema/node-config.md` is the contract and
//! `schema/node-protocol/` carries the result as `tags` and `location` in the enroll and the
//! heartbeat bodies.
//!
//! The three sources are the flags, the environment and a file. A flag beats the environment and
//! the environment beats the file. Tags merge key by key, so a flag's `isp=x` replaces the file's
//! `isp=y` and the file's other tags stay. A location is taken whole from the first source that
//! gives any part of it, so a place from the file never lands on coordinates from a flag.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most tags one install carries.
pub const MAX_TAGS: usize = 20;
/// The longest key or value in characters.
pub const MAX_TAG_LENGTH: usize = 63;
/// The longest place in characters.
pub const MAX_PLACE_LENGTH: usize = 100;
/// The precisions a person may choose, finest first.
pub const PRECISIONS: [&str; 3] = ["exact", "area", "city"];
/// What a location is rounded to when nobody chose.
pub const DEFAULT_PRECISION: &str = "city";

/// The environment variables a node reads.
pub const ENV_TAGS: &str = "RVRS_NODE_TAGS";
pub const ENV_LOCATION: &str = "RVRS_NODE_LOCATION";
pub const ENV_PLACE: &str = "RVRS_NODE_PLACE";
pub const ENV_PRECISION: &str = "RVRS_NODE_LOCATION_PRECISION";

/// Where a node is, as it declares it and as a report keeps it. The precision is a bounded
/// string on the wire and never a closed list: a reader that doesn't know the word treats it as
/// the coarsest, `city`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredLocation {
    pub lat: f64,
    pub lon: f64,
    /// "Esbjerg, DK". Free text, 1 to 100 characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// `exact`, `area` or `city`: how many digits the API keeps. `city` when left out.
    #[serde(default = "city")]
    pub precision: String,
}

fn city() -> String {
    DEFAULT_PRECISION.to_string()
}

/// Everything a node declares. The node sends the whole of it every time it sends any: the API
/// reads an omitted `tags` as "unchanged" and `{}` as "no tags".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDeclaration {
    #[serde(default)]
    pub tags: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<DeclaredLocation>,
}

/// What the command line gave, as typed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Flags {
    /// Each `--tag` as typed, `key=value`.
    pub tags: Vec<String>,
    /// `--location <lat>,<lon>`.
    pub location: Option<String>,
    /// `--place <text>`.
    pub place: Option<String>,
    /// `--location-precision <exact|area|city>`.
    pub precision: Option<String>,
}

fn quoted(text: &str) -> String {
    format!("\u{201C}{text}\u{201D}")
}

/// One `key=value`.
pub fn parse_tag(text: &str) -> Result<(String, String), String> {
    let Some((key, value)) = text.split_once('=') else {
        return Err(format!(
            "The tag {} isn't valid. A tag is a key and a value, as in isp=telia.",
            quoted(text)
        ));
    };
    let (key, value) = (key.trim(), value.trim());
    if !is_key(key) {
        return Err(format!(
            "The tag {} isn't valid. A key is 1 to {MAX_TAG_LENGTH} characters: lowercase letters, digits, \".\", \"-\" and \"_\", starting with a letter.",
            quoted(text)
        ));
    }
    if !is_value(value) {
        return Err(format!(
            "The tag {} isn't valid. A value is 1 to {MAX_TAG_LENGTH} characters: lowercase letters, digits, \".\", \"-\" and \"_\", starting with a letter or a digit.",
            quoted(text)
        ));
    }
    Ok((key.to_string(), value.to_string()))
}

/// `^[a-z][a-z0-9._-]{0,62}$`
pub fn is_key(text: &str) -> bool {
    let mut characters = text.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && text.chars().count() <= MAX_TAG_LENGTH
        && characters.all(tag_character)
}

/// `^[a-z0-9][a-z0-9._-]{0,62}$`
pub fn is_value(text: &str) -> bool {
    let mut characters = text.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && text.chars().count() <= MAX_TAG_LENGTH
        && characters.all(tag_character)
}

fn tag_character(character: char) -> bool {
    character.is_ascii_lowercase()
        || character.is_ascii_digit()
        || matches!(character, '.' | '_' | '-')
}

/// `a=b,c=d`, as `RVRS_NODE_TAGS` holds it. A key that comes twice keeps its last value, and
/// blank entries between commas are skipped.
pub fn parse_tags_list(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut tags = BTreeMap::new();
    for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (key, value) = parse_tag(entry)?;
        tags.insert(key, value);
    }
    Ok(tags)
}

/// `lat,lon`, in decimal degrees: `55.5,8.5`.
pub fn parse_location(text: &str) -> Result<(f64, f64), String> {
    let wrong = || {
        format!(
            "The location {} isn't valid. It is a latitude and a longitude in decimal degrees, as in 55.5,8.5.",
            quoted(text)
        )
    };
    let (lat, lon) = text.split_once(',').ok_or_else(wrong)?;
    let number = |part: &str| part.trim().parse::<f64>().ok().filter(|n| n.is_finite());
    let (lat, lon) = (
        number(lat).ok_or_else(wrong)?,
        number(lon).ok_or_else(wrong)?,
    );
    in_range(lat, lon, text)?;
    Ok((lat, lon))
}

fn in_range(lat: f64, lon: f64, shown: &str) -> Result<(), String> {
    if !(-90.0..=90.0).contains(&lat) {
        return Err(format!(
            "The location {} isn't valid. A latitude runs from -90 to 90.",
            quoted(shown)
        ));
    }
    if !(-180.0..=180.0).contains(&lon) {
        return Err(format!(
            "The location {} isn't valid. A longitude runs from -180 to 180.",
            quoted(shown)
        ));
    }
    Ok(())
}

/// A precision a person typed: one of the three words, in lower case.
pub fn parse_precision(text: &str) -> Result<String, String> {
    let word = text.trim().to_ascii_lowercase();
    if PRECISIONS.contains(&word.as_str()) {
        Ok(word)
    } else {
        Err(format!(
            "The location precision {} isn't one rvrs knows. Use exact, area or city.",
            quoted(text)
        ))
    }
}

/// A place a person typed: 1 to 100 characters of text. Blank means none.
pub fn parse_place(text: &str) -> Result<Option<String>, String> {
    let place = text.trim();
    if place.is_empty() {
        return Ok(None);
    }
    if place.chars().any(char::is_control) {
        return Err(format!(
            "The place {} isn't valid. It is text on one line.",
            quoted(place)
        ));
    }
    if place.chars().count() > MAX_PLACE_LENGTH {
        return Err(format!(
            "The place {} is longer than {MAX_PLACE_LENGTH} characters.",
            quoted(place)
        ));
    }
    Ok(Some(place.to_string()))
}

/// How many decimals of a coordinate a precision keeps: `exact` four (about 10 m), `area` two
/// (about 1 km), `city` one (about 10 km). A word this version doesn't know keeps one, the
/// coarsest, so a newer precision never shows more than its maker meant.
pub fn decimals(precision: &str) -> usize {
    match precision {
        "exact" => 4,
        "area" => 2,
        _ => 1,
    }
}

/// The location as the API stores it. The API does this rounding and a node may send more
/// digits. This is here so a node can show what will be kept.
pub fn round_location(location: &DeclaredLocation) -> DeclaredLocation {
    let scale = 10f64.powi(decimals(&location.precision) as i32);
    // Rounding -0.04 to one decimal gives -0.0, which prints as "-0".
    let round = |degrees: f64| {
        let rounded = (degrees * scale).round() / scale;
        if rounded == 0.0 {
            0.0
        } else {
            rounded
        }
    };
    DeclaredLocation {
        lat: round(location.lat),
        lon: round(location.lon),
        place: location.place.clone(),
        precision: location.precision.clone(),
    }
}

/// What one source says about the location, before the sources are put together.
#[derive(Debug, Default)]
struct LocationParts {
    coordinates: Option<(f64, f64)>,
    place: Option<String>,
    precision: Option<String>,
}

impl LocationParts {
    fn any(&self) -> bool {
        self.coordinates.is_some() || self.place.is_some() || self.precision.is_some()
    }

    fn into_location(self, needs: &str) -> Result<DeclaredLocation, String> {
        let Some((lat, lon)) = self.coordinates else {
            return Err(format!(
                "A place or a precision needs coordinates. Add {needs}, as in 55.5,8.5."
            ));
        };
        Ok(DeclaredLocation {
            lat,
            lon,
            place: self.place,
            precision: self.precision.unwrap_or_else(city),
        })
    }
}

/// Adds where a value came from to a sentence about it.
fn from_source(error: String, source: &str) -> String {
    if source.is_empty() {
        error
    } else {
        format!("{error} It came from {source}.")
    }
}

/// A variable's value, with an empty one read as unset.
fn variable(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    env(name).filter(|value| !value.trim().is_empty())
}

impl NodeDeclaration {
    /// The declaration as the flags, the environment and the file give it. `env` answers a
    /// variable's value, or `None` when it is unset. Every refusal is a whole sentence that
    /// names the input.
    pub fn from_sources(
        flags: &Flags,
        env: &dyn Fn(&str) -> Option<String>,
        file: Option<&NodeDeclaration>,
    ) -> Result<NodeDeclaration, String> {
        let mut tags = file.map(|file| file.tags.clone()).unwrap_or_default();
        if let Some(list) = variable(env, ENV_TAGS) {
            let from_env = parse_tags_list(&list).map_err(|error| from_source(error, ENV_TAGS))?;
            tags.extend(from_env);
        }
        for text in &flags.tags {
            let (key, value) = parse_tag(text)?;
            tags.insert(key, value);
        }
        if tags.len() > MAX_TAGS {
            return Err(format!(
                "A node declares at most {MAX_TAGS} tags, and these are {}.",
                tags.len()
            ));
        }

        // The first source that gives any part of a location gives all of it.
        let mut from_flags = LocationParts::default();
        if let Some(text) = &flags.location {
            from_flags.coordinates = Some(parse_location(text)?);
        }
        if let Some(text) = &flags.place {
            from_flags.place = parse_place(text)?;
        }
        if let Some(text) = &flags.precision {
            from_flags.precision = Some(parse_precision(text)?);
        }
        let mut from_env = LocationParts::default();
        if let Some(text) = variable(env, ENV_LOCATION) {
            from_env.coordinates =
                Some(parse_location(&text).map_err(|error| from_source(error, ENV_LOCATION))?);
        }
        if let Some(text) = variable(env, ENV_PLACE) {
            from_env.place = parse_place(&text).map_err(|error| from_source(error, ENV_PLACE))?;
        }
        if let Some(text) = variable(env, ENV_PRECISION) {
            from_env.precision =
                Some(parse_precision(&text).map_err(|error| from_source(error, ENV_PRECISION))?);
        }
        let location = if from_flags.any() {
            Some(from_flags.into_location("--location <lat>,<lon>")?)
        } else if from_env.any() {
            Some(from_env.into_location(&format!("{ENV_LOCATION}=<lat>,<lon>"))?)
        } else {
            file.and_then(|file| file.location.clone())
        };
        Ok(NodeDeclaration { tags, location })
    }

    /// The node's file, JSON in the shape the heartbeat carries: `tags` as an object and
    /// `location` as an object with `lat`, `lon`, and optionally `place` and `precision`. A key
    /// this version doesn't know is ignored. `name` is how a sentence calls the file.
    pub fn from_file(text: &str, name: &str) -> Result<NodeDeclaration, String> {
        let broken = |what: &str| format!("{name} isn't a node file. {what}");
        let value: Value = serde_json::from_str(text)
            .map_err(|_| broken("It is JSON with tags and a location."))?;
        let Some(fields) = value.as_object() else {
            return Err(broken("It is one JSON object."));
        };
        let mut declaration = NodeDeclaration::default();
        match fields.get("tags") {
            None | Some(Value::Null) => {}
            Some(Value::Object(tags)) => {
                for (key, value) in tags {
                    let Some(value) = value.as_str() else {
                        return Err(from_source(
                            format!("The tag {} isn't valid. A value is text.", quoted(key)),
                            name,
                        ));
                    };
                    let (key, value) = parse_tag(&format!("{key}={value}"))
                        .map_err(|error| from_source(error, name))?;
                    declaration.tags.insert(key, value);
                }
            }
            Some(_) => {
                return Err(broken(
                    "Its tags are an object, as in {\"isp\": \"telia\"}.",
                ))
            }
        }
        match fields.get("location") {
            None | Some(Value::Null) => {}
            Some(Value::Object(location)) => {
                let degrees = |key: &str| location.get(key).and_then(Value::as_f64);
                let (Some(lat), Some(lon)) = (degrees("lat"), degrees("lon")) else {
                    return Err(broken("Its location has a lat and a lon, both numbers."));
                };
                in_range(lat, lon, &format!("{lat},{lon}"))
                    .map_err(|error| from_source(error, name))?;
                let place = match location.get("place") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(text)) => {
                        parse_place(text).map_err(|error| from_source(error, name))?
                    }
                    Some(_) => return Err(broken("Its place is text.")),
                };
                let precision = match location.get("precision") {
                    None | Some(Value::Null) => city(),
                    Some(Value::String(text)) => {
                        parse_precision(text).map_err(|error| from_source(error, name))?
                    }
                    Some(_) => return Err(broken("Its precision is exact, area or city.")),
                };
                declaration.location = Some(DeclaredLocation {
                    lat,
                    lon,
                    place,
                    precision,
                });
            }
            Some(_) => {
                return Err(broken(
                    "Its location is an object, as in {\"lat\": 55.5, \"lon\": 8.5}.",
                ))
            }
        }
        if declaration.tags.len() > MAX_TAGS {
            return Err(format!(
                "A node declares at most {MAX_TAGS} tags, and {name} has {}.",
                declaration.tags.len()
            ));
        }
        Ok(declaration)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_tag_is_a_key_and_a_value() {
        assert_eq!(
            parse_tag("isp=telia"),
            Ok(("isp".to_string(), "telia".to_string()))
        );
        assert_eq!(
            parse_tag(" region = dk-south "),
            Ok(("region".to_string(), "dk-south".to_string()))
        );
        // A value may start with a digit, a key may not.
        assert!(parse_tag("line=5g").is_ok());
        assert!(parse_tag("5g=line").is_err());
        assert!(parse_tag("a.b_c-d=1.2_3-4").is_ok());
    }

    #[test]
    fn a_tag_that_breaks_a_rule_is_refused_with_the_rule() {
        assert_eq!(
            parse_tag("Isp=Telia").unwrap_err(),
            "The tag \u{201C}Isp=Telia\u{201D} isn't valid. A key is 1 to 63 characters: lowercase letters, digits, \".\", \"-\" and \"_\", starting with a letter."
        );
        assert_eq!(
            parse_tag("isp=Telia").unwrap_err(),
            "The tag \u{201C}isp=Telia\u{201D} isn't valid. A value is 1 to 63 characters: lowercase letters, digits, \".\", \"-\" and \"_\", starting with a letter or a digit."
        );
        assert_eq!(
            parse_tag("isp").unwrap_err(),
            "The tag \u{201C}isp\u{201D} isn't valid. A tag is a key and a value, as in isp=telia."
        );
        for bad in [
            "=telia",
            "isp=",
            "-isp=a",
            "isp=-a",
            "isp=a b",
            "isp=\u{00E5}",
        ] {
            assert!(parse_tag(bad).is_err(), "{bad}");
        }
        // 63 characters fit and 64 don't, for a key and for a value.
        let long = "a".repeat(63);
        assert!(parse_tag(&format!("{long}=x")).is_ok());
        assert!(parse_tag(&format!("x={long}")).is_ok());
        assert!(parse_tag(&format!("{long}a=x")).is_err());
        assert!(parse_tag(&format!("x={long}a")).is_err());
    }

    #[test]
    fn a_list_of_tags_keeps_the_last_value_of_a_key() {
        assert_eq!(
            parse_tags_list("isp=telia, line=ftth,,isp=tdc").unwrap(),
            tags(&[("isp", "tdc"), ("line", "ftth")])
        );
        assert_eq!(parse_tags_list("").unwrap(), BTreeMap::new());
        assert!(parse_tags_list("isp=telia,Line=ftth").is_err());
    }

    #[test]
    fn a_location_is_latitude_then_longitude() {
        assert_eq!(parse_location("55.5,8.5"), Ok((55.5, 8.5)));
        assert_eq!(parse_location(" -33.87 , 151.21 "), Ok((-33.87, 151.21)));
        assert_eq!(parse_location("90,-180"), Ok((90.0, -180.0)));
        assert_eq!(
            parse_location("95,8.5").unwrap_err(),
            "The location \u{201C}95,8.5\u{201D} isn't valid. A latitude runs from -90 to 90."
        );
        assert_eq!(
            parse_location("55.5,181").unwrap_err(),
            "The location \u{201C}55.5,181\u{201D} isn't valid. A longitude runs from -180 to 180."
        );
        for bad in ["55.5", "55.5;8.5", "a,b", "55.5,", ",8.5", "nan,1", "1,inf"] {
            assert_eq!(
                parse_location(bad).unwrap_err(),
                format!(
                    "The location \u{201C}{bad}\u{201D} isn't valid. It is a latitude and a longitude in decimal degrees, as in 55.5,8.5."
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_precision_is_one_of_three_words() {
        for word in PRECISIONS {
            assert_eq!(parse_precision(word).as_deref(), Ok(word));
        }
        assert_eq!(parse_precision(" City ").as_deref(), Ok("city"));
        assert_eq!(
            parse_precision("block").unwrap_err(),
            "The location precision \u{201C}block\u{201D} isn't one rvrs knows. Use exact, area or city."
        );
    }

    #[test]
    fn a_place_is_short_text_on_one_line() {
        assert_eq!(
            parse_place(" Esbjerg, DK ").unwrap().as_deref(),
            Some("Esbjerg, DK")
        );
        assert_eq!(parse_place("  ").unwrap(), None);
        assert!(parse_place(&"x".repeat(100)).is_ok());
        assert!(parse_place(&"x".repeat(101)).is_err());
        assert!(parse_place("a\nb").is_err());
    }

    #[test]
    fn rounding_keeps_the_digits_a_precision_allows() {
        let at = |precision: &str| DeclaredLocation {
            lat: 55.543_21,
            lon: -8.456_78,
            place: Some("Esbjerg, DK".into()),
            precision: precision.into(),
        };
        let rounded = round_location(&at("exact"));
        assert_eq!((rounded.lat, rounded.lon), (55.5432, -8.4568));
        let rounded = round_location(&at("area"));
        assert_eq!((rounded.lat, rounded.lon), (55.54, -8.46));
        let rounded = round_location(&at("city"));
        assert_eq!((rounded.lat, rounded.lon), (55.5, -8.5));
        assert_eq!(rounded.place.as_deref(), Some("Esbjerg, DK"));
        // A word this version doesn't know keeps the least.
        let rounded = round_location(&at("block"));
        assert_eq!((rounded.lat, rounded.lon), (55.5, -8.5));
        assert_eq!(rounded.precision, "block");
        // A small negative number rounds to zero, not to minus zero.
        let near = DeclaredLocation {
            lat: -0.04,
            lon: 0.04,
            place: None,
            precision: "city".into(),
        };
        let rounded = round_location(&near);
        assert!(rounded.lat.is_sign_positive() && rounded.lat == 0.0);
        assert_eq!(rounded.lon, 0.0);
        // Rounding twice changes nothing more.
        assert_eq!(round_location(&rounded), rounded);
    }

    #[test]
    fn nothing_given_declares_no_tags_and_no_location() {
        let none = NodeDeclaration::from_sources(&Flags::default(), &no_env, None).unwrap();
        assert_eq!(none, NodeDeclaration::default());
        // On the wire that is `tags: {}` and no location.
        assert_eq!(serde_json::to_string(&none).unwrap(), "{\"tags\":{}}");
    }

    #[test]
    fn a_flag_beats_the_environment_and_the_environment_beats_the_file_key_by_key() {
        let file = NodeDeclaration::from_file(
            r#"{"tags":{"isp":"tdc","line":"dsl","region":"dk-north"}}"#,
            "node.json",
        )
        .unwrap();
        let variables = [(ENV_TAGS, "line=ftth,site=lab")];
        let flags = Flags {
            tags: vec!["isp=telia".into()],
            ..Default::default()
        };
        let declared =
            NodeDeclaration::from_sources(&flags, &env(&variables), Some(&file)).unwrap();
        assert_eq!(
            declared.tags,
            tags(&[
                ("isp", "telia"),
                ("line", "ftth"),
                ("region", "dk-north"),
                ("site", "lab"),
            ])
        );
        // Without the flag the environment's word on `isp` is none, so the file's stands.
        let declared =
            NodeDeclaration::from_sources(&Flags::default(), &env(&variables), Some(&file))
                .unwrap();
        assert_eq!(declared.tags["isp"], "tdc");
        assert_eq!(declared.tags["line"], "ftth");
    }

    #[test]
    fn a_location_comes_whole_from_the_first_source_that_has_any_of_it() {
        let file = NodeDeclaration::from_file(
            r#"{"location":{"lat":55.5,"lon":8.5,"place":"Esbjerg, DK","precision":"area"}}"#,
            "node.json",
        )
        .unwrap();
        // The file alone.
        let declared =
            NodeDeclaration::from_sources(&Flags::default(), &no_env, Some(&file)).unwrap();
        assert_eq!(declared.location, file.location);
        // The environment's coordinates leave the file's place and precision behind.
        let variables = [(ENV_LOCATION, "56.1,10.2")];
        let declared =
            NodeDeclaration::from_sources(&Flags::default(), &env(&variables), Some(&file))
                .unwrap();
        let location = declared.location.unwrap();
        assert_eq!((location.lat, location.lon), (56.1, 10.2));
        assert_eq!(location.place, None);
        assert_eq!(location.precision, "city");
        // A flag beats both.
        let flags = Flags {
            location: Some("57,9.9".into()),
            place: Some("Aalborg, DK".into()),
            precision: Some("exact".into()),
            ..Default::default()
        };
        let declared =
            NodeDeclaration::from_sources(&flags, &env(&variables), Some(&file)).unwrap();
        assert_eq!(
            declared.location,
            Some(DeclaredLocation {
                lat: 57.0,
                lon: 9.9,
                place: Some("Aalborg, DK".into()),
                precision: "exact".into(),
            })
        );
    }

    #[test]
    fn a_place_without_coordinates_is_refused() {
        let flags = Flags {
            place: Some("Esbjerg, DK".into()),
            ..Default::default()
        };
        assert_eq!(
            NodeDeclaration::from_sources(&flags, &no_env, None).unwrap_err(),
            "A place or a precision needs coordinates. Add --location <lat>,<lon>, as in 55.5,8.5."
        );
        let variables = [(ENV_PRECISION, "area")];
        assert_eq!(
            NodeDeclaration::from_sources(&Flags::default(), &env(&variables), None).unwrap_err(),
            "A place or a precision needs coordinates. Add RVRS_NODE_LOCATION=<lat>,<lon>, as in 55.5,8.5."
        );
    }

    #[test]
    fn a_refusal_names_where_the_input_came_from() {
        let variables = [(ENV_TAGS, "isp=telia,Line=ftth")];
        assert_eq!(
            NodeDeclaration::from_sources(&Flags::default(), &env(&variables), None).unwrap_err(),
            "The tag \u{201C}Line=ftth\u{201D} isn't valid. A key is 1 to 63 characters: lowercase letters, digits, \".\", \"-\" and \"_\", starting with a letter. It came from RVRS_NODE_TAGS."
        );
        let variables = [(ENV_LOCATION, "north")];
        let error =
            NodeDeclaration::from_sources(&Flags::default(), &env(&variables), None).unwrap_err();
        assert!(
            error.ends_with("It came from RVRS_NODE_LOCATION."),
            "{error}"
        );
        let variables = [(ENV_PRECISION, "block"), (ENV_LOCATION, "55,8")];
        let error =
            NodeDeclaration::from_sources(&Flags::default(), &env(&variables), None).unwrap_err();
        assert!(
            error.ends_with("It came from RVRS_NODE_LOCATION_PRECISION."),
            "{error}"
        );
        // An empty variable is an unset one.
        let variables = [(ENV_TAGS, ""), (ENV_LOCATION, "  ")];
        assert_eq!(
            NodeDeclaration::from_sources(&Flags::default(), &env(&variables), None).unwrap(),
            NodeDeclaration::default()
        );
        // A bad flag names itself in the sentence.
        let flags = Flags {
            tags: vec!["Isp=Telia".into()],
            ..Default::default()
        };
        let error = NodeDeclaration::from_sources(&flags, &no_env, None).unwrap_err();
        assert!(
            error.starts_with("The tag \u{201C}Isp=Telia\u{201D}"),
            "{error}"
        );
    }

    #[test]
    fn twenty_tags_fit_and_twenty_one_do_not() {
        let list = |count: usize| {
            (0..count)
                .map(|n| format!("k{n}=v"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let twenty = list(20);
        let variables = [(ENV_TAGS, twenty.as_str())];
        assert!(NodeDeclaration::from_sources(&Flags::default(), &env(&variables), None).is_ok());
        let twenty_one = list(21);
        let variables = [(ENV_TAGS, twenty_one.as_str())];
        assert_eq!(
            NodeDeclaration::from_sources(&Flags::default(), &env(&variables), None).unwrap_err(),
            "A node declares at most 20 tags, and these are 21."
        );
    }

    #[test]
    fn the_file_is_the_declaration_as_json() {
        let declared = NodeDeclaration::from_file(
            r#"{"tags":{"isp":"telia","region":"dk-south"},"location":{"lat":55.5,"lon":8.5,"place":"Esbjerg, DK"},"future":1}"#,
            "node.json",
        )
        .unwrap();
        assert_eq!(
            declared.tags,
            tags(&[("isp", "telia"), ("region", "dk-south")])
        );
        assert_eq!(
            declared.location.as_ref().map(|l| l.precision.as_str()),
            Some("city")
        );
        // It serializes back to what the heartbeat carries.
        let body: Value = serde_json::to_value(&declared).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "tags": {"isp": "telia", "region": "dk-south"},
                "location": {"lat": 55.5, "lon": 8.5, "place": "Esbjerg, DK", "precision": "city"}
            })
        );
        assert_eq!(
            NodeDeclaration::from_file("{}", "node.json").unwrap(),
            NodeDeclaration::default()
        );
    }

    #[test]
    fn a_file_that_breaks_a_rule_is_refused_with_its_name() {
        for (text, says) in [
            (
                "tags",
                "node.json isn't a node file. It is JSON with tags and a location.",
            ),
            ("[]", "node.json isn't a node file. It is one JSON object."),
            (
                r#"{"tags":["isp=telia"]}"#,
                "node.json isn't a node file. Its tags are an object, as in {\"isp\": \"telia\"}.",
            ),
            (
                r#"{"tags":{"isp":1}}"#,
                "The tag \u{201C}isp\u{201D} isn't valid. A value is text. It came from node.json.",
            ),
            (r#"{"tags":{"Isp":"x"}}"#, "It came from node.json."),
            (
                r#"{"location":{"lat":55.5}}"#,
                "node.json isn't a node file. Its location has a lat and a lon, both numbers.",
            ),
            (
                r#"{"location":{"lat":95,"lon":8}}"#,
                "A latitude runs from -90 to 90. It came from node.json.",
            ),
            (
                r#"{"location":{"lat":55,"lon":8,"precision":"block"}}"#,
                "Use exact, area or city. It came from node.json.",
            ),
        ] {
            let error = NodeDeclaration::from_file(text, "node.json").unwrap_err();
            assert!(error.contains(says), "{text}: {error}");
        }
    }
}
