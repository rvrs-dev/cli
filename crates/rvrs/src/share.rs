//! Share links: `rvrs share [id]` makes a run's share link, a public page on the web app that
//! anyone with the link opens without signing in, and `rvrs share --stop [id]` ends it.
//!
//! A share shows the run as `sharedReport` in the app repo makes it, by the rule in
//! `schema/README.md` (What points to a home): the private network the run started in unless
//! Private Networks is turned off, and the line's public address and the install's name only
//! when the person keeps them in. The API stores what the link shows at the moment of sharing.
//! A run has one live link: sharing it again with other options keeps the link.

use std::io::IsTerminal;
use std::process::ExitCode;

use rvrs_core::model::Run;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::account;
use crate::style::Style;

/// What a share shows of a run's home details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shows {
    /// The private network the run started in: the router, its name, the source address.
    pub private_networks: bool,
    /// The line's public address and what gives it away, the origin's location among it.
    pub public_address: bool,
    /// The name of the install that made the run.
    pub install_name: bool,
}

impl Default for Shows {
    /// Private Networks on, the public address and the install's name left out.
    fn default() -> Self {
        Self {
            private_networks: true,
            public_address: false,
            install_name: false,
        }
    }
}

/// A share link as its owner reads it: `POST /v1/shares` and `GET /v1/shares` answer these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub id: String,
    /// The link to give out.
    pub url: String,
    /// The run's id in the account.
    pub run: String,
    pub shows: Shows,
    pub created_at: String,
}

/// The body of `POST /v1/shares`. Every option is sent, so the link shows what this install
/// says and never a default the API might change.
pub fn body(run: &str, shows: Shows) -> Value {
    json!({
        "run": run,
        "privateNetworks": shows.private_networks,
        "publicAddress": shows.public_address,
        "installName": shows.install_name,
    })
}

fn refused(status: u16, body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("The rvrs API answered {status}."))
}

/// Makes the run's share link, or changes what its live link shows. A run the account doesn't
/// have yet is sent first. The error is a sentence to show.
pub fn make(run: &Run, shows: Shows) -> Result<Share, String> {
    let account_id = crate::sync::account_id(run)?;
    let (status, answer) =
        account::call("POST", "/v1/shares", Some(&body(&account_id, shows)), 20)?;
    match status {
        200 | 201 => serde_json::from_value(answer).map_err(|_| {
            "The rvrs API answered without a share link. Update rvrs and try again.".to_string()
        }),
        _ => Err(refused(status, &answer)),
    }
}

/// Ends one share link. A link that has ended already counts as ended.
pub fn end(share_id: &str) -> Result<(), String> {
    let (status, answer) = account::call("DELETE", &format!("/v1/shares/{share_id}"), None, 20)?;
    match status {
        204 | 404 => Ok(()),
        _ => Err(refused(status, &answer)),
    }
}

/// Ends the run's live link. The answer is the links that ended: none when the run had none.
pub fn stop(run: &Run) -> Result<Vec<String>, String> {
    let Some(account_id) = crate::sync::known_account_id(run)? else {
        // The account doesn't have the run, so it has no link.
        return Ok(Vec::new());
    };
    let (status, answer) = account::call("GET", &format!("/v1/shares?run={account_id}"), None, 20)?;
    if status != 200 {
        return Err(refused(status, &answer));
    }
    let shares: Vec<Share> = serde_json::from_value(answer["shares"].clone()).unwrap_or_default();
    let mut ended = Vec::new();
    for share in shares {
        end(&share.id)?;
        ended.push(share.url);
    }
    Ok(ended)
}

/// What the link shows and what it leaves out: "It shows the private network. It leaves out the
/// public address and the install's name."
pub fn what_it_shows(shows: Shows) -> String {
    let parts = [
        (shows.private_networks, "the private network"),
        (shows.public_address, "the public address"),
        (shows.install_name, "the install\u{2019}s name"),
    ];
    let list = |wanted: bool| {
        let names: Vec<&str> = parts
            .iter()
            .filter(|(on, _)| *on == wanted)
            .map(|(_, name)| *name)
            .collect();
        match names.as_slice() {
            [] => String::new(),
            [one] => (*one).to_string(),
            [first, last] => format!("{first} and {last}"),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        }
    };
    match (list(true), list(false)) {
        (shown, left) if left.is_empty() => format!("It shows {shown}."),
        (shown, left) if shown.is_empty() => format!("It leaves out {left}."),
        (shown, left) => format!("It shows {shown}. It leaves out {left}."),
    }
}

/// What `rvrs share` was asked.
#[derive(Debug, Default, PartialEq)]
struct Asked {
    id: Option<String>,
    shows: Shows,
    stop: bool,
    json: bool,
}

fn parse(arguments: &[String]) -> Result<Asked, String> {
    let mut asked = Asked::default();
    let mut options = false;
    for argument in arguments {
        match argument.as_str() {
            "--no-private-networks" => {
                asked.shows.private_networks = false;
                options = true;
            }
            "--public-address" => {
                asked.shows.public_address = true;
                options = true;
            }
            "--install-name" => {
                asked.shows.install_name = true;
                options = true;
            }
            "--stop" => asked.stop = true,
            "--json" => asked.json = true,
            flag if flag.starts_with('-') => {
                return Err(format!(
                    "rvrs share has no flag {flag}. It takes --no-private-networks, --public-address, --install-name, --stop and --json."
                ))
            }
            id if asked.id.is_none() => asked.id = Some(id.to_string()),
            _ => return Err("rvrs share takes one run id, or none for the newest.".into()),
        }
    }
    if asked.stop && options {
        return Err(
            "rvrs share --stop ends the link and takes no option for what it shows.".into(),
        );
    }
    Ok(asked)
}

