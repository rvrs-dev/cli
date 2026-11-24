//! Share links: `rvrs share [id]` makes a run's share link, a public page on the web app that
//! anyone with the link opens without signing in, and `rvrs share --stop [id]` ends it.
//!
//! A share shows the run as `sharedReport` in the app repo makes it, by the rule in
//! `schema/README.md` (What points to a home): the private network the run started in unless
//! Private Networks is turned off, and the line's public address and the install's name only
//! when the person keeps them in. The API stores what the link shows at the moment of sharing.
//! A run has one live link: sharing it again with other options keeps the link.
//!
//! A workspace has rules for its links, which its owners and admins set on the web: the
//! switches a new share starts with, how many days a new link works, and whether every member
//! makes links or only owners and admins. `GET /v1/shares?run=` answers them with the run's
//! live link, and every surface starts from them, as the web's Share does. An API from before
//! the rules answers the links alone, and then the rules are the ones every workspace started
//! with: Private Networks on, the rest off, no expiry, and every member shares.

use std::io::IsTerminal;
use std::process::ExitCode;

use rvrs_core::format;
use rvrs_core::model::Run;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::account;
use crate::style::Style;

/// What the API says to a member who may not make a link, and what rvrs says before it asks.
pub const NOT_ALLOWED: &str = "Only owners and admins of this workspace share runs.";

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
    /// Private Networks on, the public address and the install's name left out: what a
    /// workspace starts with until an owner or an admin changes it.
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
    /// When the link stops working, or None for a link that works until its owner stops it.
    /// An API from before expiry sends none, and `--json` prints null for it.
    #[serde(default)]
    pub expires_at: Option<String>,
}

/// A workspace's rules for a new share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// The switches a new share starts with.
    pub defaults: Shows,
    /// Days a new link works, or None when links don't expire.
    pub expires_in_days: Option<u32>,
    /// Whether this account may make a link or change what one shows. False for a member while
    /// only owners and admins share. Stopping a link is always allowed.
    pub can_share: bool,
}

impl Default for Rules {
    /// What every workspace started with, and what an API from before the rules means.
    fn default() -> Self {
        Self {
            defaults: Shows::default(),
            expires_in_days: None,
            can_share: true,
        }
    }
}

/// Where a run's share stands: its live link, if it has one, and the rules of the workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct Standing {
    pub link: Option<Share>,
    pub rules: Rules,
}

impl Standing {
    /// The answer of `GET /v1/shares`, for the run with this id in the account. Each new field
    /// is read as optional, so an older API's answer stands for the rules every workspace
    /// started with.
    pub fn of(answer: &Value, run: Option<&str>) -> Standing {
        let link = run.and_then(|run| {
            answer["shares"]
                .as_array()?
                .iter()
                .filter_map(|share| serde_json::from_value::<Share>(share.clone()).ok())
                .find(|share| share.run == run)
        });
        let rules = Rules {
            defaults: serde_json::from_value(answer["defaults"].clone()).unwrap_or_default(),
            expires_in_days: answer["expiresInDays"]
                .as_u64()
                .and_then(|days| u32::try_from(days).ok())
                .filter(|days| *days > 0),
            can_share: answer["canShare"].as_bool().unwrap_or(true),
        };
        Standing { link, rules }
    }

    /// What the switches start at: what the live link shows, or the workspace's defaults for
    /// a new link. So sharing a shared run again without a flag answers the same link.
    pub fn start(&self) -> Shows {
        self.link
            .as_ref()
            .map_or(self.rules.defaults, |link| link.shows)
    }
}

/// The switches a person set with a flag. One they didn't set stays where it starts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Chosen {
    pub private_networks: Option<bool>,
    pub public_address: Option<bool>,
    pub install_name: Option<bool>,
}

impl Chosen {
    /// The switches from `start`, with each one the person set changed to what they said.
    pub fn over(self, start: Shows) -> Shows {
        Shows {
            private_networks: self.private_networks.unwrap_or(start.private_networks),
            public_address: self.public_address.unwrap_or(start.public_address),
            install_name: self.install_name.unwrap_or(start.install_name),
        }
    }

    fn any(self) -> bool {
        self != Chosen::default()
    }
}

/// What sharing a run comes to, before anything is sent.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// The run's live link shows this already. Nothing to send.
    Keep(Share),
    /// Make the link, or change what the live link shows to this.
    Send(Shows),
    /// Only owners and admins make or change links in this workspace, and the account is
    /// neither.
    NotAllowed,
}

