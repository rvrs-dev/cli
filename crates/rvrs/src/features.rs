//! The build features: the parts of rvrs a build can include or leave out
//! (`docs/build-features.md`). Each one is a Cargo feature of this crate of the same name, and a
//! build has all of them unless it was made without some. A node names the ones it has as
//! `features` in its heartbeat, so a name never changes.
//!
//! A build feature decides what a build can run, never what it can show. Every build reads every
//! run, and a run of a tool this build leaves out is shown from its report, as a run another
//! device made is.
//!
//! What a build leaves out is the one thing it remembers about its build features. `rvrs update`
//! and the installer ask for a build that leaves out the same ones, so a build feature added in a
//! later version is in the next update unless the person leaves it out too. A build from before
//! build features has everything, and so does its update.

use std::process::ExitCode;

use rvrs_core::model::Tool;

/// One part of rvrs that a build can leave out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    /// The name flags, Cargo and a node's heartbeat use.
    pub name: &'static str,
    /// The build feature in a sentence: "BGP", "the Console".
    pub title: &'static str,
    /// What it does, for `rvrs status` and the docs.
    pub about: &'static str,
    /// The build features it works inside. Leaving one of them out leaves this one out too.
    pub needs: &'static [&'static str],
    /// Whether this build has it.
    pub included: bool,
}

/// Every build feature this version knows, in the order a node lists them.
pub const ALL: [Feature; 8] = [
    Feature {
        name: "traceroute",
        title: "traceroute",
        about: "rvrs traceroute",
        needs: &[],
        included: cfg!(feature = "traceroute"),
    },
    Feature {
        name: "ping",
        title: "ping",
        about: "rvrs ping",
        needs: &[],
        included: cfg!(feature = "ping"),
    },
    Feature {
        name: "mtr",
        title: "MTR",
        about: "rvrs mtr",
        needs: &[],
        included: cfg!(feature = "mtr"),
    },
    Feature {
        name: "dns",
        title: "DNS",
        about: "rvrs dig",
        needs: &[],
        included: cfg!(feature = "dns"),
    },
    Feature {
        name: "bgp",
        title: "BGP",
        about: "rvrs bgp",
        needs: &[],
        included: cfg!(feature = "bgp"),
    },
    Feature {
        name: "tcp",
        title: "the TCP probe",
        about: "rvrs traceroute -T",
        needs: &["traceroute"],
        included: cfg!(feature = "tcp"),
    },
    Feature {
        name: "mtu",
        title: "the path MTU check",
        about: "rvrs traceroute --mtu",
        needs: &["traceroute"],
        included: cfg!(feature = "mtu"),
    },
    Feature {
        name: "console",
        title: "the Console",
        about: "rvrs with no command",
        needs: &[],
        included: cfg!(feature = "console"),
    },
];

pub fn find(name: &str) -> Option<&'static Feature> {
    ALL.iter().find(|feature| feature.name == name)
}

/// Whether this build has the build feature. A name this version doesn't know is never in it.
pub fn has(name: &str) -> bool {
    find(name).is_some_and(|feature| feature.included)
}

/// The build features this build has, in the order of [`ALL`].
pub fn included() -> Vec<&'static str> {
    ALL.iter()
        .filter(|feature| feature.included)
        .map(|feature| feature.name)
        .collect()
}

/// The build features this build leaves out, in the order of [`ALL`]. Empty for the full build.
pub fn left_out() -> Vec<&'static str> {
    ALL.iter()
        .filter(|feature| !feature.included)
        .map(|feature| feature.name)
        .collect()
}

/// The build feature that runs a tool. iperf3 has none until the CLI runs it.
#[cfg_attr(not(feature = "console"), allow(dead_code))]
pub fn of_tool(tool: Tool) -> Option<&'static str> {
    match tool {
        Tool::Traceroute => Some("traceroute"),
        Tool::Ping => Some("ping"),
        Tool::Mtr => Some("mtr"),
        Tool::Dns => Some("dns"),
        Tool::Bgp => Some("bgp"),
        Tool::Iperf3 => None,
    }
}