/// `rvrs share [id]`: the share link of the newest run, or the one named, on stdout. What the
/// link shows goes to stderr, so `rvrs share | pbcopy` copies the link alone.
pub fn command(arguments: &[String]) -> ExitCode {
    let asked = match parse(arguments) {
        Ok(asked) => asked,
        Err(message) => return crate::usage(&message),
    };
    if let Err(message) = crate::sync::can_share() {
        return crate::fail(&message);
    }
    let run = match crate::commands::pick_run(asked.id.as_deref()) {
        Ok(run) => run,
        Err(code) => return code,
    };
    if asked.stop {
        return match stop(&run) {
            Ok(ended) if ended.is_empty() => {
                println!("{} has no share link.", crate::commands::run_name(&run));
                ExitCode::SUCCESS
            }
            Ok(ended) => {
                println!(
                    "Stopped sharing {}. {} no longer opens.",
                    crate::commands::run_name(&run),
                    ended.join(", ")
                );
                ExitCode::SUCCESS
            }
            Err(message) => crate::fail(&message),
        };
    }
    let share = match make(&run, asked.shows) {
        Ok(share) => share,
        Err(message) => return crate::fail(&message),
    };
    if asked.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&share).unwrap_or_default()
        );
        return ExitCode::SUCCESS;
    }
    println!("{}", share.url);
    let style = Style::detect_stderr();
    let sentence = format!(
        "Anyone with the link can open {} without signing in. {}",
        crate::commands::run_name(&run),
        what_it_shows(share.shows)
    );
    if std::io::stderr().is_terminal() {
        eprintln!("{}", style.dim(&sentence));
    } else {
        eprintln!("{sentence}");
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_share_shows_the_private_network_and_nothing_else_by_default() {
        assert_eq!(
            parse(&[]).unwrap(),
            Asked {
                shows: Shows {
                    private_networks: true,
                    public_address: false,
                    install_name: false
                },
                ..Asked::default()
            }
        );
    }

    #[test]
    fn each_flag_turns_one_option() {
        let asked = parse(&words(
            "abc234 --no-private-networks --public-address --install-name --json",
        ))
        .unwrap();
        assert_eq!(asked.id.as_deref(), Some("abc234"));
        assert_eq!(
            asked.shows,
            Shows {
                private_networks: false,
                public_address: true,
                install_name: true
            }
        );
        assert!(asked.json && !asked.stop);
    }

    #[test]
    fn stop_takes_an_id_and_no_option() {
        let asked = parse(&words("--stop abc234")).unwrap();
        assert!(asked.stop);
        assert_eq!(asked.id.as_deref(), Some("abc234"));
        assert!(parse(&words("--stop --public-address")).is_err());
    }

    #[test]
    fn a_wrong_flag_or_a_second_id_is_refused() {
        assert_eq!(
            parse(&words("--public")).unwrap_err(),
            "rvrs share has no flag --public. It takes --no-private-networks, --public-address, --install-name, --stop and --json."
        );
        assert!(parse(&words("abc234 def567")).is_err());
    }

    /// The API's fixture in the app repo (`packages/schema/fixtures/share.json`).
    const FIXTURE: &str = r#"{
      "input": {
        "run": "4f6c8f0e-2b7d-4c55-9a8e-1d2f3a4b5c6d",
        "privateNetworks": true,
        "publicAddress": false
      },
      "share": {
        "id": "q3Lr8XwT2a",
        "url": "https://app.rvrs.dev/r/q3Lr8XwT2a",
        "run": "4f6c8f0e-2b7d-4c55-9a8e-1d2f3a4b5c6d",
        "shows": { "privateNetworks": true, "publicAddress": false, "installName": false },
        "createdAt": "2026-11-23T20:15:04.000Z"
      }
    }"#;

    #[test]
    fn the_body_names_every_option() {
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        let sent = body("4f6c8f0e-2b7d-4c55-9a8e-1d2f3a4b5c6d", Shows::default());
        for key in ["run", "privateNetworks", "publicAddress"] {
            assert_eq!(sent[key], fixture["input"][key], "{key}");
        }
        assert_eq!(sent["installName"], false);
    }

    #[test]
    fn a_share_reads_from_the_apis_answer_and_back() {
        let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
        let share: Share = serde_json::from_value(fixture["share"].clone()).unwrap();
        assert_eq!(share.id, "q3Lr8XwT2a");
        assert_eq!(share.url, "https://app.rvrs.dev/r/q3Lr8XwT2a");
        assert_eq!(share.run, "4f6c8f0e-2b7d-4c55-9a8e-1d2f3a4b5c6d");
        assert_eq!(share.shows, Shows::default());
        assert_eq!(share.created_at, "2026-11-23T20:15:04.000Z");
        assert_eq!(serde_json::to_value(&share).unwrap(), fixture["share"]);
    }

    #[test]
    fn the_sentence_says_what_the_link_shows_and_leaves_out() {
        assert_eq!(
            what_it_shows(Shows::default()),
            "It shows the private network. It leaves out the public address and the install\u{2019}s name."
        );
        assert_eq!(
            what_it_shows(Shows {
                private_networks: true,
                public_address: true,
                install_name: true
            }),
            "It shows the private network, the public address and the install\u{2019}s name."
        );
        assert_eq!(
            what_it_shows(Shows {
                private_networks: false,
                public_address: false,
                install_name: false
            }),
            "It leaves out the private network, the public address and the install\u{2019}s name."
        );
    }
}
