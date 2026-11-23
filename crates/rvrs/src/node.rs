//! `rvrs node`. Nodes aren't available yet (initiative B5), so the command still refuses. Before
//! it does, it reads what a node will declare about itself, its tags and its location, from the
//! flags, the environment and the node's file, so a tag or a coordinate it can't read is
//! reported now with the sentence the node will use. `schema/node-config.md` is the contract.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use rvrs_core::node_config::{Flags, NodeDeclaration};
use rvrs_core::paths;

/// Where the node's file is when nothing says otherwise.
pub const FILE_NAME: &str = "node.json";
/// The environment variable that names the node's file.
pub const ENV_CONFIG: &str = "RVRS_NODE_CONFIG";

/// The flags and the file's path from what follows `rvrs node`. A flag or a word this version
/// doesn't know is left alone: the node's own flags arrive with the node, and until then the
/// command refuses whatever it is given.
fn parse(arguments: &[String]) -> Result<(Flags, Option<PathBuf>), String> {
    let mut flags = Flags::default();
    let mut config = None;
    let mut at = 0;
    while at < arguments.len() {
        let argument = &arguments[at];
        at += 1;
        let (name, inline) = match argument.split_once('=') {
            Some((name, value)) if name.starts_with("--") => (name, Some(value.to_string())),
            _ => (argument.as_str(), None),
        };
        let wants = match name {
            "--tag" => "a tag, as in --tag isp=telia",
            "--location" => "a latitude and a longitude, as in --location 55.5,8.5",
            "--place" => "a place, as in --place \"Esbjerg, DK\"",
            "--location-precision" => "exact, area or city",
            "--config" => "the path of a file",
            _ => continue,
        };
        let value = match inline {
            Some(value) => value,
            None => {
                let next = arguments.get(at).filter(|next| !next.starts_with("--"));
                let Some(next) = next else {
                    return Err(format!("{name} takes {wants}."));
                };
                at += 1;
                next.clone()
            }
        };
        match name {
            "--tag" => flags.tags.push(value),
            "--location" => flags.location = Some(value),
            "--place" => flags.place = Some(value),
            "--location-precision" => flags.precision = Some(value),
            _ => config = Some(PathBuf::from(value)),
        }
    }
    Ok((flags, config))
}

/// What the node declares, from the flags, the environment and the file. `read` is how a file is
/// read, so a test needs no disk.
fn declaration(
    arguments: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    default_file: &Path,
    read: &dyn Fn(&Path) -> std::io::Result<String>,
) -> Result<NodeDeclaration, String> {
    let (flags, flag_file) = parse(arguments)?;
    let named = flag_file.or_else(|| {
        env(ENV_CONFIG)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    });
    let explicit = named.is_some();
    let path = named.unwrap_or_else(|| default_file.to_path_buf());
    let file = match read(&path) {
        Ok(text) => {
            let shown = paths::display(&path);
            Some(NodeDeclaration::from_file(&text, &shown)?)
        }
        // The file in the config folder is optional. One the person named has to be there.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit => None,
        Err(error) => {
            return Err(format!(
                "Couldn't read the node's file {}. {}.",
                paths::display(&path),
                capitalized(
                    error
                        .to_string()
                        .split(" (os error")
                        .next()
                        .unwrap_or_default()
                )
            ))
        }
    };
    NodeDeclaration::from_sources(&flags, env, file.as_ref())
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
        .unwrap_or_default()
}