/// Whether this build can run the tool. The Console offers only these.
#[cfg_attr(not(feature = "console"), allow(dead_code))]
pub fn runs(tool: Tool) -> bool {
    tool.is_available() && of_tool(tool).is_some_and(has)
}

/// The sentence for a command whose build feature this build leaves out.
pub fn missing(name: &str) -> String {
    let title = find(name).map_or(name, |feature| feature.title);
    format!("This build of rvrs leaves out {title}. rvrs update --with {name} adds it.")
}

/// Refuses a command whose build feature this build leaves out. Nothing was run, so the exit code is
/// the one for a run that couldn't be made.
pub fn refuse(name: &str) -> ExitCode {
    crate::fail(&missing(name))
}

/// Reads "bgp,console" or "bgp console" into names, in the order of [`ALL`]. A name this
/// version doesn't know is an error that lists the ones it does.
pub fn parse_list(text: &str) -> Result<Vec<&'static str>, String> {
    let mut names = Vec::new();
    for word in text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|word| !word.is_empty())
    {
        let word = word.to_ascii_lowercase();
        match find(&word) {
            Some(feature) => names.push(feature.name),
            None => {
                return Err(format!(
                    "rvrs has no build feature \u{201C}{word}\u{201D}. The build features are {}.",
                    and_list(&ALL.iter().map(|feature| feature.name).collect::<Vec<_>>())
                ))
            }
        }
    }
    if names.is_empty() {
        return Err(format!(
            "Name the build features, as in bgp,console. The build features are {}.",
            and_list(&ALL.iter().map(|feature| feature.name).collect::<Vec<_>>())
        ));
    }
    Ok(ordered(&names))
}

/// The names in the order of [`ALL`], each once, with every build feature whose needs are left out
/// added: a build without traceroute has no TCP probe either.
pub fn closed(left_out: &[&str]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = ordered(left_out);
    loop {
        let more: Vec<&'static str> = ALL
            .iter()
            .filter(|feature| !out.contains(&feature.name))
            .filter(|feature| feature.needs.iter().any(|need| out.contains(need)))
            .map(|feature| feature.name)
            .collect();
        if more.is_empty() {
            return ordered(&out);
        }
        out.extend(more);
    }
}

fn ordered(names: &[&str]) -> Vec<&'static str> {
    ALL.iter()
        .map(|feature| feature.name)
        .filter(|name| names.contains(name))
        .collect()
}

/// The build features a build should leave out once `with` is added back and `without` is left out, on
/// top of what `current` leaves out.
pub fn wanted(current: &[&str], with: &[&str], without: &[&str]) -> Vec<&'static str> {
    let mut out: Vec<&str> = current
        .iter()
        .copied()
        .filter(|name| !with.contains(name))
        .collect();
    out.extend(without.iter().copied());
    // Adding a build feature back adds back what it works inside.
    let needed: Vec<&str> = with
        .iter()
        .filter_map(|name| find(name))
        .flat_map(|feature| feature.needs.iter().copied())
        .collect();
    out.retain(|name| !needed.contains(name));
    closed(&out)
}

/// The release archive of the build for a target that leaves out these build features, as the
/// installer, `rvrs update` and `scripts/build.sh` name it: `rvrs-<target>.tar.gz` for the full
/// build, `rvrs-<target>-without-bgp-console.tar.gz` with the names sorted.
pub fn archive(target: &str, left_out: &[&str]) -> String {
    if left_out.is_empty() {
        return format!("rvrs-{target}.tar.gz");
    }
    let mut names: Vec<&str> = left_out.to_vec();
    names.sort_unstable();
    names.dedup();
    format!("rvrs-{target}-without-{}.tar.gz", names.join("-"))
}

/// The builds a release has for a target, read off its SHA256SUMS: what each leaves out, the
/// full build first.
pub fn builds_in(sums: &str, target: &str) -> Vec<Vec<String>> {
    let full = format!("rvrs-{target}.tar.gz");
    let prefix = format!("rvrs-{target}-without-");
    let mut builds: Vec<Vec<String>> = sums
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(|file| file.trim_start_matches('*'))
        .filter_map(|file| {
            if file == full {
                return Some(Vec::new());
            }
            let names = file.strip_prefix(&prefix)?.strip_suffix(".tar.gz")?;
            Some(names.split('-').map(str::to_string).collect())
        })
        .collect();
    builds.sort_by_key(Vec::len);
    builds
}