/// What sharing the run with these flags comes to.
pub fn plan(standing: &Standing, chosen: Chosen) -> Plan {
    let shows = chosen.over(standing.start());
    match &standing.link {
        Some(link) if link.shows == shows => Plan::Keep(link.clone()),
        _ if !standing.rules.can_share => Plan::NotAllowed,
        _ => Plan::Send(shows),
    }
}

/// The body of `POST /v1/shares`. Every option is sent, worked out from the workspace's
/// defaults or the live link and the person's flags, so the link shows what rvrs said it would.
pub fn body(run: &str, shows: Shows) -> Value {
    json!({
        "run": run,
        "privateNetworks": shows.private_networks,
        "publicAddress": shows.public_address,
        "installName": shows.install_name,
    })
}

/// One request to the API as this install: the method, the path and a body, then the status
/// and the body of the answer. The binary asks the API through `account::call`, and the tests
/// ask a stand-in.
pub type Api<'a> = &'a dyn Fn(&str, &str, Option<&Value>) -> Result<(u16, Value), String>;

fn api(method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value), String> {
    account::call(method, path, body, 20)
}

fn refused(status: u16, body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("The rvrs API answered {status}."))
}

/// Reads where the share of the run with this id in the account stands. None for a run the
/// account doesn't have yet: it has no link, and the answer still holds the workspace's rules.
fn read(api: Api, run: Option<&str>) -> Result<Standing, String> {
    let path = match run {
        Some(run) => format!("/v1/shares?run={run}"),
        None => "/v1/shares".to_string(),
    };
    let (status, answer) = api("GET", &path, None)?;
    if status != 200 {
        return Err(refused(status, &answer));
    }
    Ok(Standing::of(&answer, run))
}

/// Asks the API for the link of the run with this id in the account, showing this.
fn send(api: Api, run: &str, shows: Shows) -> Result<Share, String> {
    let (status, answer) = api("POST", "/v1/shares", Some(&body(run, shows)))?;
    match status {
        200 | 201 => serde_json::from_value(answer).map_err(|_| {
            "The rvrs API answered without a share link. Update rvrs and try again.".to_string()
        }),
        403 if answer["error"]["code"] == "FORBIDDEN" && answer["error"]["message"].is_null() => {
            Err(NOT_ALLOWED.to_string())
        }
        _ => Err(refused(status, &answer)),
    }
}

/// Shares the run with this id in the account by the workspace's rules and the person's flags:
/// the live link as it is when it shows that already, a new or changed link when the account
/// may make one, and `NOT_ALLOWED` otherwise.
fn share_run(api: Api, run: &str, chosen: Chosen) -> Result<Share, String> {
    match plan(&read(api, Some(run))?, chosen) {
        Plan::Keep(link) => Ok(link),
        Plan::Send(shows) => send(api, run, shows),
        Plan::NotAllowed => Err(NOT_ALLOWED.to_string()),
    }
}

/// Where the run's share stands, for the Console's sheet. Asks the API and sends nothing: a
/// run the account doesn't have yet has no link.
#[cfg_attr(not(feature = "console"), allow(dead_code))]
pub fn standing(run: &Run) -> Result<Standing, String> {
    let known = crate::sync::known_account_id(run)?;
    read(&api, known.as_deref())
}

/// Makes the run's share link, or changes what its live link shows. A run the account doesn't
/// have yet is sent first. The error is a sentence to show.
#[cfg_attr(not(feature = "console"), allow(dead_code))]
pub fn make(run: &Run, shows: Shows) -> Result<Share, String> {
    let account_id = crate::sync::account_id(run)?;
    send(&api, &account_id, shows)
}

fn end_with(api: Api, share_id: &str) -> Result<(), String> {
    let (status, answer) = api("DELETE", &format!("/v1/shares/{share_id}"), None)?;
    match status {
        204 | 404 => Ok(()),
        _ => Err(refused(status, &answer)),
    }
}

/// Ends one share link. A link that has ended already counts as ended.
#[cfg_attr(not(feature = "console"), allow(dead_code))]
pub fn end(share_id: &str) -> Result<(), String> {
    end_with(&api, share_id)
}