pub fn run(arguments: &[String]) -> ExitCode {
    let env = |name: &str| std::env::var(name).ok();
    let default_file = paths::config_dir().join(FILE_NAME);
    if let Err(message) = declaration(arguments, &env, &default_file, &|path| {
        std::fs::read_to_string(path)
    }) {
        return crate::usage(&message);
    }
    crate::fail(
        "Nodes aren't available yet, so runs come from this device. The waitlist is at rvrs.dev.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    fn none(_: &str) -> Option<String> {
        None
    }

    fn missing(_: &Path) -> std::io::Result<String> {
        Err(Error::new(ErrorKind::NotFound, "no such file"))
    }

    fn default_file() -> PathBuf {
        PathBuf::from("/nowhere/node.json")
    }

    #[test]
    fn the_flags_of_a_node_are_read_in_both_spellings() {
        let (flags, config) = parse(&words(
            "--tag isp=telia --tag=line=ftth --location 55.5,8.5 --place=Esbjerg --location-precision area --config /etc/rvrs/node.json",
        ))
        .unwrap();
        assert_eq!(flags.tags, ["isp=telia", "line=ftth"]);
        assert_eq!(flags.location.as_deref(), Some("55.5,8.5"));
        assert_eq!(flags.place.as_deref(), Some("Esbjerg"));
        assert_eq!(flags.precision.as_deref(), Some("area"));
        assert_eq!(config, Some(PathBuf::from("/etc/rvrs/node.json")));
    }

    #[test]
    fn a_flag_without_its_value_says_what_it_takes() {
        assert_eq!(
            parse(&words("--tag")).unwrap_err(),
            "--tag takes a tag, as in --tag isp=telia."
        );
        assert_eq!(
            parse(&words("--location --place x")).unwrap_err(),
            "--location takes a latitude and a longitude, as in --location 55.5,8.5."
        );
    }

    #[test]
    fn other_words_are_left_to_the_node_that_will_read_them() {
        let (flags, config) = parse(&words("--token abc --api https://api.example.net")).unwrap();
        assert_eq!(flags, Flags::default());
        assert_eq!(config, None);
    }

    #[test]
    fn a_good_declaration_reaches_the_refusal() {
        let declared = declaration(
            &words("--tag isp=telia --location 55.5,8.5"),
            &none,
            &default_file(),
            &missing,
        )
        .unwrap();
        assert_eq!(declared.tags["isp"], "telia");
        assert_eq!(declared.location.unwrap().precision, "city");
    }

    #[test]
    fn a_bad_tag_or_coordinate_is_reported_with_its_sentence() {
        assert_eq!(
            declaration(&words("--tag Isp=Telia"), &none, &default_file(), &missing).unwrap_err(),
            "The tag \u{201C}Isp=Telia\u{201D} isn't valid. A key is 1 to 63 characters: lowercase letters, digits, \".\", \"-\" and \"_\", starting with a letter."
        );
        assert_eq!(
            declaration(&words("--location 95,8"), &none, &default_file(), &missing).unwrap_err(),
            "The location \u{201C}95,8\u{201D} isn't valid. A latitude runs from -90 to 90."
        );
        let env = |name: &str| (name == "RVRS_NODE_TAGS").then(|| "a=b,C=d".to_string());
        assert!(declaration(&[], &env, &default_file(), &missing)
            .unwrap_err()
            .ends_with("It came from RVRS_NODE_TAGS."));
    }

    #[test]
    fn the_file_sits_under_the_environment_and_the_flags() {
        let read = |_: &Path| Ok(r#"{"tags":{"isp":"tdc","site":"lab"}}"#.to_string());
        let env = |name: &str| (name == "RVRS_NODE_TAGS").then(|| "isp=telia".to_string());
        let declared =
            declaration(&words("--tag site=roof"), &env, &default_file(), &read).unwrap();
        assert_eq!(declared.tags["isp"], "telia");
        assert_eq!(declared.tags["site"], "roof");
    }

    #[test]
    fn a_file_the_person_named_has_to_be_there() {
        // The default file may be missing. One named by a flag or the environment may not.
        assert!(declaration(&[], &none, &default_file(), &missing).is_ok());
        let error = declaration(
            &words("--config /etc/rvrs/node.json"),
            &none,
            &default_file(),
            &missing,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Couldn't read the node's file /etc/rvrs/node.json. No such file."
        );
        let env = |name: &str| (name == ENV_CONFIG).then(|| "/etc/rvrs/node.json".to_string());
        assert!(declaration(&[], &env, &default_file(), &missing).is_err());
        // A file that is there and isn't a node file says so, by name.
        let read = |_: &Path| Ok("not json".to_string());
        assert!(declaration(&[], &none, &default_file(), &read)
            .unwrap_err()
            .starts_with("/nowhere/node.json isn't a node file."));
    }
}