/// The builds a release has, in words: "the full build and the one without console".
pub fn builds_text(builds: &[Vec<String>]) -> String {
    let words: Vec<String> = builds
        .iter()
        .map(|left_out| {
            if left_out.is_empty() {
                "the full build".to_string()
            } else {
                format!("the one without {}", and_list(left_out))
            }
        })
        .collect();
    if words.is_empty() {
        "no build for this device".to_string()
    } else {
        and_list(&words)
    }
}

/// The features `cargo build` needs for a build that leaves out these, from the `default`
/// features of the crate being built, so a build feature added since is in.
pub fn cargo_features(default: &[String], left_out: &[&str]) -> Vec<String> {
    default
        .iter()
        .filter(|name| !left_out.contains(&name.as_str()))
        .cloned()
        .collect()
}

/// The `default` features in a Cargo.toml, from the one line `default = [...]`.
pub fn default_features(manifest: &str) -> Option<Vec<String>> {
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("default = ["))?;
    let inside = line.split_once('[')?.1.split_once(']')?.0;
    Some(
        inside
            .split(',')
            .map(|name| name.trim().trim_matches('"').to_string())
            .filter(|name| !name.is_empty())
            .collect(),
    )
}

/// "without console" for the version line, or nothing for the full build.
pub fn version_note() -> Option<String> {
    let out = left_out();
    (!out.is_empty()).then(|| format!("without {}", and_list(&out)))
}