/// Ends the live link of the run with this id in the account. The answer is the links that
/// ended: none when the run had none. A member who may not make links still ends their own.
fn stop_with(api: Api, run: Option<&str>) -> Result<Vec<String>, String> {
    let Some(run) = run else {
        // The account doesn't have the run, so it has no link.
        return Ok(Vec::new());
    };
    let (status, answer) = api("GET", &format!("/v1/shares?run={run}"), None)?;
    if status != 200 {
        return Err(refused(status, &answer));
    }
    let shares: Vec<Share> = answer["shares"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|share| serde_json::from_value(share.clone()).ok())
        .collect();
    let mut ended = Vec::new();
    for share in shares {
        end_with(api, &share.id)?;
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

/// Before a link is made, in a workspace whose links expire: "The link stops working after 30
/// days." The web's Share says the same.
#[cfg_attr(not(feature = "console"), allow(dead_code))]
pub fn expires_after(days: u32) -> String {
    match days {
        1 => "The link stops working after 1 day.".to_string(),
        _ => format!("The link stops working after {days} days."),
    }
}

/// For a link that expires: "The link stops working on Dec 24, 2026.", the day in this
/// device's time zone, as the web's Share writes it in the browser's.
pub fn expires_on(moment: &str) -> Option<String> {
    let (year, month, day) = local_day(format::seconds_of(moment)?)?;
    Some(format!(
        "The link stops working on {}.",
        date_words(year, month, day)
    ))
}

/// "Dec 24, 2026"
fn date_words(year: i64, month: u32, day: u32) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let name = MONTHS[(month.clamp(1, 12) - 1) as usize];
    format!("{name} {day}, {year}")
}

/// The year, the month (1 to 12) and the day of a moment in this device's time zone.
fn local_day(seconds: u64) -> Option<(i64, u32, u32)> {
    let moment = libc::time_t::try_from(seconds).ok()?;
    // SAFETY: tm is plain data, and localtime_r fills the one passed to it or returns null.
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&moment, &mut parts) }.is_null() {
        return None;
    }
    Some((
        i64::from(parts.tm_year) + 1900,
        u32::try_from(parts.tm_mon + 1).ok()?,
        u32::try_from(parts.tm_mday).ok()?,
    ))
}

/// What `rvrs share` was asked.
#[derive(Debug, Default, PartialEq)]
struct Asked {
    id: Option<String>,
    chosen: Chosen,
    stop: bool,
    json: bool,
}

const FLAGS: &str = "--private-networks, --no-private-networks, --public-address, --no-public-address, --install-name, --no-install-name, --stop and --json";

/// Sets one switch from one of its two flags. Giving both is refused, since one of them would
/// be ignored.
fn set(slot: &mut Option<bool>, on: bool, yes: &str, no: &str) -> Result<(), String> {
    match slot {
        Some(was) if *was != on => Err(format!("{yes} and {no} can\u{2019}t both be given.")),
        _ => {
            *slot = Some(on);
            Ok(())
        }
    }
}

fn parse(arguments: &[String]) -> Result<Asked, String> {
    let mut asked = Asked::default();
    for argument in arguments {
        let on = !argument.starts_with("--no-");
        match argument.as_str() {
            "--private-networks" | "--no-private-networks" => set(
                &mut asked.chosen.private_networks,
                on,
                "--private-networks",
                "--no-private-networks",
            )?,
            "--public-address" | "--no-public-address" => set(
                &mut asked.chosen.public_address,
                on,
                "--public-address",
                "--no-public-address",
            )?,
            "--install-name" | "--no-install-name" => set(
                &mut asked.chosen.install_name,
                on,
                "--install-name",
                "--no-install-name",
            )?,
            "--stop" => asked.stop = true,
            "--json" => asked.json = true,
            flag if flag.starts_with('-') => {
                return Err(format!("rvrs share has no flag {flag}. It takes {FLAGS}."))
            }
            id if asked.id.is_none() => asked.id = Some(id.to_string()),
            _ => return Err("rvrs share takes one run id, or none for the newest.".into()),
        }
    }
    if asked.stop && asked.chosen.any() {
        return Err(
            "rvrs share --stop ends the link and takes no option for what it shows.".into(),
        );
    }
    Ok(asked)
}