/// "a", "a and b", "a, b and c".
pub fn and_list<S: AsRef<str>>(items: &[S]) -> String {
    match items {
        [] => String::new(),
        [one] => one.as_ref().to_string(),
        [rest @ .., last] => format!(
            "{} and {}",
            rest.iter()
                .map(|item| item.as_ref())
                .collect::<Vec<_>>()
                .join(", "),
            last.as_ref()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_build_feature_is_a_cargo_feature_in_the_default() {
        let manifest = include_str!("../Cargo.toml");
        let default = default_features(manifest).expect("a default line");
        let names: Vec<&str> = ALL.iter().map(|feature| feature.name).collect();
        assert_eq!(
            default, names,
            "Cargo.toml's default and ALL list the same build features"
        );
        for feature in &ALL {
            assert!(
                manifest.contains(&format!("\n{} = [", feature.name)),
                "{} is a feature",
                feature.name
            );
            // A name joins archive names with dashes and is typed in flags.
            assert!(feature
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        }
    }

    #[test]
    fn the_names_are_the_node_protocols() {
        // schema/node-protocol/README.md, the heartbeat's features. iperf3 joins when the CLI
        // runs it.
        let protocol = include_str!("../../../schema/node-protocol/README.md");
        let line = protocol
            .lines()
            .find(|line| line.starts_with("| `features` |"))
            .expect("the features row");
        for feature in &ALL {
            assert!(
                line.contains(&format!("`{}`", feature.name)),
                "{}",
                feature.name
            );
        }
    }

    #[test]
    fn the_installer_knows_the_same_build_features() {
        let installer = include_str!("../../../install.sh");
        let names: Vec<&str> = ALL.iter().map(|feature| feature.name).collect();
        assert!(
            installer.contains(&format!("\nfeatures=\"{}\"\n", names.join(" "))),
            "install.sh lists the build features of ALL"
        );
        let inside: Vec<&str> = ALL
            .iter()
            .filter(|feature| feature.needs == ["traceroute"])
            .map(|feature| feature.name)
            .collect();
        assert!(installer.contains(&format!("\ninside_traceroute=\"{}\"\n", inside.join(" "))));
        // No build feature needs another one but traceroute, which is all install.sh and
        // scripts/build.sh know how to close over.
        assert!(ALL
            .iter()
            .all(|feature| feature.needs.is_empty() || feature.needs == ["traceroute"]));
    }

    #[test]
    fn the_full_build_has_everything() {
        // The tests run on the default features.
        assert!(left_out().is_empty());
        assert_eq!(included().len(), ALL.len());
        assert_eq!(version_note(), None);
        assert!(runs(Tool::Bgp));
        assert!(!runs(Tool::Iperf3));
    }

    #[test]
    fn archives_are_named_by_what_they_leave_out() {
        let target = "x86_64-unknown-linux-musl";
        assert_eq!(
            archive(target, &[]),
            "rvrs-x86_64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(
            archive(target, &["console"]),
            "rvrs-x86_64-unknown-linux-musl-without-console.tar.gz"
        );
        assert_eq!(
            archive(target, &["console", "bgp"]),
            "rvrs-x86_64-unknown-linux-musl-without-bgp-console.tar.gz"
        );
        // sha256sum lists the files in name order, which puts the full build last.
        let sums = "cd  rvrs-x86_64-unknown-linux-musl-without-console.tar.gz\n\
                    ab  rvrs-x86_64-unknown-linux-musl.tar.gz\n\
                    ef  rvrs-aarch64-apple-darwin.tar.gz\n\
                    01  rvrs_0.14.0_amd64.deb\n";
        let builds = builds_in(sums, target);
        assert_eq!(
            builds,
            vec![Vec::<String>::new(), vec!["console".to_string()]]
        );
        assert_eq!(
            builds_text(&builds),
            "the full build and the one without console"
        );
        assert_eq!(builds_text(&[]), "no build for this device");
    }

    #[test]
    fn leaving_out_traceroute_leaves_out_what_works_inside_it() {
        assert_eq!(closed(&["traceroute"]), vec!["traceroute", "tcp", "mtu"]);
        assert_eq!(closed(&["console", "bgp"]), vec!["bgp", "console"]);
        assert_eq!(closed(&[]), Vec::<&str>::new());
    }

    #[test]
    fn with_and_without_change_what_an_update_leaves_out() {
        // A build from before build features leaves out nothing, and its update neither.
        assert_eq!(wanted(&[], &[], &[]), Vec::<&str>::new());
        assert_eq!(wanted(&["console"], &[], &[]), vec!["console"]);
        assert_eq!(wanted(&["console"], &["console"], &[]), Vec::<&str>::new());
        assert_eq!(
            wanted(&[], &[], &["console", "bgp"]),
            vec!["bgp", "console"]
        );
        // Adding the TCP probe back brings traceroute with it.
        assert_eq!(
            wanted(&["traceroute", "tcp", "mtu"], &["tcp"], &[]),
            vec!["mtu"]
        );
    }

    #[test]
    fn lists_of_build_features_are_read_and_checked() {
        assert_eq!(parse_list("console").unwrap(), vec!["console"]);
        assert_eq!(parse_list("console,BGP").unwrap(), vec!["bgp", "console"]);
        assert_eq!(parse_list("bgp console").unwrap(), vec!["bgp", "console"]);
        let error = parse_list("consol").unwrap_err();
        assert!(error.contains("\u{201C}consol\u{201D}"), "{error}");
        assert!(error.contains("mtu and console"), "{error}");
        assert!(parse_list(" , ").is_err());
    }

    #[test]
    fn cargo_gets_the_default_features_minus_what_is_left_out() {
        let manifest = "[features]\ndefault = [\"traceroute\", \"ping\", \"iperf3\", \"console\"]\ntraceroute = []\n";
        let default = default_features(manifest).unwrap();
        assert_eq!(default, vec!["traceroute", "ping", "iperf3", "console"]);
        // iperf3 is newer than this build, so it is in.
        assert_eq!(
            cargo_features(&default, &["console"]),
            vec!["traceroute", "ping", "iperf3"]
        );
        assert_eq!(default_features("[package]\nname = \"x\"\n"), None);
    }

    #[test]
    fn a_missing_build_feature_names_the_way_back() {
        assert_eq!(
            missing("bgp"),
            "This build of rvrs leaves out BGP. rvrs update --with bgp adds it."
        );
        assert_eq!(
            missing("console"),
            "This build of rvrs leaves out the Console. rvrs update --with console adds it."
        );
        assert_eq!(and_list(&["a", "b", "c"]), "a, b and c");
    }
}