/// The sentence `rvrs share` writes to stderr under the link: who opens it, what it shows and,
/// for a link that expires, the day it stops working.
fn sentence(run: &Run, share: &Share) -> String {
    let mut sentence = format!(
        "Anyone with the link can open {} without signing in. {}",
        crate::commands::run_name(run),
        what_it_shows(share.shows)
    );
    if let Some(expiry) = share.expires_at.as_deref().and_then(expires_on) {
        sentence.push(' ');
        sentence.push_str(&expiry);
    }
    sentence
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
        let ended =
            crate::sync::known_account_id(&run).and_then(|known| stop_with(&api, known.as_deref()));
        return match ended {
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
    let shared = crate::sync::account_id(&run)
        .and_then(|account_id| share_run(&api, &account_id, asked.chosen));
    let share = match shared {
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
    let sentence = sentence(&run, &share);
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
    use std::cell::RefCell;
    use std::collections::VecDeque;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    const RUN: &str = "4f6c8f0e-2b7d-4c55-9a8e-1d2f3a4b5c6d";

    /// The API's fixture in the app repo (`packages/schema/fixtures/share.json`), with the
    /// workspace's rules since rvrs-dev/app#14.
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
        "createdAt": "2026-11-23T20:15:04.000Z",
        "expiresAt": null
      },
      "shares": {
        "defaults": { "privateNetworks": true, "publicAddress": false, "installName": false },
        "expiresInDays": 30,
        "canShare": true
      }
    }"#;

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).unwrap()
    }

    fn shows(private_networks: bool, public_address: bool, install_name: bool) -> Shows {
        Shows {
            private_networks,
            public_address,
            install_name,
        }
    }

    /// The fixture's share, showing this, expiring then.
    fn link(shown: Shows, expires_at: Option<&str>) -> Value {
        let mut share = fixture()["share"].clone();
        share["shows"] = serde_json::to_value(shown).unwrap();
        share["expiresAt"] = expires_at.map_or(Value::Null, Value::from);
        share
    }

    /// `GET /v1/shares?run=` as rvrs-dev/app#14 answers it.
    fn listing(links: Vec<Value>, defaults: Shows, days: Option<u32>, can_share: bool) -> Value {
        json!({
            "shares": links,
            "defaults": defaults,
            "expiresInDays": days,
            "canShare": can_share,
        })
    }

    /// The API, as far as share.rs asks it: each call is kept, and each answer is the next one
    /// a test lined up.
    #[derive(Default)]
    struct StandIn {
        calls: RefCell<Vec<(String, String, Option<Value>)>>,
        answers: RefCell<VecDeque<(u16, Value)>>,
    }

    impl StandIn {
        fn answering(answers: Vec<(u16, Value)>) -> StandIn {
            StandIn {
                answers: RefCell::new(answers.into()),
                ..StandIn::default()
            }
        }

        fn call(
            &self,
            method: &str,
            path: &str,
            body: Option<&Value>,
        ) -> Result<(u16, Value), String> {
            self.calls
                .borrow_mut()
                .push((method.to_string(), path.to_string(), body.cloned()));
            self.answers
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| format!("No answer lined up for {method} {path}."))
        }

        fn calls(&self) -> Vec<(String, String, Option<Value>)> {
            self.calls.borrow().clone()
        }

        /// `share_run` for the run of the fixture, asking this stand-in.
        fn share(&self, chosen: Chosen) -> Result<Share, String> {
            share_run(
                &|m: &str, p: &str, b: Option<&Value>| self.call(m, p, b),
                RUN,
                chosen,
            )
        }

        fn send(&self, shows: Shows) -> Result<Share, String> {
            send(
                &|m: &str, p: &str, b: Option<&Value>| self.call(m, p, b),
                RUN,
                shows,
            )
        }

        fn stop(&self, run: Option<&str>) -> Result<Vec<String>, String> {
            stop_with(
                &|m: &str, p: &str, b: Option<&Value>| self.call(m, p, b),
                run,
            )
        }
    }

    #[test]
    fn no_flag_leaves_every_switch_where_it_starts() {
        assert_eq!(parse(&[]).unwrap(), Asked::default());
        assert!(!Asked::default().chosen.any());
    }

    #[test]
    fn each_flag_sets_one_switch() {
        let asked = parse(&words(
            "abc234 --no-private-networks --public-address --install-name --json",
        ))
        .unwrap();
        assert_eq!(asked.id.as_deref(), Some("abc234"));
        assert_eq!(
            asked.chosen,
            Chosen {
                private_networks: Some(false),
                public_address: Some(true),
                install_name: Some(true)
            }
        );
        assert!(asked.json && !asked.stop);
        let asked = parse(&words(
            "--private-networks --no-public-address --no-install-name",
        ))
        .unwrap();
        assert_eq!(
            asked.chosen,
            Chosen {
                private_networks: Some(true),
                public_address: Some(false),
                install_name: Some(false)
            }
        );
    }

    #[test]
    fn a_switch_and_its_opposite_are_refused_together() {
        assert_eq!(
            parse(&words("--public-address --no-public-address")).unwrap_err(),
            "--public-address and --no-public-address can\u{2019}t both be given."
        );
        assert!(parse(&words("--no-install-name --install-name")).is_err());
        // The same flag twice says one thing.
        assert!(parse(&words("--public-address --public-address")).is_ok());
    }

    #[test]
    fn stop_takes_an_id_and_no_option() {
        let asked = parse(&words("--stop abc234")).unwrap();
        assert!(asked.stop);
        assert_eq!(asked.id.as_deref(), Some("abc234"));
        assert!(parse(&words("--stop --public-address")).is_err());
        assert!(parse(&words("--stop --no-public-address")).is_err());
    }

    #[test]
    fn a_wrong_flag_or_a_second_id_is_refused() {
        assert_eq!(
            parse(&words("--public")).unwrap_err(),
            "rvrs share has no flag --public. It takes --private-networks, --no-private-networks, --public-address, --no-public-address, --install-name, --no-install-name, --stop and --json."
        );
        assert!(parse(&words("abc234 def567")).is_err());
    }

    #[test]
    fn the_body_names_every_option() {
        let sent = body(RUN, Shows::default());
        for key in ["run", "privateNetworks", "publicAddress"] {
            assert_eq!(sent[key], fixture()["input"][key], "{key}");
        }
        assert_eq!(sent["installName"], false);
    }

    #[test]
    fn a_share_reads_from_the_apis_answer_and_back() {
        let share: Share = serde_json::from_value(fixture()["share"].clone()).unwrap();
        assert_eq!(share.id, "q3Lr8XwT2a");
        assert_eq!(share.url, "https://app.rvrs.dev/r/q3Lr8XwT2a");
        assert_eq!(share.run, RUN);
        assert_eq!(share.shows, Shows::default());
        assert_eq!(share.created_at, "2026-11-23T20:15:04.000Z");
        assert_eq!(share.expires_at, None);
        assert_eq!(serde_json::to_value(&share).unwrap(), fixture()["share"]);
    }

    #[test]
    fn json_output_gains_expires_at_and_keeps_the_rest() {
        // An API from before expiry: the share reads, and --json prints the field as null.
        let mut older = fixture()["share"].clone();
        older.as_object_mut().unwrap().remove("expiresAt");
        let share: Share = serde_json::from_value(older.clone()).unwrap();
        assert_eq!(share.expires_at, None);
        let printed = serde_json::to_value(&share).unwrap();
        assert_eq!(printed["expiresAt"], Value::Null);
        for key in ["id", "url", "run", "shows", "createdAt"] {
            assert_eq!(printed[key], older[key], "{key}");
        }
        // A link that expires prints when.
        let expiring: Share =
            serde_json::from_value(link(Shows::default(), Some("2026-12-24T12:00:00.000Z")))
                .unwrap();
        assert_eq!(
            serde_json::to_value(&expiring).unwrap()["expiresAt"],
            "2026-12-24T12:00:00.000Z"
        );
    }

    #[test]
    fn the_rules_read_from_the_apis_answer() {
        let answer = {
            let mut answer = fixture()["shares"].clone();
            answer["shares"] = json!([link(Shows::default(), None)]);
            answer
        };
        let standing = Standing::of(&answer, Some(RUN));
        assert_eq!(
            standing.rules,
            Rules {
                defaults: Shows::default(),
                expires_in_days: Some(30),
                can_share: true
            }
        );
        assert_eq!(standing.link.unwrap().id, "q3Lr8XwT2a");
        // A run the account doesn't have yet has no link, whatever the answer lists.
        assert_eq!(Standing::of(&answer, None).link, None);
        // A listing of another run's link is no link of this one.
        assert_eq!(
            Standing::of(&answer, Some("0e6d2f0a-8c0e-4f58-9a55-0b8f3f6f1c11")).link,
            None
        );
    }

    #[test]
    fn an_older_apis_answer_stands_for_the_rules_every_workspace_started_with() {
        let standing = Standing::of(&json!({ "shares": [] }), Some(RUN));
        assert_eq!(standing.rules, Rules::default());
        assert_eq!(standing.start(), shows(true, false, false));
        assert!(standing.rules.can_share);
        assert_eq!(standing.rules.expires_in_days, None);
        // Links that don't expire answer null.
        let never = Standing::of(&listing(vec![], Shows::default(), None, true), Some(RUN));
        assert_eq!(never.rules.expires_in_days, None);
    }

    #[test]
    fn the_switches_start_at_the_workspaces_defaults_and_a_flag_wins() {
        let standing = Standing::of(
            &listing(vec![], shows(false, true, true), None, true),
            Some(RUN),
        );
        assert_eq!(
            plan(&standing, Chosen::default()),
            Plan::Send(shows(false, true, true))
        );
        let chosen = Chosen {
            public_address: Some(false),
            ..Chosen::default()
        };
        assert_eq!(
            plan(&standing, chosen),
            Plan::Send(shows(false, false, true))
        );
    }

    #[test]
    fn a_shared_run_starts_from_what_its_link_shows() {
        let standing = Standing::of(
            &listing(
                vec![link(shows(true, true, false), None)],
                Shows::default(),
                None,
                true,
            ),
            Some(RUN),
        );
        // Shared again without a flag, it is the same link, and nothing is sent.
        let Plan::Keep(kept) = plan(&standing, Chosen::default()) else {
            panic!("the link shows this already");
        };
        assert_eq!(kept.shows, shows(true, true, false));
        // A flag changes what it shows, and the rest stays as the link has it.
        let chosen = Chosen {
            install_name: Some(true),
            ..Chosen::default()
        };
        assert_eq!(plan(&standing, chosen), Plan::Send(shows(true, true, true)));
    }

    #[test]
    fn a_member_who_may_not_share_makes_no_link_and_keeps_the_one_they_have() {
        let none = Standing::of(
            &listing(vec![], Shows::default(), Some(30), false),
            Some(RUN),
        );
        assert_eq!(plan(&none, Chosen::default()), Plan::NotAllowed);
        let held = Standing::of(
            &listing(
                vec![link(Shows::default(), None)],
                Shows::default(),
                None,
                false,
            ),
            Some(RUN),
        );
        assert!(matches!(plan(&held, Chosen::default()), Plan::Keep(_)));
        let chosen = Chosen {
            public_address: Some(true),
            ..Chosen::default()
        };
        assert_eq!(plan(&held, chosen), Plan::NotAllowed);
    }

    #[test]
    fn a_share_sends_the_workspaces_defaults_with_the_flags() {
        let made = link(shows(false, true, false), Some("2026-12-23T20:15:04.000Z"));
        let api = StandIn::answering(vec![
            (
                200,
                listing(vec![], shows(false, true, false), Some(30), true),
            ),
            (201, made),
        ]);
        let share = api.share(Chosen::default()).unwrap();
        assert_eq!(
            share.expires_at.as_deref(),
            Some("2026-12-23T20:15:04.000Z")
        );
        let calls = api.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(
            (calls[0].0.as_str(), calls[0].1.as_str()),
            ("GET", &*format!("/v1/shares?run={RUN}"))
        );
        assert_eq!(
            (calls[1].0.as_str(), calls[1].1.as_str()),
            ("POST", "/v1/shares")
        );
        assert_eq!(
            calls[1].2.as_ref().unwrap(),
            &json!({ "run": RUN, "privateNetworks": false, "publicAddress": true, "installName": false })
        );
    }

    #[test]
    fn an_older_api_gets_the_fixed_defaults() {
        let api = StandIn::answering(vec![
            (200, json!({ "shares": [] })),
            (201, fixture()["share"].clone()),
        ]);
        let chosen = Chosen {
            install_name: Some(true),
            ..Chosen::default()
        };
        api.share(chosen).unwrap();
        assert_eq!(
            api.calls()[1].2.as_ref().unwrap(),
            &json!({ "run": RUN, "privateNetworks": true, "publicAddress": false, "installName": true })
        );
    }

    #[test]
    fn a_member_who_may_not_share_is_told_and_nothing_is_sent() {
        let api = StandIn::answering(vec![(200, listing(vec![], Shows::default(), None, false))]);
        let refused = api.share(Chosen::default());
        assert_eq!(refused.unwrap_err(), NOT_ALLOWED);
        assert_eq!(api.calls().len(), 1, "only the read");
    }

    #[test]
    fn the_apis_refusal_says_the_same() {
        // The rules changed between the read and the share.
        let api = StandIn::answering(vec![
            (200, listing(vec![], Shows::default(), None, true)),
            (
                403,
                json!({ "error": { "code": "FORBIDDEN", "message": NOT_ALLOWED } }),
            ),
        ]);
        let refused = api.share(Chosen::default());
        assert_eq!(refused.unwrap_err(), NOT_ALLOWED);
        // An answer without the sentence gets rvrs's own.
        let api = StandIn::answering(vec![(403, json!({ "error": { "code": "FORBIDDEN" } }))]);
        let refused = api.send(Shows::default());
        assert_eq!(refused.unwrap_err(), NOT_ALLOWED);
    }

    #[test]
    fn a_shared_run_answers_its_link_without_a_second_ask() {
        let api = StandIn::answering(vec![(
            200,
            listing(
                vec![link(Shows::default(), Some("2026-12-24T12:00:00.000Z"))],
                shows(false, false, false),
                Some(30),
                false,
            ),
        )]);
        let share = api.share(Chosen::default()).unwrap();
        assert_eq!(share.url, "https://app.rvrs.dev/r/q3Lr8XwT2a");
        assert_eq!(api.calls().len(), 1);
    }

    #[test]
    fn a_member_who_may_not_share_still_stops_their_link() {
        let api = StandIn::answering(vec![
            (
                200,
                listing(
                    vec![link(Shows::default(), None)],
                    Shows::default(),
                    None,
                    false,
                ),
            ),
            (204, Value::Null),
        ]);
        let ended = api.stop(Some(RUN)).unwrap();
        assert_eq!(ended, ["https://app.rvrs.dev/r/q3Lr8XwT2a"]);
        assert_eq!(api.calls()[1].0, "DELETE");
        assert_eq!(api.calls()[1].1, "/v1/shares/q3Lr8XwT2a");
        // A run the account doesn't have has no link, and nothing is asked.
        let quiet = StandIn::default();
        assert!(quiet.stop(None).unwrap().is_empty());
        assert!(quiet.calls().is_empty());
    }

    #[test]
    fn expiry_is_said_before_and_after_the_link_is_made() {
        assert_eq!(expires_after(30), "The link stops working after 30 days.");
        assert_eq!(expires_after(7), "The link stops working after 7 days.");
        assert_eq!(date_words(2026, 12, 24), "Dec 24, 2026");
        assert_eq!(date_words(2027, 1, 5), "Jan 5, 2027");
        // Noon in UTC is the same day in every time zone but the few past UTC+12.
        let said = expires_on("2026-12-24T12:00:00.000Z").unwrap();
        assert!(
            said == "The link stops working on Dec 24, 2026."
                || said == "The link stops working on Dec 25, 2026.",
            "{said}"
        );
        assert_eq!(expires_on("never"), None);
    }

    #[test]
    fn the_line_under_the_link_ends_with_the_day_it_stops() {
        use rvrs_core::model::Tool;
        let run = Run::new(Tool::Ping, "192.0.2.1", rvrs_core::FamilyChoice::Auto, 0, 0);
        let lasting: Share = serde_json::from_value(link(Shows::default(), None)).unwrap();
        assert_eq!(
            sentence(&run, &lasting),
            format!(
                "Anyone with the link can open {} without signing in. {}",
                crate::commands::run_name(&run),
                what_it_shows(Shows::default())
            )
        );
        let expiring: Share =
            serde_json::from_value(link(Shows::default(), Some("2026-12-24T12:00:00.000Z")))
                .unwrap();
        let said = sentence(&run, &expiring);
        assert!(
            said.ends_with("The link stops working on Dec 24, 2026.")
                || said.ends_with("The link stops working on Dec 25, 2026."),
            "{said}"
        );
    }

    #[test]
    fn the_sentence_says_what_the_link_shows_and_leaves_out() {
        assert_eq!(
            what_it_shows(Shows::default()),
            "It shows the private network. It leaves out the public address and the install\u{2019}s name."
        );
        assert_eq!(
            what_it_shows(shows(true, true, true)),
            "It shows the private network, the public address and the install\u{2019}s name."
        );
        assert_eq!(
            what_it_shows(shows(false, false, false)),
            "It leaves out the private network, the public address and the install\u{2019}s name."
        );
    }
}
