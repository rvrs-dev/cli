//! The Console: what `rvrs` opens when run without a command. The app's Console on a terminal:
//! one field, Recent underneath, the result beside it on a wide terminal, and a sheet for the
//! selected hop with its network and the things to do with that host.

use std::io::Write;
use std::net::IpAddr;
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style as Paint};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use ratatui::Frame;

use rvrs_core::cases::{self, Case, Cases};
use rvrs_core::dns::result::{
    changes, explain_record, tool_host, zone_line, RecordGroup, SourceStatus, VerdictGrade,
};
use rvrs_core::dns::{self, Diff, DnsResult, Dnssec, Record, SourceResult};
use rvrs_core::engine::{MtrOptions, Options, PingOptions};
use rvrs_core::favorites::Favorites;
use rvrs_core::history::History;
use rvrs_core::model::{groups, Grade, Hop, Outcome, Run, Slot, Tool, Verdict};
use rvrs_core::session::{Lookups, Session};
use rvrs_core::{format, is_private, FamilyChoice};

#[path = "console_account.rs"]
mod account_view;
#[path = "console_bgp.rs"]
mod bgp_view;
#[path = "console_help.rs"]
mod help_view;
#[path = "console_run.rs"]
mod run_picker;
#[path = "console_share.rs"]
mod share_sheet;
use bgp_view::{BgpTab, PrefixFilter};
use run_picker::Launcher;
use share_sheet::Sheet;

use crate::account;
use crate::args::normalize_target;
use crate::pretty::{self, View};
use crate::style::{truncate_end, truncate_middle, width, Style, SPINNER};

/// The Console's chrome, in the terminal's own sixteen colors so it follows the person's theme:
/// yellow labels, blue keys, a cyan frame and selection, magenta for the tool. A figure or a state
/// takes its color from COLOR.md, and a host, a network or a count is ink.
const LABEL: Color = Color::Yellow;
const KEY: Color = Color::Blue;
const ACCENT: Color = Color::Cyan;
const TOOL: Color = Color::Magenta;

/// The mark at the top right, the try shell's art (the user's design). Each row has a mask of the
/// same width that colors it: w the mark and "networking" in bold white, g "rvrs" in green, m the
/// rules and the signature in gray, d the dots in dim gray. A narrower header shows the first
/// [`LOGO_CUT`] columns, the mark and the rules, and a narrow one none of it.
const LOGO: [&str; 6] = [
    r"  _____       __                 :           :",
    r"__\_  /___ __(_/__ _ ____        .           .                .",
    "\\  /  .  /   .   \\      \\-\u{00BB}     rvrs        \u{00B7}:\u{00B7}\u{00B7} \u{00B7}            : snp!",
    r"/__\     \  ____  \   _\ \   networking     .:.....:..........:.. .   .",
    "===/__/__/========______/==============\u{00BB} ....:.....:.         :",
    "                                    :         :          \u{00B7}",
];

const LOGO_MASK: [&str; 6] = [
    "  wwwww       ww                 d           d",
    "wwww  wwww wwwwwww w wwww        d           d                d",
    "w  w  w  w   w   w      www     gggg        dddd d            d mmmm",
    "wwww     w  wwww  w   ww w   wwwwwwwwww     ddddddddddddddddddddd d   d",
    "mmmwwwwwwwmmmmmmmmwwwwwwwmmmmmmmmmmmmmmm dddddddddddd         d",
    "                                    d         d          d",
];

/// The columns of the mark, "networking" and the rules, cut before the dots.
const LOGO_CUT: usize = 40;

/// The logo's rows in at most `columns` columns, colored by its mask.
fn logo_lines(columns: usize) -> Vec<Line<'static>> {
    LOGO.iter()
        .zip(LOGO_MASK)
        .map(|(row, mask)| {
            let mask: Vec<char> = mask.chars().collect();
            let mut spans: Vec<Span<'static>> = Vec::new();
            for (index, character) in row.chars().take(columns).enumerate() {
                let paint = match mask.get(index) {
                    _ if character == ' ' => Paint::new(),
                    Some('w') => Paint::new().fg(Color::White).add_modifier(Modifier::BOLD),
                    Some('g') => Paint::new()
                        .fg(Color::Indexed(149))
                        .add_modifier(Modifier::BOLD),
                    Some('m') => Paint::new().fg(Color::Indexed(246)),
                    Some('d') => Paint::new().fg(Color::Indexed(240)),
                    _ => Paint::new(),
                };
                match spans.last_mut() {
                    Some(last) if last.style == paint => last.content.to_mut().push(character),
                    _ => spans.push(Span::styled(character.to_string(), paint)),
                }
            }
            Line::from(spans)
        })
        .collect()
}

/// The logo's full width.
fn logo_width() -> usize {
    LOGO.iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    /// The field and the list under it.
    Start,
    /// The open run.
    Result,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Trace,
    Mtr,
    Ping,
    Bgp,
    Favorite,
    CopyAddress,
    CopyHostname,
    Whois,
    /// A DNS record's value.
    CopyValue,
    /// A DNS record as a line of a zone file.
    CopyZoneLine,
}

impl Action {
    /// The key that does it while a hop is selected.
    fn key(self) -> &'static str {
        match self {
            Action::Trace => "t",
            Action::Mtr => "m",
            Action::Ping => "p",
            Action::Bgp => "b",
            Action::Favorite => "f",
            Action::CopyAddress => "c",
            Action::CopyHostname => "n",
            Action::Whois => "w",
            Action::CopyValue => "c",
            Action::CopyZoneLine => "z",
        }
    }

    /// Title case, as the app's buttons and menus.
    fn title(self) -> &'static str {
        match self {
            Action::Trace => "Run Traceroute",
            Action::Mtr => "Run MTR",
            Action::Ping => "Run Ping",
            Action::Bgp => "Run BGP",
            Action::Favorite => "Favorite",
            Action::CopyAddress => "Copy Address",
            Action::CopyHostname => "Copy Hostname",
            Action::Whois => "Whois",
            Action::CopyValue => "Copy Value",
            Action::CopyZoneLine => "Copy as Zone Line",
        }
    }
}

/// The tabs of a DNS result, as the app has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DnsTab {
    Overview,
    Records,
    Compare,
    Trace,
}

impl DnsTab {
    const ALL: [DnsTab; 4] = [
        DnsTab::Overview,
        DnsTab::Records,
        DnsTab::Compare,
        DnsTab::Trace,
    ];

    fn title(self) -> &'static str {
        match self {
            DnsTab::Overview => "Overview",
            DnsTab::Records => "Records",
            DnsTab::Compare => "Compare",
            DnsTab::Trace => "Trace",
        }
    }
}

/// The run on screen: live from a session, or opened from history.
enum Shown {
    Live {
        session: Box<Session>,
        started: Instant,
        elapsed: Option<Duration>,
        saved: bool,
    },
    Saved(Box<Run>),
}

impl Shown {
    fn run(&self) -> &Run {
        match self {
            Shown::Live { session, .. } => &session.run,
            Shown::Saved(run) => run,
        }
    }
}

/// The parts of the Console, as tabs in the footer: the app's tabs on a terminal. Each is
/// reached with shift and its number from anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// The Target field and Recent.
    Console,
    /// Hosts kept on purpose. Return runs the selected tool against one.
    Favorites,
    /// Saved runs and the cases they sit in. Without a case it lists the saved runs; with
    /// cases its first level is All saved and each case.
    Cases,
    /// Who this install is signed in as, and signing in and out.
    Account,
}

/// Which saved runs the Cases section shows once you have gone into an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    All,
    Case(String),
}

/// The small list that puts a run in cases: every case, the ones holding the run checked, and
/// a New case row to type a name into.
struct Picker {
    run: Run,
    /// A case, or the New case row after the last one.
    row: usize,
    name: String,
}

impl Section {
    const ALL: [Section; 4] = [
        Section::Console,
        Section::Favorites,
        Section::Cases,
        Section::Account,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Console => "Console",
            Section::Favorites => "Favorites",
            Section::Cases => "Cases",
            Section::Account => "Account",
        }
    }

    fn number(self) -> usize {
        Section::ALL
            .iter()
            .position(|section| *section == self)
            .unwrap_or(0)
            + 1
    }

    /// Shift and a number. Terminals send the character the layout puts on that key, so the
    /// shifted digits of the common layouts are read: ! for 1, @ or " for 2, # or § or £ for 3,
    /// $ or ¤ for 4.
    /// None of them can be part of a target, so they work while typing too.
    fn of(key: &KeyEvent) -> Option<Section> {
        let number = match key.code {
            KeyCode::Char('!') => 1,
            KeyCode::Char('@' | '"') => 2,
            KeyCode::Char('#' | '\u{00A7}' | '\u{00A3}') => 3,
            KeyCode::Char('$' | '\u{00A4}') => 4,
            KeyCode::Char(digit @ '1'..='9') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                digit as usize - '0' as usize
            }
            _ => return None,
        };
        Section::ALL.get(number.checked_sub(1)?).copied()
    }
}

pub struct App {
    history: History,
    /// Runs kept on purpose, apart from history.
    saved: History,
    /// Named sets of saved runs.
    cases: Cases,
    /// The entry of the Cases section you went into: All saved, or one case.
    case: Option<Scope>,
    /// Open while a run is being put in cases.
    picker: Option<Picker>,
    /// Open while a tool is being picked for a host.
    launcher: Option<Launcher>,
    /// Open while a run is being shared.
    share: Option<Sheet>,
    /// Who this install is signed in as, and a login in progress.
    account: account_view::Account,
    /// Hosts kept on purpose. The app's Favorites.
    favorites: Favorites,
    /// When a sync last changed what this device keeps, as far as the Console has seen.
    synced: Option<std::time::SystemTime>,
    section: Section,
    input: String,
    /// Whether the Target field has the keys. It never has them by itself: a tool's key or `:`
    /// gives them to it, and until then every key is a command, as in k9s.
    field: bool,
    /// The host picked among what the field suggests, by its place in `suggestions`. None while
    /// the typed text is what Return runs.
    suggestion: Option<usize>,
    /// What `/` narrows the list on screen to: Recent, the hops, the records or a lookup's rows.
    search: String,
    /// Whether the keys go to the search line at the foot of the screen.
    searching: bool,
    /// The tool the field runs: the highlighted chip.
    tool: Tool,
    /// Selected row of the list under the field. None while nothing is selected.
    row: Option<usize>,
    pane: Pane,
    shown: Option<Shown>,
    hop: usize,
    scroll: usize,
    /// The selected hop's details, in a column beside the table.
    details: bool,
    /// Whether the keys go to the details column. Tab moves them between the two.
    details_focus: bool,
    /// The selected action in the details column.
    action: usize,
    /// MTR: the figures give way to a wide strip of every hop's rounds.
    mtr_history: bool,
    /// DNS: the open tab, the selected row in it, and the source the Records tab shows.
    dns_tab: DnsTab,
    dns_row: usize,
    dns_source: usize,
    /// BGP: the open tab, the selected row in it, the side Neighbors shows, the filter of
    /// Prefixes and how many days History looks back.
    bgp_tab: BgpTab,
    bgp_row: usize,
    bgp_side: rvrs_core::bgp::Side,
    bgp_filter: PrefixFilter,
    bgp_days: u64,
    /// How wide a BGP lookup's rows were last drawn, so the keys read the rows the screen shows.
    bgp_columns: std::cell::Cell<usize>,
    /// What the daily look found when rvrs has fallen behind: "0.3.1 is out", "4 new commits".
    update: Option<String>,
    /// Open while `?` shows the keys or a tool's commands.
    help: Option<help_view::Help>,
    toast: Option<(String, Instant)>,
    frame: usize,
    /// How many rows PageUp and PageDown move: the list's height when it was last drawn.
    page: usize,
    quit: bool,
}

/// What the list under the field holds.
enum Rows<'a> {
    /// Runs, from history or saved runs. Return opens one without running it again.
    Runs(Vec<&'a Run>),
    /// Favorite hosts. Return runs the selected tool against one.
    Hosts(Vec<&'a str>),
    /// All saved (None) and the cases. Return goes into one.
    Cases(Vec<Option<&'a Case>>),
}

impl Rows<'_> {
    fn len(&self) -> usize {
        match self {
            Rows::Runs(runs) => runs.len(),
            Rows::Hosts(hosts) => hosts.len(),
            Rows::Cases(cases) => cases.len(),
        }
    }
}

/// What is typed in the Target field or the search, read as a filter: every word must appear in
/// the tool or the host. "ping git" keeps the pings to hosts with "git" in their name.
fn matches(filter: &[String], tool: &str, host: &str) -> bool {
    found(filter, &format!("{tool} {host}"))
}

/// Whether every word is somewhere in the text, without regard to case. No words find anything.
fn found(words: &[String], text: &str) -> bool {
    let text = text.to_lowercase();
    words.iter().all(|word| text.contains(word.as_str()))
}

/// The words of a search or a filter, lower case.
fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_lowercase).collect()
}

/// What a hop answers to in a search: every address that replied, its name and its network.
fn hop_found(run: &Run, hop: &Hop, words: &[String]) -> bool {
    if words.is_empty() {
        return true;
    }
    let mut text = String::new();
    for address in hop.addresses() {
        text.push_str(&format!("{address} "));
        if let Some(name) = run.name(address) {
            text.push_str(&format!("{name} "));
        }
        if let Some(network) = run.network(address) {
            text.push_str(&format!("{} {} ", network.label(), network.name));
        }
    }
    found(words, &text)
}

/// What a record answers to in a search: its name, type and value.
fn record_found(record: &Record, words: &[String]) -> bool {
    found(
        words,
        &format!("{} {} {}", record.name, record.type_name(), record.value),
    )
}

/// How many hosts the field suggests at most.
const SUGGESTIONS: usize = 8;

/// Where the keys are, as far as a search cares: a search belongs to the list it narrowed and
/// ends when the screen shows another one.
#[derive(Debug, Clone, PartialEq)]
struct Place {
    pane: Pane,
    section: Section,
    case: Option<Scope>,
    run: Option<String>,
    dns_tab: DnsTab,
    dns_source: usize,
    bgp_tab: BgpTab,
}

pub fn open(target: Option<String>) -> ExitCode {
    let saved = History::saved();
    let cases = Cases::open(&saved);
    let mut app = App::new(History::open(), saved, cases, Favorites::open());
    // The tool chosen last, kept across sessions.
    let remembered = rvrs_core::paths::config_dir().join("console.json");
    if let Some(tool) = last_tool(&remembered) {
        app.tool = tool;
    }
    let mut kept_tool = app.tool;
    if let Some(target) = target {
        app.input = target;
        app.run_input();
    }
    app.update = crate::update::newer_fact();
    crate::update::look_when_due();
    let mut terminal = ratatui::init();
    let outcome = (|| -> std::io::Result<()> {
        while !app.quit {
            app.tick();
            terminal.draw(|frame| app.draw(frame))?;
            if event::poll(Duration::from_millis(60))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind != KeyEventKind::Release {
                        app.key(key);
                    }
                    if app.tool != kept_tool {
                        kept_tool = app.tool;
                        keep_tool(&remembered, kept_tool);
                    }
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    app.retire();
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => crate::fail(&format!("The Console lost the terminal ({error}).")),
    }
}

/// The tool the Console chose last, from `console.json`. A tool the CLI can't run yet is
/// left out.
fn last_tool(path: &std::path::Path) -> Option<Tool> {
    let data = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&data).ok()?;
    let tool: Tool = serde_json::from_value(value.get("tool")?.clone()).ok()?;
    crate::features::runs(tool).then_some(tool)
}

/// Writes the chosen tool to `console.json`. A write that fails costs only the memory of it.
fn keep_tool(path: &std::path::Path, tool: Tool) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, serde_json::json!({ "tool": tool }).to_string());
}

/// When the sync last changed what this device keeps.
fn sync_mark() -> Option<std::time::SystemTime> {
    std::fs::metadata(crate::sync::changed_path())
        .and_then(|file| file.modified())
        .ok()
}

impl App {
    pub fn new(history: History, saved: History, cases: Cases, favorites: Favorites) -> Self {
        Self {
            history,
            saved,
            cases,
            case: None,
            picker: None,
            launcher: None,
            share: None,
            account: account_view::Account::load(),
            favorites,
            synced: sync_mark(),
            section: Section::Console,
            input: String::new(),
            field: false,
            suggestion: None,
            search: String::new(),
            searching: false,
            // traceroute, or the first tool this build runs.
            tool: Tool::ALL
                .into_iter()
                .find(|tool| crate::features::runs(*tool))
                .unwrap_or(Tool::Traceroute),
            row: None,
            pane: Pane::Start,
            shown: None,
            hop: 0,
            scroll: 0,
            details: false,
            details_focus: false,
            action: 0,
            mtr_history: false,
            dns_tab: DnsTab::Overview,
            dns_row: 0,
            dns_source: 0,
            bgp_tab: BgpTab::Overview,
            bgp_row: 0,
            bgp_side: rvrs_core::bgp::Side::Left,
            bgp_filter: PrefixFilter::All,
            bgp_days: 7,
            bgp_columns: std::cell::Cell::new(100),
            update: None,
            help: None,
            toast: None,
            frame: 0,
            page: 10,
            quit: false,
        }
    }

    /// The words the list is narrowed to: the search's. What is typed in the Target field never
    /// narrows it: the field suggests hosts of its own.
    fn filter(&self) -> Vec<String> {
        self.search_words()
    }

    /// What the field holds: a tool named in front, as in `ping example.com`, and the target
    /// being typed.
    fn typed(&self) -> (Option<Tool>, String) {
        let text = self.input.trim_start();
        if let Some((first, rest)) = text.split_once(' ') {
            let named = Tool::ALL.into_iter().find(|tool| {
                crate::features::runs(*tool) && tool.command() == first.to_lowercase()
            });
            if let Some(tool) = named {
                return (Some(tool), rest.trim().to_string());
            }
        }
        (None, text.trim_end().to_string())
    }

    /// The hosts the field suggests for what is typed, as the app suggests them: favorites, then
    /// hosts from history, newest first. Those that start with what is typed come before those
    /// that only hold it.
    fn suggestions(&self) -> Vec<String> {
        let (_, typed) = self.typed();
        let typed = typed.to_lowercase();
        if typed.is_empty() || typed.contains(' ') {
            return Vec::new();
        }
        let mut seen = std::collections::HashSet::new();
        let (mut starting, mut holding) = (Vec::new(), Vec::new());
        let hosts = self
            .favorites
            .hosts
            .iter()
            .chain(self.history.runs.iter().map(|run| &run.target));
        for host in hosts {
            let lower = host.to_lowercase();
            if !lower.contains(&typed) || !seen.insert(lower.clone()) {
                continue;
            }
            if lower.starts_with(&typed) {
                starting.push(host.clone());
            } else {
                holding.push(host.clone());
            }
        }
        starting.extend(holding);
        starting.truncate(SUGGESTIONS);
        starting
    }

    /// The suggestion that is picked, while it is still among the suggestions.
    fn picked(&self) -> Option<String> {
        self.suggestion
            .and_then(|index| self.suggestions().into_iter().nth(index))
    }

    /// The words of the search, lower case.
    fn search_words(&self) -> Vec<String> {
        words(&self.search)
    }

    /// The list under the field, narrowed to what the typed words match.
    fn rows(&self) -> Rows<'_> {
        let filter = self.filter();
        let keep = |run: &&Run| matches(&filter, run.tool.command(), &run.target);
        match self.section {
            Section::Console => {
                Rows::Runs(self.history.runs.iter().filter(keep).take(50).collect())
            }
            // Without a case the saved runs are the list, as they were before cases.
            Section::Cases if self.cases.cases.is_empty() => {
                Rows::Runs(self.saved.runs.iter().filter(keep).collect())
            }
            Section::Cases => match &self.case {
                None => Rows::Cases(
                    std::iter::once(None)
                        .chain(
                            self.cases
                                .cases
                                .iter()
                                .filter(|case| matches(&filter, "", &case.name))
                                .map(Some),
                        )
                        .collect(),
                ),
                Some(Scope::All) => Rows::Runs(self.saved.runs.iter().filter(keep).collect()),
                Some(Scope::Case(id)) => {
                    let case = self.cases.get(id);
                    Rows::Runs(
                        self.saved
                            .runs
                            .iter()
                            .filter(|run| case.is_some_and(|case| case.holds(&run.id)))
                            .filter(keep)
                            .collect(),
                    )
                }
            },
            Section::Account => Rows::Hosts(Vec::new()),
            Section::Favorites => Rows::Hosts(
                self.favorites
                    .hosts
                    .iter()
                    .map(String::as_str)
                    .filter(|host| matches(&filter, "", host))
                    .collect(),
            ),
        }
    }

    /// The host of the selected row: a run's target, or a favorite.
    fn selected_host(&self) -> Option<String> {
        match (self.row, self.rows()) {
            (Some(row), Rows::Runs(runs)) => runs.get(row).map(|run| run.target.clone()),
            (Some(row), Rows::Hosts(hosts)) => hosts.get(row).map(|host| (*host).to_string()),
            _ => None,
        }
    }

    /// The run of the selected row, when the row is a run.
    fn selected_run(&self) -> Option<Run> {
        match (self.row, self.rows()) {
            (Some(row), Rows::Runs(runs)) => runs.get(row).map(|run| (*run).clone()),
            _ => None,
        }
    }

    /// The list's name in the panel title.
    fn list_name(&self) -> String {
        match self.section {
            Section::Console => "Recent".into(),
            Section::Favorites => "Favorites".into(),
            Section::Account => "Account".into(),
            Section::Cases if self.cases.cases.is_empty() => "Saved".into(),
            Section::Cases => match &self.case {
                None => "Cases".into(),
                Some(Scope::All) => "All saved".into(),
                Some(Scope::Case(id)) => self
                    .cases
                    .get(id)
                    .map_or_else(|| "Case".into(), |case| case.name.clone()),
            },
        }
    }

    /// The case you went into, when it is one case and not All saved.
    fn open_case(&self) -> Option<&Case> {
        match (&self.section, &self.case) {
            (Section::Cases, Some(Scope::Case(id))) => self.cases.get(id),
            _ => None,
        }
    }

    /// Back from a case to the list of cases, with that case selected.
    fn leave_case(&mut self) {
        let Some(scope) = self.case.take() else {
            return;
        };
        self.row = match self.rows() {
            Rows::Cases(entries) => entries.iter().position(|entry| match (entry, &scope) {
                (None, Scope::All) => true,
                (Some(case), Scope::Case(id)) => &case.id == id,
                _ => false,
            }),
            _ => None,
        };
    }

    /// Makes a host a favorite, or stops it being one, and says which.
    fn toggle_favorite(&mut self, host: &str) {
        match self.favorites.toggle(host) {
            Ok(true) => self.toast(format!("{host} is a favorite now")),
            Ok(false) => self.toast(format!("{host} is no longer a favorite")),
            Err(error) => self.toast(format!("Couldn't save the favorite ({error}).")),
        }
    }

    /// Keeps the selection on a row that still exists after the list changed.
    fn clamp_row(&mut self) {
        let count = self.rows().len();
        self.row = match self.row {
            Some(_) if count == 0 => None,
            Some(row) => Some(row.min(count - 1)),
            None => None,
        };
    }

    fn run(&self) -> Option<&Run> {
        self.shown.as_ref().map(Shown::run)
    }

    /// Whether this install can share a run: a person signed in with a login.
    fn can_share(&self) -> bool {
        matches!(self.account.state, account_view::State::SignedIn { .. })
    }

    fn is_saved(&self) -> bool {
        self.run().is_some_and(|run| self.saved.contains(&run.id))
    }

    /// A run can be saved once it has ended and was made.
    fn can_keep(run: &Run) -> bool {
        !run.is_running() && !matches!(run.outcome, Some(Outcome::Failed(_)))
    }

    /// Saves the open run, or removes it from saved runs again.
    fn toggle_save(&mut self) {
        let Some(run) = self.run().cloned() else {
            return;
        };
        self.toggle_save_of(&run);
    }

    /// Saving puts the run in the case used last, as the app does. Removing it from saved runs
    /// takes it out of every case.
    fn toggle_save_of(&mut self, run: &Run) {
        if !Self::can_keep(run) {
            return;
        }
        if self.saved.contains(&run.id) {
            match cases::unsave(&mut self.saved, &mut self.cases, &run.id) {
                Ok(_) => self.toast("Removed from saved runs"),
                Err(error) => self.toast(format!("Couldn't remove the saved run ({error}).")),
            }
            return;
        }
        match cases::save(&mut self.saved, &mut self.cases, run, None) {
            Ok(kept) => match kept.case {
                Some(case) => self.toast(format!("Saved to {}", case.name)),
                None => self.toast("Saved"),
            },
            Err(error) => self.toast(format!("Couldn't save the run ({error}).")),
        }
    }

    /// Opens the case picker for a run.
    fn open_picker(&mut self, run: Run) {
        if !Self::can_keep(&run) {
            return;
        }
        // With no case yet, the New case row is the only place to go.
        self.picker = Some(Picker {
            run,
            row: 0,
            name: String::new(),
        });
    }

    /// Puts the picker's run in the case on that row, or takes it out.
    fn toggle_case(&mut self, index: usize) {
        let Some(picker) = &self.picker else { return };
        let run = picker.run.clone();
        let Some(case) = self.cases.cases.get(index).cloned() else {
            return;
        };
        if case.holds(&run.id) {
            match self.cases.remove(&case.id, &run.id) {
                Ok(_) => self.toast(format!("Removed from {}", case.name)),
                Err(error) => self.toast(format!("Couldn't save the case ({error}).")),
            }
            return;
        }
        match cases::save(&mut self.saved, &mut self.cases, &run, Some(&case.name)) {
            Ok(kept) if kept.newly_saved => self.toast(format!("Saved to {}", case.name)),
            Ok(_) => self.toast(format!("Added to {}", case.name)),
            Err(error) => self.toast(format!("Couldn't save the run ({error}).")),
        }
    }

    /// Makes a case from the typed name and puts the picker's run in it.
    fn create_case(&mut self) {
        let Some(picker) = &self.picker else { return };
        let name = picker.name.trim().to_string();
        let run = picker.run.clone();
        if name.is_empty() {
            self.toast("Type a name for the new case");
            return;
        }
        match cases::save(&mut self.saved, &mut self.cases, &run, Some(&name)) {
            Ok(kept) => {
                let name = kept.case.map_or(name, |case| case.name);
                self.toast(format!("Saved to {name}"));
                if let Some(picker) = &mut self.picker {
                    picker.name.clear();
                    picker.row = self
                        .cases
                        .find(&name)
                        .and_then(|case| self.cases.cases.iter().position(|c| c.id == case.id))
                        .unwrap_or(0);
                }
            }
            Err(error) => self.toast(format!("Couldn't save the run ({error}).")),
        }
    }

    fn picker_key(&mut self, key: KeyEvent) {
        let count = self.cases.cases.len();
        let Some(picker) = &mut self.picker else {
            return;
        };
        let on_new = picker.row >= count;
        match key.code {
            KeyCode::Esc if !picker.name.is_empty() => picker.name.clear(),
            KeyCode::Esc | KeyCode::Left => self.picker = None,
            KeyCode::Up => picker.row = picker.row.saturating_sub(1),
            KeyCode::Down => picker.row = (picker.row + 1).min(count),
            KeyCode::Enter if on_new => self.create_case(),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') if !on_new => {
                let row = picker.row;
                self.toggle_case(row);
            }
            KeyCode::Backspace => {
                picker.name.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                picker.name.clear()
            }
            // Typing goes to the New case row, wherever the selection was.
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !(character == ' ' && picker.name.is_empty()) =>
            {
                picker.row = count;
                picker.name.push(character);
            }
            _ => {}
        }
    }

    /// Opens a section's list. The open run stays open: Return with nothing selected goes back to
    /// it.
    fn go_to(&mut self, section: Section) {
        self.section = section;
        self.case = None;
        self.row = None;
        self.field = false;
        self.input.clear();
        self.suggestion = None;
        self.details = false;
        self.details_focus = false;
        self.pane = Pane::Start;
    }

    /// Gives the keys to the Target field, for this tool. From a result or another section it
    /// goes back to the Console first, where the field is.
    fn type_target(&mut self, tool: Tool) {
        if !crate::features::runs(tool) {
            if let Some(feature) =
                crate::features::of_tool(tool).filter(|name| !crate::features::has(name))
            {
                self.toast(crate::features::missing(feature));
            }
            return;
        }
        if self.section == Section::Account {
            self.go_to(Section::Console);
        }
        self.tool = tool;
        self.pane = Pane::Start;
        self.row = None;
        self.field = true;
        self.suggestion = None;
        // A new target is a new question: the suggestions follow what is typed, not an old search.
        self.search.clear();
        self.searching = false;
    }

    /// The tool a key picks on a list: the run picker's letters, t, m, p, d and b.
    fn tool_of(key: &KeyEvent) -> Option<Tool> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        let KeyCode::Char(typed) = key.code else {
            return None;
        };
        Tool::ALL
            .into_iter()
            .filter(|tool| crate::features::runs(*tool))
            .find(|tool| run_picker::letter(*tool) == typed)
    }

    /// Opens the search line at the foot of the screen. A new search starts empty.
    fn open_search(&mut self) {
        self.search.clear();
        self.searching = true;
    }

    /// Where the keys are: a search ends when this changes.
    fn place(&self) -> Place {
        Place {
            pane: if self.shown.is_some() {
                self.pane
            } else {
                Pane::Start
            },
            section: self.section,
            case: self.case.clone(),
            run: self.run().map(|run| run.id.clone()),
            dns_tab: self.dns_tab,
            dns_source: self.dns_source,
            bgp_tab: self.bgp_tab,
        }
    }

    /// The hops a search leaves, by their place in the path. Every hop without one.
    fn hop_rows(&self) -> Vec<usize> {
        let Some(run) = self.run() else {
            return Vec::new();
        };
        let words = self.search_words();
        run.path()
            .iter()
            .enumerate()
            .filter(|(_, hop)| hop_found(run, hop, &words))
            .map(|(index, _)| index)
            .collect()
    }

    /// Moves the selected hop through the hops a search leaves: `by` rows down, or up when it
    /// is negative. A hop the search left out steps to the nearest one it kept.
    fn step_hop(&mut self, by: isize) {
        let rows = self.hop_rows();
        if rows.is_empty() {
            return;
        }
        let count = rows.len() as isize;
        // Between two rows, the first step down lands on the one below and up on the one above.
        let target = match rows.iter().position(|index| *index >= self.hop) {
            Some(at) if rows[at] == self.hop => at as isize + by,
            Some(at) if by > 0 => at as isize + by - 1,
            Some(at) => at as isize + by,
            None if by > 0 => count - 1,
            None => count + by,
        };
        self.hop = rows[target.clamp(0, count - 1) as usize];
    }

    /// After the search changed: the selection goes to the first row it leaves, so a search
    /// lands on its first hit.
    fn land_search(&mut self) {
        let on_start = self.pane == Pane::Start || self.shown.is_none();
        if on_start {
            self.row = (self.rows().len() > 0).then_some(0);
            return;
        }
        match self.run().map(|run| run.tool) {
            Some(Tool::Traceroute | Tool::Mtr) => {
                if let Some(first) = self.hop_rows().first() {
                    self.hop = *first;
                }
                self.scroll = 0;
            }
            Some(Tool::Dns) => self.dns_row = 0,
            Some(Tool::Bgp) => self.bgp_row = 0,
            _ => {}
        }
    }

    /// Whether `/` has rows to narrow on this screen.
    fn can_search(&self) -> bool {
        let on_start = self.pane == Pane::Start || self.shown.is_none();
        if on_start {
            return self.section != Section::Account;
        }
        match self.run() {
            Some(run) if run.is_report_only() => false,
            Some(run) => match run.tool {
                Tool::Traceroute | Tool::Mtr => true,
                Tool::Dns => matches!(self.dns_tab, DnsTab::Overview | DnsTab::Records),
                Tool::Bgp => true,
                Tool::Ping | Tool::Iperf3 => false,
            },
            None => false,
        }
    }

    /// The search line takes the keys while it is open. Return keeps what it found and gives the
    /// keys back to the list, esc drops it, and an arrow moves through what it found.
    fn search_key(&mut self, key: KeyEvent) {
        match key.code {
            // esc leaves the selection where it was: a hop keeps its place in the path.
            KeyCode::Esc => {
                self.search.clear();
                self.searching = false;
                self.clamp_row();
            }
            KeyCode::Enter => self.searching = false,
            KeyCode::Backspace => {
                if self.search.pop().is_none() {
                    self.searching = false;
                }
                self.land_search();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search.clear();
                self.land_search();
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                self.searching = false;
                self.dispatch(key);
            }
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !(character == ' '
                        && (self.search.is_empty() || self.search.ends_with(' '))) =>
            {
                self.search.push(character);
                self.land_search();
            }
            _ => {}
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    // Running

    fn start(&mut self, target: &str) {
        // An AS number or a prefix is not a host. Only BGP can take it.
        if rvrs_core::bgp::Target::is_routing_only(target) {
            self.tool = Tool::Bgp;
        }
        self.start_with(self.tool, target);
    }

    /// Starts a run with the defaults the app's Console uses: a traceroute as the command runs
    /// it, a ping of 20 packets a second apart.
    fn start_with(&mut self, tool: Tool, target: &str) {
        let target = normalize_target(target);
        if target.is_empty() {
            return;
        }
        if let Some(feature) =
            crate::features::of_tool(tool).filter(|name| !crate::features::has(name))
        {
            self.toast(crate::features::missing(feature));
            return;
        }
        if tool == Tool::Bgp && rvrs_core::bgp::Target::parse(&target).is_none() {
            self.toast(format!(
                "\u{201C}{target}\u{201D} isn't a hostname, an address, a prefix or an AS number."
            ));
            return;
        }
        self.retire();
        let bgp_session = if tool == Tool::Bgp {
            Self::bgp_session(&target)
        } else {
            None
        };
        let session = match tool {
            Tool::Bgp if bgp_session.is_some() => match bgp_session {
                Some(session) => session,
                None => return,
            },
            // An MTR runs until it is stopped, as the original's live view does.
            Tool::Mtr => Session::start_mtr(
                &target,
                FamilyChoice::Auto,
                MtrOptions::default(),
                Lookups::ALL,
            ),
            Tool::Ping => Session::start_ping(
                &target,
                FamilyChoice::Auto,
                PingOptions {
                    count: Some(20),
                    ..PingOptions::default()
                },
                Lookups::ALL,
            ),
            Tool::Dns => Session::start_dns(&target, dns::Options::default(), Lookups::ALL),
            _ => Session::start(
                &target,
                FamilyChoice::Auto,
                Options::default(),
                Lookups::ALL,
            ),
        };
        self.dns_tab = DnsTab::Overview;
        self.dns_row = 0;
        self.dns_source = 0;
        self.reset_bgp();
        self.shown = Some(Shown::Live {
            session: Box::new(session),
            started: Instant::now(),
            elapsed: None,
            saved: false,
        });
        self.pane = Pane::Result;
        self.hop = 0;
        self.scroll = 0;
        self.details = false;
        self.details_focus = false;
        self.input.clear();
        self.field = false;
        self.suggestion = None;
        self.row = None;
    }

    /// Stops the live run, if there is one, and keeps what it measured in history. A run that
    /// is replaced or left behind on quitting is not thrown away.
    pub fn retire(&mut self) {
        let Some(Shown::Live { session, saved, .. }) = &mut self.shown else {
            return;
        };
        if !session.run.is_running() || *saved {
            return;
        }
        session.stop();
        let deadline = Instant::now() + Duration::from_millis(400);
        while session.run.is_running() && Instant::now() < deadline {
            session.pump(Duration::from_millis(20));
        }
        let measured = if session.run.tool == Tool::Ping {
            !session.run.packets().is_empty()
        } else if session.run.tool == Tool::Dns {
            session.run.dns.as_ref().is_some_and(|dns| !dns.is_empty())
        } else if session.run.tool == Tool::Bgp {
            session.run.bgp.as_ref().is_some_and(|bgp| !bgp.is_empty())
        } else {
            session.run.path_len() > 0
        };
        if !session.run.is_running() && measured {
            *saved = true;
            session.finish_lookups(Duration::ZERO);
            let run = session.run.clone();
            crate::pulse::count_run(run.tool);
            crate::sync::queue(&run);
            let _ = self.history.save(&run);
        }
    }

    /// Runs what is typed. One word is a target for the selected tool. Two words starting with
    /// a tool, as in "ping example.com", pick the tool as well.
    fn run_input(&mut self) {
        let words: Vec<String> = self.input.split_whitespace().map(str::to_string).collect();
        match words.as_slice() {
            [] => {}
            [target] => {
                let target = target.clone();
                self.start(&target);
            }
            [tool, target] => {
                let named = Tool::ALL.into_iter().find(|known| {
                    crate::features::runs(*known) && known.command() == tool.to_lowercase()
                });
                match named {
                    Some(tool) => {
                        self.tool = tool;
                        let target = target.clone();
                        self.start_with(tool, &target);
                    }
                    None => self
                        .toast("Type one target, or a tool and a target, as in ping example.com"),
                }
            }
            _ => self.toast("Type one target, or a tool and a target, as in ping example.com"),
        }
    }

    /// Folds in what the run learned since the last frame and saves it once it has ended.
    /// Reads history, saved runs, cases and favorites again after a sync changed them, so what
    /// another device made or kept shows without leaving the Console.
    fn take_in_sync(&mut self) {
        let mark = sync_mark();
        if mark == self.synced || self.picker.is_some() {
            return;
        }
        self.synced = mark;
        self.history = History::open();
        self.saved = History::saved();
        self.cases = Cases::open(&self.saved);
        self.favorites = Favorites::open();
        if let Some(Scope::Case(open)) = &self.case {
            if self.cases.get(open).is_none() {
                self.case = None;
            }
        }
        self.clamp_row();
    }

    pub fn tick(&mut self) {
        self.frame += 1;
        // About twice a second.
        if self.frame.is_multiple_of(8) {
            self.take_in_sync();
        }
        if let Some(sentence) = self.account.tick() {
            self.toast(sentence);
        }
        self.share_tick();
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, since)| since.elapsed() > Duration::from_millis(1800))
        {
            self.toast = None;
        }
        let Some(Shown::Live {
            session,
            started,
            elapsed,
            saved,
        }) = &mut self.shown
        else {
            return;
        };
        session.pump(Duration::ZERO);
        if session.run.is_running() {
            return;
        }
        if elapsed.is_none() {
            *elapsed = Some(started.elapsed());
        }
        // Save once the lookups have landed, or after they have had their moment.
        let settled = session.pending_lookups() == 0
            || elapsed.is_some_and(|took| started.elapsed() > took + Duration::from_secs(3));
        if !*saved && settled {
            *saved = true;
            session.finish_lookups(Duration::ZERO);
            // A lookup nobody answered has nothing to compare a later one with.
            let unanswered = (session.run.tool == Tool::Dns
                && session.run.dns.as_ref().is_none_or(DnsResult::is_empty))
                || (session.run.tool == Tool::Bgp
                    && session.run.bgp.as_ref().is_none_or(|bgp| bgp.is_empty()));
            if !matches!(session.run.outcome, Some(Outcome::Failed(_))) && !unanswered {
                let run = session.run.clone();
                crate::pulse::count_run(run.tool);
                crate::sync::queue(&run);
                if let Err(error) = self.history.save(&run) {
                    self.toast = Some((
                        format!("Couldn't save this run to history ({error})."),
                        Instant::now(),
                    ));
                }
            }
        }
    }

    // Keys

    pub fn key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
        {
            self.quit = true;
            return;
        }
        if self.help.is_some() {
            self.help_key(key);
            return;
        }
        // The picker takes every key while it is open, so a name can hold any character.
        if self.picker.is_some() {
            self.picker_key(key);
            return;
        }
        if self.launcher.is_some() {
            self.launcher_key(key);
            return;
        }
        if self.share.is_some() {
            self.share_key(key);
            return;
        }
        let place = self.place();
        if self.searching {
            self.search_key(key);
        } else {
            self.dispatch(key);
        }
        // A search narrows one list. Another list on screen starts without it.
        if self.place() != place {
            self.search.clear();
            self.searching = false;
        }
    }

    /// The keys of the screen underneath: the sections, then the list or the open run.
    fn dispatch(&mut self, key: KeyEvent) {
        if let Some(section) = Section::of(&key) {
            self.go_to(section);
            return;
        }
        if self.section == Section::Account && (self.pane == Pane::Start || self.shown.is_none()) {
            self.account_key(key);
            return;
        }
        match self.pane {
            Pane::Start if self.field => self.field_key(key),
            Pane::Start => self.start_key(key),
            Pane::Result => self.result_key(key),
        }
    }

    /// The Account section takes every key: there is no field to type into there. A tool's key
    /// and `:` go back to the Console's field, as they do from every list.
    fn account_key(&mut self, key: KeyEvent) {
        use account_view::State;
        if !self.account.is_busy() {
            if let Some(tool) = Self::tool_of(&key) {
                self.type_target(tool);
                return;
            }
            match key.code {
                KeyCode::Char(':') => {
                    self.type_target(self.tool);
                    return;
                }
                KeyCode::Char('?') => {
                    self.open_help();
                    return;
                }
                _ => {}
            }
        }
        match (key.code, &self.account.state) {
            (KeyCode::Char('s' | 'S'), State::SignedOut | State::Failed(_)) => {
                self.account.sign_in()
            }
            (KeyCode::Char('q' | 'Q'), State::SignedIn { .. }) => self.account.sign_out(),
            (KeyCode::Char('o'), State::Waiting { link, .. }) => {
                let link = link.clone();
                if !account::open_link(&link) {
                    self.toast("Couldn't open a browser. Copy the link with c");
                }
            }
            (KeyCode::Char('c'), State::Waiting { link, .. }) => {
                let link = link.clone();
                self.copy(&link);
            }
            (KeyCode::Esc | KeyCode::Left, _) if self.account.is_busy() => {
                if !matches!(self.account.state, State::SigningOut) {
                    self.account.cancel();
                    self.toast("Login canceled");
                }
            }
            (KeyCode::Esc | KeyCode::Left, _) => self.go_to(Section::Console),
            _ => {}
        }
    }

    /// The next tool this build runs, or the one before it.
    fn next_tool(&mut self, back: bool) {
        let available: Vec<Tool> = Tool::ALL
            .into_iter()
            .filter(|tool| crate::features::runs(*tool))
            .collect();
        if available.is_empty() {
            return;
        }
        let current = available
            .iter()
            .position(|tool| *tool == self.tool)
            .unwrap_or(0);
        let step = if back { available.len() - 1 } else { 1 };
        self.tool = available[(current + step) % available.len()];
    }

    /// The Target field while it has the keys. Typing goes into it and it suggests hosts under
    /// itself: the arrows pick one, right completes the field with it, Return runs it, or what
    /// is typed when none is picked. Tab picks the tool, and esc gives the keys back to the list.
    fn field_key(&mut self, key: KeyEvent) {
        let count = self.suggestions().len();
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                self.input.clear();
                self.field = false;
                self.suggestion = None;
                self.row = None;
            }
            KeyCode::Left if self.input.is_empty() => self.field = false,
            KeyCode::Down if count > 0 => {
                self.suggestion = Some(self.suggestion.map_or(0, |at| (at + 1).min(count - 1)));
            }
            KeyCode::Char('n') if control && count > 0 => {
                self.suggestion = Some(self.suggestion.map_or(0, |at| (at + 1).min(count - 1)));
            }
            KeyCode::Up => self.suggestion = self.suggestion.and_then(|at| at.checked_sub(1)),
            KeyCode::Char('p') if control => {
                self.suggestion = self.suggestion.and_then(|at| at.checked_sub(1))
            }
            // Right puts the picked host into the field, behind a tool named in front.
            KeyCode::Right => {
                if let Some(host) = self.picked() {
                    self.input = match self.typed().0 {
                        Some(tool) => format!("{} {host}", tool.command()),
                        None => host,
                    };
                    self.suggestion = None;
                }
            }
            KeyCode::Enter => match (self.picked(), self.typed().0) {
                (Some(host), Some(tool)) => {
                    self.tool = tool;
                    self.start_with(tool, &host);
                }
                (Some(host), None) => self.start(&host),
                (None, _) if self.input.trim().is_empty() => self.field = false,
                (None, _) => self.run_input(),
            },
            KeyCode::Tab => self.next_tool(false),
            KeyCode::BackTab => self.next_tool(true),
            KeyCode::Backspace => {
                self.input.pop();
                self.suggestion = None;
            }
            KeyCode::Char('u') if control => {
                self.input.clear();
                self.suggestion = None;
            }
            KeyCode::Char('?') if self.input.is_empty() => self.open_help(),
            // A space separates a tool from its target.
            KeyCode::Char(' ') if !self.input.is_empty() && !self.input.ends_with(' ') => {
                self.input.push(' ');
                self.suggestion = None;
            }
            KeyCode::Char(character) if !control && !character.is_whitespace() => {
                self.input.push(character);
                self.suggestion = None;
            }
            _ => {}
        }
    }

    /// A list's keys: every key is a command, as in k9s. A tool's key opens the Target field for
    /// that tool, `/` searches the list, and the letters that act on a row act on the selected
    /// one.
    fn start_key(&mut self, key: KeyEvent) {
        let count = self.rows().len();
        if let Some(tool) = Self::tool_of(&key) {
            self.type_target(tool);
            return;
        }
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            // Inside a case, esc and left step back out to the cases.
            KeyCode::Esc | KeyCode::Left
                if self.input.is_empty()
                    && self.search.is_empty()
                    && self.case.is_some()
                    && self.section == Section::Cases =>
            {
                self.leave_case()
            }
            // esc takes back one thing at a time: the search, what was typed, the selection.
            // With nothing left to take back it quits.
            KeyCode::Esc => {
                if !self.search.is_empty() {
                    self.search.clear();
                    self.clamp_row();
                } else if !self.input.is_empty() {
                    self.input.clear();
                    self.row = None;
                } else if self.row.is_some() {
                    self.row = None;
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('/') => self.open_search(),
            KeyCode::Char(':') => {
                let tool = self.tool;
                self.type_target(tool);
            }
            KeyCode::Char('?') => self.open_help(),
            KeyCode::Down => {
                self.row = match self.row {
                    None if count > 0 => Some(0),
                    Some(row) if row + 1 < count => Some(row + 1),
                    other => other,
                };
            }
            KeyCode::Char('j') if plain => {
                self.row = match self.row {
                    None if count > 0 => Some(0),
                    Some(row) if row + 1 < count => Some(row + 1),
                    other => other,
                };
            }
            KeyCode::Up | KeyCode::Char('k') if plain => {
                self.row = match self.row {
                    Some(0) | None => None,
                    Some(row) => Some(row - 1),
                };
            }
            KeyCode::Home | KeyCode::Char('g') if plain && count > 0 => self.row = Some(0),
            KeyCode::End | KeyCode::Char('G') if count > 0 => self.row = Some(count - 1),
            KeyCode::PageDown if count > 0 => {
                self.row = Some(self.row.map_or(0, |row| row + self.page).min(count - 1));
            }
            KeyCode::PageUp => {
                self.row = match self.row {
                    Some(row) if row > 0 => Some(row.saturating_sub(self.page)),
                    _ => None,
                };
            }
            // With a row selected, letters act on it. s saves a run or removes it from saved runs.
            KeyCode::Char('s') if self.row.is_some() => {
                if let Some(run) = self.selected_run() {
                    self.toggle_save_of(&run);
                    self.clamp_row();
                }
            }
            // r picks a tool to run against the row's host. A run's own tool is selected first.
            KeyCode::Char('r') if self.row.is_some() => {
                if let Some(host) = self.selected_host() {
                    let tool = self.selected_run().map_or(self.tool, |run| run.tool);
                    self.open_launcher(&host, Some(tool));
                }
            }
            // a puts the run in cases.
            KeyCode::Char('a') if self.row.is_some() => {
                if let Some(run) = self.selected_run() {
                    self.open_picker(run);
                }
            }
            // S shares it.
            KeyCode::Char('S') if self.row.is_some() => {
                if let Some(run) = self.selected_run() {
                    self.open_share(run);
                }
            }
            // Inside a case, x takes the run out of that case only. It stays saved.
            KeyCode::Char('x') if self.row.is_some() => {
                let case = self
                    .open_case()
                    .map(|case| (case.id.clone(), case.name.clone()));
                if let (Some((id, name)), Some(run)) = (case, self.selected_run()) {
                    match self.cases.remove(&id, &run.id) {
                        Ok(_) => self.toast(format!("Removed from {name}. It stays saved.")),
                        Err(error) => self.toast(format!("Couldn't save the case ({error}).")),
                    }
                    self.clamp_row();
                }
            }
            // f makes the row's host a favorite, or stops it being one.
            KeyCode::Char('f') if self.row.is_some() => {
                if let Some(host) = self.selected_host() {
                    self.toggle_favorite(&host);
                    self.clamp_row();
                }
            }
            // Left steps back out of the list: the selection goes.
            KeyCode::Left => self.row = None,
            // With nothing selected, tab picks the tool, as it does in the field.
            KeyCode::Tab if self.row.is_none() => self.next_tool(false),
            KeyCode::BackTab if self.row.is_none() => self.next_tool(true),
            // Right goes into the selected entry, as Return does.
            KeyCode::Right
                if self.row.is_none() && !(self.input.is_empty() && self.shown.is_some()) => {}
            KeyCode::Enter | KeyCode::Right => match (self.row, self.rows()) {
                (Some(row), Rows::Runs(runs)) => {
                    if let Some(run) = runs.get(row) {
                        let run = (*run).clone();
                        if let Some(Shown::Live { session, .. }) = &self.shown {
                            session.stop();
                        }
                        self.shown = Some(Shown::Saved(Box::new(run)));
                        self.dns_tab = DnsTab::Overview;
                        self.dns_row = 0;
                        self.dns_source = 0;
                        self.pane = Pane::Result;
                        self.hop = 0;
                        self.scroll = 0;
                    }
                }
                // A case: going into it lists its runs, the first selected.
                (Some(row), Rows::Cases(entries)) => {
                    if let Some(entry) = entries.get(row) {
                        let scope = entry.map_or(Scope::All, |case| Scope::Case(case.id.clone()));
                        self.case = Some(scope);
                        self.row = (self.rows().len() > 0).then_some(0);
                    }
                }
                // A favorite is a host: going into it runs the selected tool against it.
                (Some(row), Rows::Hosts(hosts)) => {
                    if let Some(host) = hosts.get(row) {
                        let host = (*host).to_string();
                        self.start(&host);
                    }
                }
                // Nothing typed and nothing selected: back to the run that is open.
                (None, _) if self.input.is_empty() && self.shown.is_some() => {
                    self.pane = Pane::Result
                }
                (None, _) if !self.input.trim().is_empty() => self.run_input(),
                // Return on an empty Console opens the field, for whoever expects to type.
                (None, _) if key.code == KeyCode::Enter => {
                    let tool = self.tool;
                    self.type_target(tool);
                }
                (None, _) => {}
            },
            _ => {}
        }
    }

    fn result_key(&mut self, key: KeyEvent) {
        let Some(run) = self.run() else {
            self.pane = Pane::Start;
            return;
        };
        // S shares the open run, whatever its tool, once it has ended.
        if key.code == KeyCode::Char('S') && !run.is_running() {
            let run = run.clone();
            self.open_share(run);
            return;
        }
        // The keys every result has: the field for a new target, help, and the search.
        match key.code {
            KeyCode::Char(':') => {
                let tool = self.tool;
                self.type_target(tool);
                return;
            }
            KeyCode::Char('?') => {
                self.open_help();
                return;
            }
            KeyCode::Char('/') => {
                if self.can_search() {
                    self.open_search();
                }
                return;
            }
            // With the details closed, esc drops the search before it leaves the run.
            KeyCode::Esc if !self.details && !self.search.is_empty() => {
                self.search.clear();
                return;
            }
            _ => {}
        }
        if run.is_report_only() {
            self.report_key(key);
            return;
        }
        if run.tool == Tool::Dns {
            self.dns_key(key);
            return;
        }
        if run.tool == Tool::Bgp {
            self.bgp_key(key);
            return;
        }
        let is_ping = run.tool == Tool::Ping;
        // A ping has packets and no hops to select.
        let hops = if is_ping { 0 } else { run.path_len() };
        let running = run.is_running();
        let target = run.target.clone();
        let tool = run.tool;
        let destination = run.destination;
        match key.code {
            // Tab moves the keys between the hops and the open details.
            KeyCode::Tab if self.details => {
                self.details_focus = !self.details_focus;
                self.action = 0;
            }
            // Left steps back out: from the details to the hops, then closes the details, then
            // leaves the run.
            KeyCode::Esc | KeyCode::Left if self.details_focus => self.details_focus = false,
            KeyCode::Esc | KeyCode::Left if self.details => self.details = false,
            KeyCode::Esc | KeyCode::Left => self.pane = Pane::Start,
            // d shows the selected hop's details beside the hops, as k9s describes, and hides
            // them again.
            KeyCode::Char('d') if hops > 0 => {
                self.details = !self.details;
                self.details_focus = false;
            }
            // f keeps a host in Favorites: the selected hop while its details are open, the
            // run's target otherwise.
            KeyCode::Char('f') if self.details && hops > 0 => self.hop_action(Action::Favorite),
            KeyCode::Char('f') => self.toggle_favorite(&target),
            // In the details column the arrows pick an action and Return does it.
            KeyCode::Down | KeyCode::Char('j') if self.details_focus => {
                let count = self
                    .run()
                    .and_then(|run| {
                        run.path()
                            .get(self.hop)
                            .map(|hop| actions_for(run, hop).len())
                    })
                    .unwrap_or(0);
                self.action = (self.action + 1).min(count.saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') if self.details_focus => {
                self.action = self.action.saturating_sub(1)
            }
            KeyCode::Enter | KeyCode::Right if self.details_focus => {
                let chosen = self.run().and_then(|run| {
                    run.path()
                        .get(self.hop)
                        .and_then(|hop| actions_for(run, hop).get(self.action).copied())
                });
                if let Some(action) = chosen {
                    self.hop_action(action);
                }
            }
            KeyCode::Char('t') if hops > 0 => self.hop_action(Action::Trace),
            KeyCode::Char('m') if hops > 0 => self.hop_action(Action::Mtr),
            KeyCode::Char('p') if hops > 0 => self.hop_action(Action::Ping),
            KeyCode::Char('b') if hops > 0 => self.hop_action(Action::Bgp),
            // An MTR shows its figures, or gives the room to each hop's rounds.
            KeyCode::Char('v') if tool == Tool::Mtr => self.mtr_history = !self.mtr_history,
            KeyCode::Char('n') if hops > 0 => self.hop_action(Action::CopyHostname),
            KeyCode::Char('w') if hops > 0 => self.hop_action(Action::Whois),
            KeyCode::Char('c') if is_ping => {
                if let Some(address) = destination {
                    self.copy(&address.to_string());
                }
            }
            // The arrows move through the hops a search leaves, or through every hop.
            KeyCode::Down | KeyCode::Char('j') if hops > 0 => self.step_hop(1),
            KeyCode::Up | KeyCode::Char('k') if hops > 0 => self.step_hop(-1),
            KeyCode::PageDown if hops > 0 => self.step_hop(self.page as isize),
            KeyCode::PageUp if hops > 0 => self.step_hop(-(self.page as isize)),
            KeyCode::Home | KeyCode::Char('g') if hops > 0 => self.step_hop(-(hops as isize)),
            KeyCode::End | KeyCode::Char('G') if hops > 0 => self.step_hop(hops as isize),
            // The hop's details open beside the table, and the hops stay navigable.
            KeyCode::Enter | KeyCode::Right if hops > 0 => self.details = true,
            KeyCode::Char('R') => self.start_with(tool, &target),
            KeyCode::Char('r') => self.open_result_launcher(),
            // s stops a run while it runs and saves it once it has ended: a running run can't
            // be saved, so the key never means both at once.
            KeyCode::Char('s') if running => {
                if let Some(Shown::Live { session, .. }) = &self.shown {
                    session.stop();
                }
            }
            KeyCode::Char('s') => self.toggle_save(),
            KeyCode::Char('a') if !running => {
                if let Some(run) = self.run().cloned() {
                    self.open_picker(run);
                }
            }
            KeyCode::Char('c') if hops > 0 => {
                if let Some(address) = run.path()[self.hop.min(hops - 1)].address() {
                    self.copy(&address.to_string());
                }
            }
            _ => {}
        }
    }

    /// The record the selection is on, in the tabs that list records.
    fn dns_record(&self) -> Option<Record> {
        let dns = self.run()?.dns.as_ref()?;
        dns_records(dns, self.dns_tab, self.dns_source, &self.search_words())
            .into_iter()
            .nth(self.dns_row)
    }

    fn set_dns_tab(&mut self, tab: DnsTab) {
        if self.dns_tab != tab {
            self.dns_tab = tab;
            self.dns_row = 0;
            self.details = false;
            self.details_focus = false;
        }
    }

    /// Keys of a DNS result: the tabs, the rows in them, and a record's details.
    /// A run shown from its report: nothing in it to select, so the keys are the run's own.
    fn report_key(&mut self, key: KeyEvent) {
        let Some(run) = self.run() else { return };
        let (tool, target) = (run.tool, run.target.clone());
        match key.code {
            KeyCode::Esc | KeyCode::Left => self.pane = Pane::Start,
            KeyCode::Char('f') => self.toggle_favorite(&target),
            KeyCode::Char('R') if crate::features::runs(tool) => self.start_with(tool, &target),
            KeyCode::Char('r') => self.open_result_launcher(),
            KeyCode::Char('s') => self.toggle_save(),
            KeyCode::Char('a') => {
                if let Some(run) = self.run().cloned() {
                    self.open_picker(run);
                }
            }
            _ => {}
        }
    }

    fn dns_key(&mut self, key: KeyEvent) {
        let Some(run) = self.run() else { return };
        let running = run.is_running();
        let target = run.target.clone();
        let dns = run
            .dns
            .clone()
            .unwrap_or_else(|| DnsResult::new(&run.target));
        let sources = dns.ordered_sources();
        self.dns_source = self.dns_source.min(sources.len().saturating_sub(1));
        let records = dns_records(&dns, self.dns_tab, self.dns_source, &self.search_words());
        let rows = match self.dns_tab {
            DnsTab::Overview | DnsTab::Records => records.len(),
            DnsTab::Compare => sources.len(),
            DnsTab::Trace => dns.path.len(),
        };
        self.dns_row = self.dns_row.min(rows.saturating_sub(1));
        let record = records.get(self.dns_row).cloned();
        let actions = record.as_ref().map(record_actions).unwrap_or_default();
        match key.code {
            // Tab moves the keys between the records and the open details. Without details it
            // goes to the next tab.
            KeyCode::Tab if self.details => {
                self.details_focus = !self.details_focus;
                self.action = 0;
            }
            KeyCode::Tab => {
                let at = DnsTab::ALL
                    .iter()
                    .position(|tab| *tab == self.dns_tab)
                    .unwrap_or(0);
                self.set_dns_tab(DnsTab::ALL[(at + 1) % DnsTab::ALL.len()]);
            }
            KeyCode::Char(digit @ '1'..='4') => {
                self.set_dns_tab(DnsTab::ALL[digit as usize - '1' as usize]);
            }
            KeyCode::Esc | KeyCode::Left if self.details_focus => self.details_focus = false,
            KeyCode::Esc | KeyCode::Left if self.details => self.details = false,
            KeyCode::Esc | KeyCode::Left => self.pane = Pane::Start,
            // d shows the selected record's details beside the records, and hides them again.
            KeyCode::Char('d') if record.is_some() => {
                self.details = !self.details;
                self.details_focus = false;
            }
            KeyCode::Down | KeyCode::Char('j') if self.details_focus => {
                self.action = (self.action + 1).min(actions.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') if self.details_focus => {
                self.action = self.action.saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Right if self.details_focus => {
                if let Some(action) = actions.get(self.action).copied() {
                    self.record_action(action);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.dns_row = (self.dns_row + 1).min(rows.saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => self.dns_row = self.dns_row.saturating_sub(1),
            KeyCode::PageDown => {
                self.dns_row = (self.dns_row + self.page).min(rows.saturating_sub(1));
            }
            KeyCode::PageUp => self.dns_row = self.dns_row.saturating_sub(self.page),
            KeyCode::Home | KeyCode::Char('g') => self.dns_row = 0,
            KeyCode::End | KeyCode::Char('G') => self.dns_row = rows.saturating_sub(1),
            // A record's details open beside the records. A source in Compare opens its records.
            KeyCode::Enter | KeyCode::Right if record.is_some() => self.details = true,
            KeyCode::Enter | KeyCode::Right if self.dns_tab == DnsTab::Compare && rows > 0 => {
                let source = self.dns_row;
                self.set_dns_tab(DnsTab::Records);
                self.dns_source = source;
            }
            KeyCode::Char('o') if self.dns_tab == DnsTab::Records && sources.len() > 1 => {
                self.dns_source = (self.dns_source + 1) % sources.len();
                self.dns_row = 0;
                self.details = false;
                self.details_focus = false;
            }
            KeyCode::Char('t') if record.is_some() => self.record_action(Action::Trace),
            KeyCode::Char('m') if record.is_some() => self.record_action(Action::Mtr),
            KeyCode::Char('p') if record.is_some() => self.record_action(Action::Ping),
            KeyCode::Char('b') if record.is_some() => self.record_action(Action::Bgp),
            KeyCode::Char('z') if record.is_some() => self.record_action(Action::CopyZoneLine),
            KeyCode::Char('c') => match self.dns_tab {
                DnsTab::Overview | DnsTab::Records => self.record_action(Action::CopyValue),
                DnsTab::Compare => {
                    if let Some(source) = sources.get(self.dns_row) {
                        let server = source.server.clone();
                        self.copy(&server);
                    }
                }
                DnsTab::Trace => {
                    if let Some(step) = dns.path.get(self.dns_row) {
                        let address = step.address.clone();
                        self.copy(&address);
                    }
                }
            },
            KeyCode::Char('f') => self.toggle_favorite(&target),
            KeyCode::Char('R') => self.start_with(Tool::Dns, &target),
            KeyCode::Char('r') => self.open_result_launcher(),
            KeyCode::Char('s') if running => {
                if let Some(Shown::Live { session, .. }) = &self.shown {
                    session.stop();
                }
            }
            KeyCode::Char('s') => self.toggle_save(),
            KeyCode::Char('a') if !running => {
                if let Some(run) = self.run().cloned() {
                    self.open_picker(run);
                }
            }
            _ => {}
        }
    }

    /// Does one of the things the details offer with the selected record.
    fn record_action(&mut self, action: Action) {
        let Some(record) = self.dns_record() else {
            return;
        };
        if !record_actions(&record).contains(&action) {
            return;
        }
        match action {
            Action::Trace | Action::Mtr | Action::Ping => {
                let tool = match action {
                    Action::Ping => Tool::Ping,
                    Action::Mtr => Tool::Mtr,
                    _ => Tool::Traceroute,
                };
                if let Some(host) = tool_host(&record) {
                    self.start_with(tool, &host);
                }
            }
            Action::Bgp => {
                if let Some(host) = tool_host(&record) {
                    self.start_with(Tool::Bgp, &host);
                }
            }
            Action::CopyValue => self.copy(&record.value),
            Action::CopyZoneLine => self.copy(&zone_line(&record)),
            _ => {}
        }
    }

    /// Does one of the things the details offer with the selected hop's host.
    fn hop_action(&mut self, action: Action) {
        let Some(run) = self.run() else { return };
        let Some(hop) = run.path().get(self.hop) else {
            return;
        };
        // Favorite is the heart behind the host, not a listed action, so it is let through.
        if action != Action::Favorite && !actions_for(run, hop).contains(&action) {
            return;
        }
        let Some(address) = hop.address() else { return };
        let name = run.name(address).map(str::to_string);
        match action {
            Action::Trace | Action::Mtr | Action::Ping => {
                let tool = match action {
                    Action::Ping => Tool::Ping,
                    Action::Mtr => Tool::Mtr,
                    _ => Tool::Traceroute,
                };
                let target = name.unwrap_or_else(|| address.to_string());
                self.start_with(tool, &target);
            }
            // BGP looks up the route the hop's address falls in.
            Action::Bgp => self.start_with(Tool::Bgp, &address.to_string()),
            Action::Favorite => {
                let host = name.unwrap_or_else(|| address.to_string());
                self.toggle_favorite(&host);
            }
            // A record's actions. A hop has neither.
            Action::CopyValue | Action::CopyZoneLine => {}
            Action::CopyAddress => self.copy(&address.to_string()),
            Action::CopyHostname => {
                if let Some(name) = name {
                    self.copy(&name);
                }
            }
            Action::Whois => {
                let url = format!("https://stat.ripe.net/{address}");
                if open_in_browser(&url) {
                    self.toast("Opened RIPEstat in the browser");
                } else {
                    self.copy(&url);
                }
            }
        }
    }

    /// Opens the run picker for the host the result's selection names. On the run's own
    /// target, its tool is selected first.
    fn open_result_launcher(&mut self) {
        let Some(run) = self.run() else { return };
        let tool = run.tool;
        let own = run.target.clone();
        if let Some(host) = self.selected_result_host() {
            let first = (host == own).then_some(tool);
            self.open_launcher(&host, first);
        }
    }

    fn copy(&mut self, text: &str) {
        copy_to_clipboard(text);
        self.toast(format!("Copied {text}"));
    }

    // Drawing

    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        if area.width < 40 || area.height < 10 {
            frame.render_widget(
                Paragraph::new("The Console needs at least 40 columns and 10 rows."),
                area,
            );
            return;
        }
        // A short terminal gives the header's rows to the result.
        let header_rows = if area.height >= 22 { 6 } else { 0 };
        let on_start = self.pane == Pane::Start || self.shown.is_none();
        let prompt_rows = if on_start && self.section != Section::Account {
            3
        } else {
            0
        };
        let [header, prompt, main, crumbs] = Layout::vertical([
            Constraint::Length(header_rows),
            Constraint::Length(prompt_rows),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .areas(area);
        // The panel's frame, its header row and a row of context stay when the list pages.
        self.page = usize::from(main.height).saturating_sub(4).max(1);
        if header_rows > 0 {
            self.draw_header(frame, header);
        }
        if on_start && self.section == Section::Account {
            let block = panel("Account", "this install", "");
            let inner = block.inner(main);
            frame.render_widget(block, main);
            self.account.draw(frame, inner, self.frame, LABEL);
        } else if on_start {
            self.draw_prompt(frame, prompt);
            self.draw_recent(frame, main);
            self.draw_suggestions(frame, prompt, main);
        } else {
            self.draw_run(frame, main);
        }
        self.draw_crumbs(frame, crumbs, header_rows == 0);
        if self.picker.is_some() {
            self.draw_picker(frame, area);
        }
        if self.launcher.is_some() {
            self.draw_launcher(frame, area);
        }
        if self.share.is_some() {
            self.draw_share(frame, area);
        }
        if self.help.is_some() {
            self.draw_help(frame, area);
        }
    }

    /// The case picker, over the middle of the screen: every case with a check where it holds
    /// the run, then the New case row.
    fn draw_picker(&self, frame: &mut Frame, area: Rect) {
        let Some(picker) = &self.picker else { return };
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let count = self.cases.cases.len();
        let inside = self.cases.holding(&picker.run.id).count();
        let box_width = 60.min(area.width.saturating_sub(4));
        // Cases scroll; the New case row always shows under them.
        let room = usize::from(area.height.saturating_sub(10)).max(1);
        let slots = count.min(room);
        let box_height = (slots as u16 + 5).min(area.height.saturating_sub(2));
        let rect = Rect {
            x: area.x + (area.width - box_width) / 2,
            y: area.y + (area.height - box_height) / 2,
            width: box_width,
            height: box_height,
        };
        let block = panel("Cases", &picker.run.target, &inside.to_string());
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        let columns = usize::from(inner.width);
        let first = (picker.row.min(count.saturating_sub(1)) + 1).saturating_sub(slots);
        let mut lines: Vec<Line> = Vec::new();
        for (index, case) in self.cases.cases.iter().enumerate().skip(first).take(slots) {
            let check = if case.holds(&picker.run.id) {
                "[\u{2713}]"
            } else {
                "[ ]"
            };
            let text = fit(&format!(" {check} {}", case.name), columns);
            lines.push(if picker.row == index {
                Line::styled(text, selected())
            } else {
                Line::raw(text)
            });
        }
        let on_new = picker.row >= count;
        let lead = " +  New case: ";
        let new_row = if picker.name.is_empty() && !on_new {
            Line::from(vec![Span::raw(lead), Span::styled("type a name", dim)])
        } else {
            Line::raw(fit(
                &format!(
                    "{lead}{}",
                    tail(&picker.name, columns.saturating_sub(lead.len() + 1))
                ),
                columns,
            ))
        };
        lines.push(if on_new {
            new_row.style(selected())
        } else {
            new_row
        });
        frame.render_widget(Paragraph::new(lines), inner);
        if on_new && self.help.is_none() {
            let typed = tail(&picker.name, columns.saturating_sub(lead.len() + 1));
            let row = slots as u16;
            frame.set_cursor_position(Position::new(
                inner.x + (width(lead) + width(&typed)) as u16,
                inner.y + row,
            ));
        }
    }

    /// The keys that do something right now, most useful first.
    fn keys(&self) -> Vec<(&'static str, &'static str)> {
        if self.help.is_some() {
            return self.help_keys();
        }
        if let Some(sheet) = &self.share {
            return sheet.keys();
        }
        if self.searching {
            return vec![
                ("type", "Search"),
                ("\u{21B5}", "Keep"),
                ("\u{2191}\u{2193}", "Move"),
                ("esc", "Clear"),
            ];
        }
        if self.launcher.is_some() {
            return vec![
                ("\u{2191}\u{2193}", "Tool"),
                ("\u{21B5}", "Run"),
                ("\u{2190}", "Close"),
            ];
        }
        if let Some(picker) = &self.picker {
            let on_new = picker.row >= self.cases.cases.len();
            return vec![
                ("\u{2191}\u{2193}", "Case"),
                if on_new {
                    ("\u{21B5}", "Create")
                } else {
                    ("space", "Check")
                },
                ("type", "New Case"),
                ("\u{2190}", "Close"),
            ];
        }
        if self.pane == Pane::Result {
            if let Some(run) = self.run().filter(|run| run.is_report_only()) {
                let mut keys = vec![("s", "Save"), ("a", "Add to Case"), ("f", "Favorite")];
                if self.can_share() {
                    keys.push(("S", "Share"));
                }
                if crate::features::runs(run.tool) {
                    keys.push(("R", "Run Again"));
                }
                keys.extend([("r", "Run"), ("\u{2190}", "Back"), ("?", "Help")]);
                return keys;
            }
        }
        let is_bgp = self.run().is_some_and(|run| run.tool == Tool::Bgp);
        if is_bgp && self.pane == Pane::Result {
            return self.bgp_keys();
        }
        let is_dns = self.run().is_some_and(|run| run.tool == Tool::Dns);
        if self.details && self.pane == Pane::Result && is_dns {
            return if self.details_focus {
                vec![
                    ("\u{2191}\u{2193}", "Action"),
                    ("\u{21B5}", "Do It"),
                    ("tab", "Records"),
                    ("\u{2190}", "Records"),
                ]
            } else {
                vec![
                    ("\u{2191}\u{2193}", "Record"),
                    ("tab", "Details"),
                    ("c", "Copy Value"),
                    ("z", "Copy as Zone Line"),
                    ("d", "Hide Details"),
                    ("\u{2190}", "Close"),
                ]
            };
        }
        if self.details && self.pane == Pane::Result {
            // The hop's details are open. Tab moves between the hops and the details.
            return if self.details_focus {
                vec![
                    ("\u{2191}\u{2193}", "Action"),
                    ("\u{21B5}", "Do It"),
                    ("tab", "Hops"),
                    ("\u{2190}", "Hops"),
                ]
            } else {
                vec![
                    ("\u{2191}\u{2193}", "Hop"),
                    ("tab", "Details"),
                    ("c", "Copy Address"),
                    ("d", "Hide Details"),
                    ("\u{2190}", "Close"),
                ]
            };
        }
        if self.section == Section::Account && (self.pane == Pane::Start || self.shown.is_none()) {
            let mut keys = self.account.keys();
            if !self.account.is_busy() {
                keys.push(("\u{2190}", "Console"));
            }
            return keys;
        }
        if (self.pane == Pane::Start || self.shown.is_none()) && self.field {
            let mut keys = vec![("\u{21B5}", "Run")];
            if !self.suggestions().is_empty() {
                keys.push(("\u{2191}\u{2193}", "Suggestion"));
            }
            if self.picked().is_some() {
                keys.push(("\u{2192}", "Complete"));
            }
            keys.push(("tab", "Tool"));
            if self.input.is_empty() {
                keys.push(("?", "Help"));
            }
            keys.push(("esc", "Close"));
            return keys;
        }
        if self.pane == Pane::Start || self.shown.is_none() {
            let rows = self.rows();
            let on_row = self.row.is_some();
            let enter = match (&rows, self.row, self.input.is_empty()) {
                (Rows::Runs(_) | Rows::Cases(_), Some(_), _) => "Open",
                (Rows::Hosts(_), Some(_), _) => "Run",
                (_, None, true) if self.shown.is_some() => "Result",
                (_, None, true) => "Target",
                _ => "Run",
            };
            // Right goes in and left comes back out, everywhere in the Console.
            let mut keys = vec![(if on_row { "\u{2192}" } else { "\u{21B5}" }, enter)];
            if rows.len() > 0 || !self.search.is_empty() {
                keys.push(("/", "Search"));
            }
            if rows.len() > 0 {
                keys.push((
                    "\u{2191}\u{2193}",
                    match (self.section, &rows) {
                        (Section::Console, _) => "Recent",
                        (Section::Favorites, _) => "Favorites",
                        (_, Rows::Cases(_)) => "Cases",
                        _ => "Saved",
                    },
                ));
            }
            let is_run = matches!(rows, Rows::Runs(_));
            if on_row && is_run {
                let kept = self
                    .selected_run()
                    .is_some_and(|run| self.saved.contains(&run.id));
                keys.push(("s", if kept { "Unsave" } else { "Save" }));
                keys.push(("a", "Add to Case"));
                if self.can_share() {
                    keys.push(("S", "Share"));
                }
                if self.open_case().is_some() {
                    keys.push(("x", "Remove from Case"));
                }
            }
            if on_row && !matches!(rows, Rows::Cases(_)) {
                keys.push(("r", "Run Tool"));
                let favorite = self
                    .selected_host()
                    .is_some_and(|host| self.favorites.contains(&host));
                keys.push((
                    "f",
                    if favorite {
                        "Remove Favorite"
                    } else {
                        "Favorite"
                    },
                ));
            }
            if self.case.is_some() && self.section == Section::Cases {
                keys.push(("\u{2190}", "Cases"));
            }
            keys.push(("?", "Help"));
            let in_case = self.case.is_some() && self.section == Section::Cases;
            keys.push((
                "esc",
                if !self.search.is_empty() || !self.input.is_empty() {
                    "Clear"
                } else if in_case {
                    "Back"
                } else if self.row.is_none() {
                    "Quit"
                } else {
                    "Clear"
                },
            ));
            return keys;
        }
        let running = self.run().is_some_and(Run::is_running);
        let mut keys = if is_dns && self.pane == Pane::Result {
            let mut keys = vec![("1-4", "Tab")];
            match self.dns_tab {
                DnsTab::Overview | DnsTab::Records => {
                    keys.push(("\u{2191}\u{2193}", "Record"));
                    keys.push(("d", "Details"));
                    keys.push(("/", "Search"));
                    keys.push(("c", "Copy Value"));
                    if self.dns_tab == DnsTab::Records {
                        keys.push(("o", "Next Source"));
                    }
                }
                DnsTab::Compare => {
                    keys.push(("\u{2191}\u{2193}", "Source"));
                    keys.push(("\u{2192}", "Records"));
                    keys.push(("c", "Copy Address"));
                }
                DnsTab::Trace => {
                    keys.push(("\u{2191}\u{2193}", "Step"));
                    keys.push(("c", "Copy Address"));
                }
            }
            keys
        } else if self.run().is_some_and(|run| run.tool == Tool::Ping) {
            vec![("c", "Copy Address")]
        } else {
            vec![
                ("\u{2191}\u{2193}", "Hop"),
                ("d", "Details"),
                ("/", "Search"),
                ("c", "Copy Address"),
            ]
        };
        if self.run().is_some_and(|run| run.tool == Tool::Mtr) {
            keys.push((
                "v",
                if self.mtr_history {
                    "Figures"
                } else {
                    "Rounds"
                },
            ));
        }
        if running {
            keys.push(("r", "Run Tool"));
            keys.push(("s", "Stop"));
        } else {
            keys.push(("R", "Run Again"));
            keys.push(("r", "Run Tool"));
            let favorite = self
                .run()
                .is_some_and(|run| self.favorites.contains(&run.target));
            keys.push((
                "f",
                if favorite {
                    "Remove Favorite"
                } else {
                    "Favorite"
                },
            ));
            keys.push(("s", if self.is_saved() { "Unsave" } else { "Save" }));
            keys.push(("a", "Add to Case"));
            if self.can_share() {
                keys.push(("S", "Share"));
            }
        }
        if !self.search.is_empty() {
            keys.push(("esc", "Clear Search"));
        }
        keys.push(("\u{2190}", "Back"));
        keys
    }

    /// The facts at the top left: where runs come from, and what is known about the open one.
    fn info(&self) -> Vec<(&'static str, Line<'static>)> {
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let value = |text: String| Line::styled(text, bold);
        let on_start = self.pane == Pane::Start || self.shown.is_none();
        let Some(shown) = self.shown.as_ref().filter(|_| !on_start) else {
            let count = self.history.runs.len();
            return vec![
                ("Origin", value("this device".into())),
                (
                    "Tool",
                    Line::styled(self.tool.command().to_string(), bold.fg(TOOL)),
                ),
                (
                    "History",
                    value(if count == 1 {
                        "1 run".into()
                    } else {
                        format!("{count} runs")
                    }),
                ),
                ("Account", {
                    let (text, signed_in) = self.account.header();
                    if signed_in {
                        value(text)
                    } else {
                        Line::styled(text, Paint::new().add_modifier(Modifier::DIM))
                    }
                }),
                ("rvrs", {
                    let mut line = value(crate::help::VERSION.into());
                    // What the daily look found, if rvrs has fallen behind.
                    if let Some(newer) = &self.update {
                        line.spans.push(Span::styled(
                            format!(" \u{00B7} {newer}"),
                            Paint::new().fg(Color::Yellow),
                        ));
                    }
                    line
                }),
            ];
        };
        let run = shown.run();
        let dash = || "\u{2013}".to_string();
        let address = run.destination.map_or_else(dash, |address| {
            format!(
                "{address} \u{00B7} {}",
                rvrs_core::Family::of(address).label()
            )
        });
        let network = run
            .destination
            .and_then(|address| run.network(address))
            .map_or_else(dash, |network| {
                format!("{} {}", network.label(), network.name)
            });
        let style = Style { color: true };
        let status = if let Some(report) = run.report.as_deref().filter(|_| run.is_report_only()) {
            // The maker's status line, painted by how the run ended.
            match report.outcome.as_str() {
                "reached" => style.bold_green(&report.summary),
                "failed" => style.red("Failed"),
                _ => style.yellow(&report.summary),
            }
        } else if run.tool == Tool::Bgp {
            self.bgp_status(run, style)
        } else if run.tool == Tool::Dns {
            let found = run.dns.as_ref().filter(|dns| !dns.is_empty());
            if run.is_running() {
                let text = match found {
                    Some(dns) if dns.pending_sources == 1 => "1 source still answering".to_string(),
                    Some(dns) if dns.pending_sources > 1 => {
                        format!("{} sources still answering", dns.pending_sources)
                    }
                    Some(_) => "Walking the delegation from the root\u{2026}".to_string(),
                    None => "Looking up\u{2026}".to_string(),
                };
                format!(
                    "{} {text}",
                    style.blue(SPINNER[(self.frame / 2) % SPINNER.len()])
                )
            } else if matches!(run.outcome, Some(Outcome::Failed(_))) {
                style.red("Failed")
            } else {
                match found {
                    None => style.yellow("No source answered"),
                    Some(dns) => match dns.verdict().grade {
                        VerdictGrade::Good => style.bold_green(&dns.headline()),
                        VerdictGrade::Attention => style.yellow(&dns.headline()),
                        VerdictGrade::Problem => style.red(&dns.headline()),
                    },
                }
            }
        } else if run.tool == Tool::Mtr {
            let reached = run.destination_hop().map(|hop| hop.stats());
            if run.is_running() {
                let text = match &reached {
                    Some(_) => format!("Running \u{00B7} {}", run.mtr_summary()),
                    None if run.path_len() == 0 => "Running".to_string(),
                    None => format!("Running \u{00B7} {} hops so far", run.path_len()),
                };
                format!(
                    "{} {text}",
                    style.blue(SPINNER[(self.frame / 2) % SPINNER.len()])
                )
            } else if matches!(run.outcome, Some(Outcome::Failed(_))) {
                style.red("Failed")
            } else {
                match reached {
                    Some(stats) if stats.lost() == 0 => style.bold_green(&run.mtr_summary()),
                    Some(_) => style.yellow(&run.mtr_summary()),
                    None => style.yellow(&run.summary()),
                }
            }
        } else if run.tool == Tool::Ping {
            let stats = run.ping_stats();
            if run.is_running() {
                let text = if stats.sent == 0 {
                    "Running".to_string()
                } else {
                    format!("Running \u{00B7} {}", run.packet_summary())
                };
                format!(
                    "{} {text}",
                    style.blue(SPINNER[(self.frame / 2) % SPINNER.len()])
                )
            } else if matches!(run.outcome, Some(Outcome::Failed(_))) {
                style.red("Failed")
            } else if stats.received == 0 {
                style.yellow(&format!("No reply \u{00B7} {}", run.packet_summary()))
            } else if stats.lost() > 0 {
                style.yellow(&run.packet_summary())
            } else {
                style.bold_green(&run.packet_summary())
            }
        } else {
            let (elapsed, looking_up) = match shown {
                Shown::Live {
                    session, elapsed, ..
                } => (*elapsed, !run.is_running() && session.pending_lookups() > 0),
                Shown::Saved(_) => (None, false),
            };
            let view = View {
                run,
                style,
                width: 160,
                frame: self.frame / 2,
                elapsed,
                looking_up,
            };
            pretty::footer(&view).into_iter().next().unwrap_or_default()
        };
        let mut info = vec![
            ("Origin", value("this device".into())),
            (
                "Source",
                value(match (run.source, run.public_source) {
                    (Some(source), Some(public)) if source != public => {
                        format!("{source} \u{00B7} public {public}")
                    }
                    (Some(source), _) => source.to_string(),
                    (None, Some(public)) => format!("public {public}"),
                    (None, None) => dash(),
                }),
            ),
            ("Target", {
                let mut line = value(run.target.clone());
                if self.favorites.contains(&run.target) {
                    line.spans
                        .push(Span::styled(" \u{2665}", Paint::new().fg(TOOL)));
                }
                line
            }),
            ("Address", value(address)),
            ("Network", value(network)),
            ("Status", ansi_line(&status)),
        ];
        // A lookup has no one address or network. It has a resolver and a DNSSEC state.
        if let Some(dns) = run.dns.as_ref().filter(|_| run.tool == Tool::Dns) {
            let resolver = dns.device().map_or_else(dash, |device| {
                if device.label == device.server {
                    device.server.clone()
                } else {
                    format!("{} \u{00B7} {}", device.label, device.server)
                }
            });
            info[3] = ("Resolver", value(resolver));
            info[4] = (
                "DNSSEC",
                if run.is_running() {
                    value(dash())
                } else {
                    value(dns.dnssec.short().to_string())
                },
            );
        }
        // A BGP lookup probes nothing. It has who was asked, a prefix and the origin AS.
        if run.tool == Tool::Bgp && !run.is_report_only() {
            let [source, prefix, network] = Self::bgp_info(run);
            info[1] = (source.0, value(source.1));
            info[3] = (prefix.0, value(prefix.1));
            info[4] = (network.0, value(network.1));
        }
        if let Shown::Saved(saved) = shown {
            // A run another install made names it, as the list's FROM column does.
            let origin = saved.from.as_deref().unwrap_or("this device");
            info[0] = (
                "Origin",
                value(format!(
                    "{origin} \u{00B7} {}",
                    format::relative(saved.started_at, format::now())
                )),
            );
        }
        info
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let columns = usize::from(area.width);

        // The keys sit in columns of four, as k9s lays its own out. A narrow terminal gets
        // fewer, taller columns, then loses the logo, then the tools.
        let keys = self.keys();
        let tools_width = 19;
        let key_width = 25;
        let wanted = keys.len().div_ceil(4).max(1);
        let rows = usize::from(area.height).max(1);
        let needed = keys.len().div_ceil(rows).max(1);
        let show_keys = columns >= 70;
        // The tools belong to the overview, where one is chosen. An open run has no use for them.
        let on_start = self.pane == Pane::Start || self.shown.is_none();
        // The tools belong to the Target field: they show while it has the keys.
        let show_tools = on_start && self.field && columns >= 92;
        let room = columns.saturating_sub(42 + usize::from(show_tools) * tools_width);
        let key_columns = wanted
            .min(room / key_width)
            .max(needed.min(room / key_width))
            .max(1);
        let keys_width = key_columns * key_width;
        // The whole logo where it fits, the mark and the rules where that fits, else none.
        let free = columns.saturating_sub(42 + tools_width + keys_width + 2);
        let logo_width = if free >= logo_width() {
            logo_width()
        } else if free >= LOGO_CUT {
            LOGO_CUT
        } else {
            0
        };
        let show_logo = logo_width > 0;
        let taken = usize::from(show_tools) * tools_width
            + usize::from(show_keys) * keys_width
            + usize::from(show_logo) * (logo_width + 2);
        let info_width = columns.saturating_sub(taken + 2).clamp(24, 70);

        let mut x = area.x + 1;
        let column = |x: &mut u16, wide: usize| {
            let rect = Rect {
                x: *x,
                y: area.y,
                width: (wide as u16).min(area.right().saturating_sub(*x)),
                height: area.height,
            };
            *x += wide as u16;
            rect
        };

        let info: Vec<Line> = self
            .info()
            .into_iter()
            .map(|(label, value)| {
                let mut spans = vec![Span::styled(
                    format!("{:<10}", format!("{label}:")),
                    Paint::new().fg(LABEL),
                )];
                spans.extend(value.spans);
                Line::from(spans)
            })
            .collect();
        frame.render_widget(Paragraph::new(info), column(&mut x, info_width));

        if show_tools {
            // The tools in the app's order: the selected one marked, the ones still to come dimmed.
            let tools: Vec<Line> = Tool::ALL
                .into_iter()
                .map(|tool| {
                    if tool == self.tool {
                        Line::from(vec![
                            Span::styled("\u{25CF} ", Paint::new().fg(TOOL)),
                            Span::styled(
                                tool.command(),
                                Paint::new().fg(TOOL).add_modifier(Modifier::BOLD),
                            ),
                        ])
                    } else if crate::features::runs(tool) {
                        Line::from(vec![
                            Span::styled("\u{25CB} ", Paint::new().fg(TOOL)),
                            Span::raw(tool.command()),
                        ])
                    } else {
                        Line::styled(format!("  {:<8} soon", tool.command()), dim)
                    }
                })
                .collect();
            frame.render_widget(Paragraph::new(tools), column(&mut x, tools_width));
        }
        if show_keys {
            let per_column = keys.len().div_ceil(key_columns).clamp(1, rows);
            for chunk in keys.chunks(per_column).take(key_columns) {
                let lines: Vec<Line> = chunk
                    .iter()
                    .map(|(key, label)| key_line(key, label))
                    .collect();
                frame.render_widget(Paragraph::new(lines), column(&mut x, key_width));
            }
        }
        if show_logo {
            let rect = Rect {
                x: area.right().saturating_sub(logo_width as u16 + 2),
                y: area.y,
                width: logo_width as u16 + 1,
                height: area.height,
            };
            frame.render_widget(Paragraph::new(logo_lines(logo_width)), rect);
        }
    }

    /// The one field, framed like a prompt: the tool, then the target. Until a tool's key or `:`
    /// gives it the keys, it shows those keys.
    fn draw_prompt(&self, frame: &mut Frame, area: Rect) {
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let focused = self.field;
        let block = Block::bordered()
            .border_style(if focused {
                Paint::new().fg(ACCENT)
            } else {
                dim
            })
            .title(Line::styled(
                " Target ",
                Paint::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let lead = format!(" {}", self.tool.command());
        let room = usize::from(inner.width).saturating_sub(width(&lead) + 4);
        let typed = tail(&self.input, room);
        if !focused && self.input.is_empty() {
            // The keys that open the field, as k9s shows its own.
            let mut spans = vec![
                Span::styled(lead.clone(), dim.fg(TOOL)),
                Span::styled("> ", dim),
            ];
            for tool in Tool::ALL
                .into_iter()
                .filter(|tool| crate::features::runs(*tool))
            {
                spans.push(Span::styled(
                    format!("<{}>", run_picker::letter(tool)),
                    Paint::new().fg(KEY).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(format!(" {}  ", tool.command()), dim));
            }
            spans.push(Span::styled(
                "<:>",
                Paint::new().fg(KEY).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(format!(" {}", self.tool.command()), dim));
            frame.render_widget(Paragraph::new(Line::from(spans)), inner);
            return;
        }
        let content = Line::from(vec![
            Span::styled(
                lead.clone(),
                Paint::new().fg(TOOL).add_modifier(Modifier::BOLD),
            ),
            Span::styled("> ", dim),
            if self.input.is_empty() {
                Span::styled(
                    if self.tool == Tool::Bgp {
                        "Host, address, prefix or AS number. Typing also filters the list below"
                    } else {
                        "Host or address. Typing also filters the list below"
                    },
                    dim,
                )
            } else {
                Span::raw(typed.clone())
            },
        ]);
        frame.render_widget(Paragraph::new(content), inner);
        if focused && self.help.is_none() && !self.searching {
            let offset = (width(&lead) + 2 + width(&typed)) as u16;
            frame.set_cursor_position(Position::new(inner.x + offset, inner.y));
        }
    }

    /// The hosts the field suggests, in a box under it over the list: a heart on a favorite, the
    /// newest run to the host and how long ago, and the picked one highlighted.
    fn draw_suggestions(&self, frame: &mut Frame, prompt: Rect, main: Rect) {
        if !self.field {
            return;
        }
        let hosts = self.suggestions();
        if hosts.is_empty() {
            return;
        }
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let now = format::now();
        let facts: Vec<String> = hosts
            .iter()
            .map(|host| {
                self.history
                    .runs
                    .iter()
                    .find(|run| run.target.eq_ignore_ascii_case(host))
                    .map_or_else(
                        || "\u{2013}".to_string(),
                        |run| {
                            format!(
                                "{} \u{00B7} {}",
                                run.tool.command(),
                                format::relative(run.started_at, now)
                            )
                        },
                    )
            })
            .collect();
        let host_width = hosts
            .iter()
            .map(|host| width(host))
            .max()
            .unwrap_or(0)
            .min(48);
        let fact_width = facts.iter().map(|fact| width(fact)).max().unwrap_or(0);
        // Under the typed text: the box's frame and the heart's column sit left of it.
        let text_x = prompt.x + 1 + width(&format!(" {}> ", self.tool.command())) as u16;
        let x = text_x.saturating_sub(4).max(main.x);
        let wanted = (host_width + fact_width + 8) as u16;
        let rect = Rect {
            x,
            y: main.y,
            width: wanted.min(main.right().saturating_sub(x)),
            height: (hosts.len() as u16 + 2).min(main.height),
        };
        let block = Block::bordered().border_style(Paint::new().fg(ACCENT));
        let inner = block.inner(rect);
        let columns = usize::from(inner.width);
        let lines: Vec<Line> = hosts
            .iter()
            .zip(&facts)
            .enumerate()
            .map(|(index, (host, fact))| {
                let heart = if self.favorites.contains(host) {
                    "\u{2665}"
                } else {
                    " "
                };
                let host = fit(&truncate_middle(host, host_width), host_width);
                if self.suggestion == Some(index) {
                    Line::styled(
                        fit(&format!(" {heart} {host}  {fact}"), columns),
                        selected(),
                    )
                } else {
                    Line::from(vec![
                        Span::styled(format!(" {heart} "), Paint::new().fg(TOOL)),
                        Span::styled(host, bold),
                        Span::styled(format!("  {fact}"), dim),
                    ])
                }
            })
            .collect();
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// History as a table: Recent, Favorites, Cases or saved runs.
    fn draw_recent(&self, frame: &mut Frame, area: Rect) {
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let rows = self.rows();
        let name = self.list_name();
        // A search shows after the count. The field never narrows the list.
        let scope = "all".to_string();
        let block = panel_searched(&name, &scope, &rows.len().to_string(), true, &self.search);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let columns = usize::from(inner.width);
        let room = usize::from(inner.height).saturating_sub(1);
        let now = format::now();
        let mut lines: Vec<Line> = Vec::new();

        match &rows {
            rows if rows.len() == 0 => {
                let filtered = !self.search.trim().is_empty();
                let in_case = self.open_case().is_some();
                let title = match (self.section, filtered) {
                    (_, true) => "Nothing matches",
                    (Section::Console, _) => "No runs yet",
                    (Section::Cases, _) if in_case => "No runs in this case",
                    (Section::Cases, _) => "No saved runs",
                    (Section::Favorites, _) => "No favorites",
                    (Section::Account, _) => "Account",
                };
                let [_, middle, _] = Layout::vertical([
                    Constraint::Percentage(35),
                    Constraint::Length(2),
                    Constraint::Min(0),
                ])
                .areas(inner);
                frame.render_widget(
                    Paragraph::new(vec![Line::styled(title, bold)]).alignment(Alignment::Center),
                    middle,
                );
                return;
            }
            Rows::Runs(runs) => {
                let show_outcome = columns >= 84;
                let summary_width = if columns >= 70 { 36 } else { 24 };
                // Where each run was made from: the address probes left from, and the address
                // the internet saw. They show from 130 columns.
                let show_origin = columns >= 126;
                let dash = |address: Option<IpAddr>| {
                    address.map_or_else(|| "\u{2013}".to_string(), |address| address.to_string())
                };
                let fixed = 4
                    + 11
                    + summary_width
                    + 9
                    + if show_outcome { 10 } else { 0 }
                    + if show_origin { 34 } else { 0 };
                let target_width = columns.saturating_sub(fixed + 3).max(12);
                let mut header = format!(
                    "    {}{} {}",
                    fit("TOOL", 11),
                    fit("TARGET\u{2191}", target_width),
                    fit("SUMMARY", summary_width)
                );
                if show_outcome {
                    header.push_str(&fit("OUTCOME", 10));
                }
                if show_origin {
                    header.push_str(&format!("{}{}", fit("SOURCE", 17), fit("PUBLIC", 17)));
                }
                header.push_str(&fit_right("AGE", 9));
                lines.push(Line::styled(header, bold));
                let first = self
                    .row
                    .map_or(0, |row| (row + 1).saturating_sub(room.max(1)));
                for (index, run) in runs.iter().enumerate().skip(first).take(room.max(1)) {
                    let outcome = match &run.outcome {
                        Some(Outcome::Reached) => "Reached",
                        Some(Outcome::Exhausted) => "No reply",
                        Some(Outcome::Stopped) => "Stopped",
                        Some(Outcome::Failed(_)) => "Failed",
                        None => "Running",
                    };
                    // A star marks a saved run.
                    let star = if self.saved.contains(&run.id) {
                        "\u{2605}"
                    } else {
                        " "
                    };
                    // A heart marks a run to a favorite host.
                    let heart = if self.favorites.contains(&run.target) {
                        "\u{2665}"
                    } else {
                        " "
                    };
                    let mut text = format!(
                        " {star}{heart} {}{} {}",
                        fit(run.tool.command(), 11),
                        fit(&truncate_middle(&run.target, target_width), target_width),
                        fit(&run.summary(), summary_width)
                    );
                    if show_outcome {
                        text.push_str(&fit(outcome, 10));
                    }
                    if show_origin {
                        text.push_str(&format!(
                            "{}{}",
                            fit(&truncate_middle(&dash(run.source), 16), 17),
                            fit(&truncate_middle(&dash(run.public_source), 16), 17)
                        ));
                    }
                    text.push_str(&fit_right(&format::relative(run.started_at, now), 9));
                    let lossy = run.tool == Tool::Ping && run.ping_stats().lost() > 0;
                    let paint = if self.row == Some(index) {
                        selected()
                    } else {
                        match &run.outcome {
                            Some(Outcome::Reached) if !lossy => Paint::new(),
                            Some(Outcome::Stopped) if !lossy => dim,
                            Some(Outcome::Failed(_)) => Paint::new().fg(Color::Red),
                            None => Paint::new().fg(Color::Blue),
                            _ => Paint::new().fg(Color::Yellow),
                        }
                    };
                    lines.push(Line::styled(fit(&text, columns), paint));
                }
            }
            Rows::Cases(entries) => {
                // All saved, then each case: how many runs, how old the newest is.
                let name_width = (columns * 2 / 5).clamp(16, 60);
                lines.push(Line::styled(
                    format!(
                        "   {}{}   NEWEST RUN",
                        fit("CASE", name_width),
                        fit_right("RUNS", 6)
                    ),
                    bold,
                ));
                let first = self
                    .row
                    .map_or(0, |row| (row + 1).saturating_sub(room.max(1)));
                for (index, entry) in entries.iter().enumerate().skip(first).take(room.max(1)) {
                    let runs: Vec<&Run> = self
                        .saved
                        .runs
                        .iter()
                        .filter(|run| entry.is_none_or(|case| case.holds(&run.id)))
                        .collect();
                    let newest = runs
                        .iter()
                        .map(|run| run.started_at)
                        .max()
                        .map_or_else(|| "\u{2013}".to_string(), |at| format::relative(at, now));
                    let name = entry.map_or("All saved", |case| case.name.as_str());
                    let last = entry.is_some_and(|case| {
                        self.cases.last_used.as_deref() == Some(case.id.as_str())
                    });
                    let text = format!(
                        "   {}{}   {}{}",
                        fit(
                            &truncate_middle(name, name_width.saturating_sub(1)),
                            name_width
                        ),
                        fit_right(&runs.len().to_string(), 6),
                        fit(&newest, 10),
                        if last { "  used last" } else { "" }
                    );
                    lines.push(Line::styled(
                        fit(&text, columns),
                        if self.row == Some(index) {
                            selected()
                        } else if entry.is_none() {
                            bold
                        } else {
                            Paint::new()
                        },
                    ));
                }
            }
            Rows::Hosts(hosts) => {
                // A favorite is a host: its row shows how often it was run and how the newest went.
                let host_width = (columns * 2 / 5).clamp(16, 60);
                lines.push(Line::styled(
                    format!(
                        "   {}{}{}",
                        fit("HOST\u{2191}", host_width),
                        fit_right("RUNS", 6),
                        "   LAST RUN"
                    ),
                    bold,
                ));
                let first = self
                    .row
                    .map_or(0, |row| (row + 1).saturating_sub(room.max(1)));
                for (index, host) in hosts.iter().enumerate().skip(first).take(room.max(1)) {
                    let of_host: Vec<&Run> = self
                        .history
                        .runs
                        .iter()
                        .filter(|run| run.target.eq_ignore_ascii_case(host))
                        .collect();
                    let last = of_host.first().map_or_else(
                        || "\u{2013}".to_string(),
                        |run| {
                            format!(
                                "{} \u{00B7} {} \u{00B7} {}",
                                run.tool.command(),
                                run.summary(),
                                format::relative(run.started_at, now)
                            )
                        },
                    );
                    let text = format!(
                        " \u{2665} {}{}   {last}",
                        fit(
                            &truncate_middle(host, host_width.saturating_sub(1)),
                            host_width
                        ),
                        fit_right(&of_host.len().to_string(), 6)
                    );
                    lines.push(Line::styled(
                        fit(&text, columns),
                        if self.row == Some(index) {
                            selected()
                        } else {
                            Paint::new()
                        },
                    ));
                }
            }
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_run(&mut self, frame: &mut Frame, area: Rect) {
        let Some(shown) = &self.shown else { return };
        let run = shown.run();
        // The hop's details take a column on the right, or the lower part of a narrow terminal.
        let split = if run.tool == Tool::Dns {
            self.details && self.dns_record().is_some()
        } else if run.tool == Tool::Bgp {
            self.details && self.bgp_has_details()
        } else {
            self.details && run.tool != Tool::Ping && run.path_len() > 0
        };
        if !split {
            self.details = false;
            self.details_focus = false;
        }
        let area = if split {
            let (table, details) = if area.width >= 110 {
                let [table, details] =
                    Layout::horizontal([Constraint::Min(40), Constraint::Length(54)]).areas(area);
                (table, details)
            } else {
                let [table, details] = Layout::vertical([
                    Constraint::Min(6),
                    Constraint::Length(area.height.saturating_sub(8).min(22)),
                ])
                .areas(area);
                (table, details)
            };
            self.draw_details(frame, details);
            table
        } else {
            area
        };
        let table_focused = !self.details_focus;
        let Some(shown) = &self.shown else { return };
        let run = shown.run();
        let (name, count) =
            if let Some(report) = run.report.as_deref().filter(|_| run.is_report_only()) {
                let records = report.records.as_ref().map_or(0, Vec::len);
                let count = match (run.tool, records) {
                    (Tool::Dns, 1) => "1 record".to_string(),
                    (Tool::Dns, records) => format!("{records} records"),
                    _ => "report".to_string(),
                };
                (run.tool.title(), count)
            } else if run.tool == Tool::Bgp {
                ("BGP", Self::bgp_count(run))
            } else if run.tool == Tool::Dns {
                let records = run.dns.as_ref().map_or(0, DnsResult::answer_count);
                (
                    "DNS",
                    if records == 1 {
                        "1 record".to_string()
                    } else {
                        format!("{records} records")
                    },
                )
            } else if run.tool == Tool::Ping {
                ("Ping", format!("{} pkt", run.ping_stats().sent))
            } else if run.tool == Tool::Mtr {
                let rounds = run
                    .path()
                    .iter()
                    .map(|hop| hop.stats().sent)
                    .max()
                    .unwrap_or(0);
                ("MTR", format!("{rounds} rounds"))
            } else {
                ("Traceroute", run.path_len().to_string())
            };
        let block = panel_searched(name, &run.target, &count, table_focused, &self.search);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if let Some(Outcome::Failed(message)) = &run.outcome {
            frame.render_widget(
                Paragraph::new(Line::styled(
                    format!(" {message}"),
                    Paint::new().fg(Color::Red),
                )),
                inner,
            );
            return;
        }
        if run.is_report_only() {
            // The report's page: status line, figures, records, sources, interpretation.
            let style = Style { color: true };
            // The header's Status line already says the first line of it.
            let lines: Vec<Line> = std::iter::once(String::new())
                .chain(
                    pretty::report_lines(run, style, usize::from(inner.width).saturating_sub(2))
                        .into_iter()
                        .skip(1),
                )
                .map(|line| ansi_line(&format!(" {line}")))
                .collect();
            frame.render_widget(Paragraph::new(lines), inner);
            return;
        }
        if run.tool == Tool::Bgp {
            self.draw_bgp(run, frame, inner);
            return;
        }
        if run.tool == Tool::Dns {
            // The tabs stay put. What is under them scrolls with the selection.
            let [tabs, body] =
                Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
            let (lines, at) = self.dns_lines(run, body);
            let offset = (at + 2).saturating_sub(usize::from(body.height));
            frame.render_widget(Paragraph::new(self.dns_tabs()), tabs);
            frame.render_widget(Paragraph::new(lines).scroll((offset as u16, 0)), body);
            return;
        }
        let lines = if run.tool == Tool::Ping {
            self.ping_lines(run, inner)
        } else if run.tool == Tool::Mtr {
            self.mtr_lines(inner)
        } else {
            self.hop_lines(inner)
        };
        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// "<1> Overview  <2> Records  <3> Compare  <4> Trace", the open one marked.
    fn dns_tabs(&self) -> Line<'static> {
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let mut spans = vec![Span::raw(" ")];
        for (index, tab) in DnsTab::ALL.into_iter().enumerate() {
            spans.push(Span::styled(
                format!("<{}>", index + 1),
                Paint::new().fg(KEY).add_modifier(Modifier::BOLD),
            ));
            spans.push(if tab == self.dns_tab {
                Span::styled(
                    format!(" {} ", tab.title()),
                    selected().add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(format!(" {} ", tab.title()), dim)
            });
            spans.push(Span::raw(" "));
        }
        Line::from(spans)
    }

    /// The open tab of a DNS result, and the line its selected row is on.
    fn dns_lines(&self, run: &Run, inner: Rect) -> (Vec<Line<'static>>, usize) {
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let columns = usize::from(inner.width).max(20);
        let empty = DnsResult::new(&run.target);
        let dns = run.dns.as_ref().unwrap_or(&empty);
        let diff = Diff::of(dns);
        let running = run.is_running();
        let sentence = |text: &str, paint: Paint| -> Vec<Line<'static>> {
            pretty::wrap(text, columns.saturating_sub(2).min(100))
                .into_iter()
                .map(|line| Line::styled(format!(" {line}"), paint))
                .collect()
        };
        let header = |text: &str| Line::styled(format!(" {text}"), bold);
        let row = |text: String, index: usize| {
            if index == self.dns_row {
                Line::styled(fit(&text, columns), selected())
            } else {
                Line::raw(text)
            }
        };
        let pending = || {
            Line::styled(
                match dns.pending_sources {
                    0 => " Walking the delegation from the root\u{2026}".to_string(),
                    1 => " 1 source still answering".to_string(),
                    count => format!(" {count} sources still answering"),
                },
                dim,
            )
        };
        let mut lines: Vec<Line> = Vec::new();
        let mut at = 0;
        if dns.is_empty() {
            lines.push(Line::styled(
                if running {
                    " Waiting for the first answer\u{2026}"
                } else {
                    " No source answered"
                },
                dim,
            ));
            return (lines, 0);
        }
        match self.dns_tab {
            DnsTab::Overview => {
                if running {
                    lines.push(pending());
                } else {
                    let verdict = dns.verdict();
                    let color = match verdict.grade {
                        VerdictGrade::Good => Color::Green,
                        VerdictGrade::Attention => Color::Yellow,
                        VerdictGrade::Problem => Color::Red,
                    };
                    lines.push(Line::styled(format!(" {}", verdict.title), bold.fg(color)));
                    lines.extend(sentence(&verdict.detail, Paint::new()));
                }
                lines.push(Line::default());
                let addresses = dns.addresses();
                let v4 = addresses
                    .iter()
                    .filter(|record| record.type_code == dns::types::A)
                    .count();
                let mut tiles = vec![
                    Span::styled(" A / AAAA ", dim),
                    Span::styled(format!("{v4} / {}", addresses.len() - v4), bold),
                ];
                if let Some(ttl) = dns.address_ttl() {
                    tiles.push(Span::styled("    TTL ", dim));
                    tiles.push(Span::styled(format::ttl(ttl), bold));
                }
                if !running {
                    tiles.push(Span::styled("    DNSSEC ", dim));
                    tiles.push(Span::styled(
                        dns.dnssec.short(),
                        match dns.dnssec {
                            Dnssec::Signed => bold.fg(Color::Green),
                            Dnssec::Broken => bold.fg(Color::Red),
                            _ => bold,
                        },
                    ));
                }
                lines.push(Line::from(tiles));
                let search = self.search_words();
                let keys: Vec<&Record> = dns
                    .key_records()
                    .into_iter()
                    .filter(|record| record_found(record, &search))
                    .collect();
                if !keys.is_empty() {
                    lines.push(Line::default());
                    lines.push(header("KEY RECORDS"));
                    for (index, record) in keys.into_iter().enumerate() {
                        if index == self.dns_row {
                            at = lines.len();
                        }
                        lines.push(row(
                            crate::dig::record_row(record, Style::PLAIN, columns),
                            index,
                        ));
                    }
                }
                if !dns.path.is_empty() {
                    lines.push(Line::default());
                    lines.push(header("RESOLUTION PATH"));
                    let steps: Vec<String> = dns
                        .path
                        .iter()
                        .enumerate()
                        .map(|(index, step)| {
                            format!("{} {}", step.role(index + 1 == dns.path.len()), step.server)
                        })
                        .collect();
                    lines.extend(sentence(&steps.join(" \u{2192} "), dim));
                }
                if !running {
                    lines.push(Line::default());
                    lines.push(header("RECENT CHANGES"));
                    let previous = self.history.previous_dns(run);
                    match previous
                        .and_then(|previous| previous.dns.as_ref().map(|then| (previous, then)))
                    {
                        None => lines.extend(sentence(
                            "First saved lookup of this name. Later lookups are compared with it.",
                            dim,
                        )),
                        Some((previous, then)) => {
                            let when = format::relative(previous.started_at, format::now());
                            let when = if when == "Just now" {
                                "just now".to_string()
                            } else {
                                when
                            };
                            let changed = changes(dns, Some(then));
                            if changed.is_empty() {
                                lines.extend(sentence(
                                    &format!("Same addresses, names and nameservers as the lookup {when}."),
                                    dim,
                                ));
                            } else {
                                for change in &changed {
                                    lines.push(Line::styled(
                                        format!(
                                            " {}",
                                            truncate_end(&change.line(), columns.saturating_sub(2))
                                        ),
                                        Paint::new().fg(Color::Yellow),
                                    ));
                                }
                                lines.extend(sentence(
                                    &format!("Compared with the lookup {when}."),
                                    dim,
                                ));
                            }
                        }
                    }
                }
            }
            DnsTab::Records => {
                let sources = dns.ordered_sources();
                let chosen = self.dns_source.min(sources.len().saturating_sub(1));
                let mut chips = vec![Span::raw(" ")];
                for (index, source) in sources.iter().enumerate() {
                    let label = format!(" {} ", truncate_end(&source.label, 24));
                    chips.push(if index == chosen {
                        Span::styled(label, selected().add_modifier(Modifier::BOLD))
                    } else {
                        Span::styled(label, dim)
                    });
                }
                lines.push(Line::from(chips));
                if let Some(source) = sources.get(chosen) {
                    let mut facts = vec![source.server.clone()];
                    if let Some(rtt) = source.rtt_ms {
                        facts.push(format!("{rtt:.0} ms"));
                    }
                    facts.push("from this device".to_string());
                    if source.authentic_data {
                        facts.push("validated (AD)".to_string());
                    }
                    if source.is_authoritative {
                        facts.push("authoritative".to_string());
                    }
                    lines.push(Line::styled(format!("  {}", facts.join(" \u{00B7} ")), dim));
                    if let Some(error) = &source.error {
                        lines.push(Line::default());
                        lines.extend(sentence(error, Paint::new().fg(Color::Red)));
                    }
                    let grouped = grouped_records(source, &self.search_words());
                    let mut last = None;
                    for (index, (group, record)) in grouped.iter().enumerate() {
                        if last != Some(*group) {
                            lines.push(Line::default());
                            lines.push(header(&group.title().to_uppercase()));
                            last = Some(*group);
                        }
                        if index == self.dns_row {
                            at = lines.len();
                        }
                        lines.push(row(
                            crate::dig::record_row(record, Style::PLAIN, columns),
                            index,
                        ));
                    }
                    if grouped.is_empty() && source.error.is_none() {
                        lines.push(Line::default());
                        lines.push(Line::styled(
                            if self.search.trim().is_empty() {
                                " No records"
                            } else {
                                " No record matches"
                            },
                            dim,
                        ));
                    }
                    // Types the source refused or failed, each kind of failure once.
                    let mut failed: Vec<(&String, Vec<&String>)> = Vec::new();
                    for (type_name, failure) in &source.failures {
                        match failed.iter_mut().find(|(known, _)| *known == failure) {
                            Some((_, types)) => types.push(type_name),
                            None => failed.push((failure, vec![type_name])),
                        }
                    }
                    if !failed.is_empty() {
                        lines.push(Line::default());
                        lines.push(header("NOT ANSWERED"));
                        for (failure, types) in failed {
                            let types: Vec<&str> = types.iter().map(|name| name.as_str()).collect();
                            lines.extend(sentence(
                                &format!(" {} \u{00B7} {failure}", types.join(", ")),
                                Paint::new().fg(Color::Yellow),
                            ));
                        }
                    }
                    let secured = grouped
                        .iter()
                        .any(|(group, _)| *group == RecordGroup::Security);
                    if source.is_authoritative && !secured && !grouped.is_empty() {
                        lines.push(Line::default());
                        lines.extend(sentence(
                            "No CAA, DS, DNSKEY or RRSIG records in the zone's answer.",
                            dim,
                        ));
                    }
                }
            }
            DnsTab::Compare => {
                if running {
                    lines.push(pending());
                } else {
                    let differences = diff.difference_count();
                    lines.push(if diff.agree {
                        Line::styled(" All sources agree", bold.fg(Color::Green))
                    } else {
                        Line::styled(" Answers differ", bold.fg(Color::Yellow))
                    });
                    lines.extend(sentence(
                        &if diff.agree {
                            "Device, public resolvers and the zone agree on addresses, names, mail and nameservers.".to_string()
                        } else {
                            format!(
                                "The sources differ in {differences} place{}.",
                                if differences == 1 { "" } else { "s" }
                            )
                        },
                        dim,
                    ));
                }
                lines.push(Line::default());
                let sources = dns.ordered_sources();
                let table =
                    crate::dig::compare_rows(&sources, &diff, Style::PLAIN, columns.min(120));
                for (index, text) in table.into_iter().enumerate() {
                    let Some(source) = index.checked_sub(1).and_then(|row| sources.get(row)) else {
                        lines.push(Line::styled(text.to_uppercase(), dim));
                        continue;
                    };
                    if index - 1 == self.dns_row {
                        at = lines.len();
                        lines.push(Line::styled(fit(&text, columns), selected()));
                        continue;
                    }
                    let status = diff.status(source);
                    let color = match status {
                        SourceStatus::Match => Color::Green,
                        SourceStatus::Differs => Color::Yellow,
                        SourceStatus::Error => Color::Red,
                    };
                    let cut = text.len().saturating_sub(status.title().len());
                    lines.push(Line::from(vec![
                        Span::raw(text[..cut].to_string()),
                        Span::styled(text[cut..].to_string(), Paint::new().fg(color)),
                    ]));
                }
                if !running && !diff.notes.is_empty() {
                    lines.push(Line::default());
                    lines.push(header("DIFFERENCES"));
                    for note in &diff.notes {
                        lines.push(Line::styled(
                            format!(" {}", note.title),
                            if note.is_difference {
                                Paint::new().fg(Color::Yellow)
                            } else {
                                Paint::new()
                            },
                        ));
                        lines.extend(sentence(&note.detail, dim));
                    }
                }
            }
            DnsTab::Trace => {
                lines.push(header("DELEGATION PATH"));
                if dns.path.is_empty() {
                    lines.push(Line::styled(
                        if running {
                            "  Walking the delegation from the root\u{2026}"
                        } else {
                            "  Couldn't walk the delegation"
                        },
                        dim,
                    ));
                }
                let zones = dns
                    .path
                    .iter()
                    .map(|step| step.zone.len())
                    .max()
                    .unwrap_or(1)
                    .min(32);
                for (index, step) in dns.path.iter().enumerate() {
                    let mut detail = step.detail();
                    if step.ds_present == Some(true) {
                        detail.push_str(" \u{00B7} DS");
                    }
                    if step.signed {
                        detail.push_str(" \u{00B7} RRSIG");
                    }
                    let text = format!(
                        "  {:<15}{:<zones$}  {detail}",
                        step.role(index + 1 == dns.path.len()),
                        truncate_middle(&step.zone, zones)
                    );
                    if index == self.dns_row {
                        at = lines.len();
                        lines.push(Line::styled(fit(&text, columns), selected()));
                    } else if step.error.is_some() {
                        lines.push(Line::styled(text, Paint::new().fg(Color::Yellow)));
                    } else {
                        lines.push(Line::raw(text));
                    }
                }
                if !running {
                    lines.push(Line::default());
                    lines.push(Line::styled(
                        format!(" {}", dns.dnssec.title()),
                        match dns.dnssec {
                            Dnssec::Signed => bold.fg(Color::Green),
                            Dnssec::Broken => bold.fg(Color::Red),
                            _ => bold,
                        },
                    ));
                    if !dns.dnssec_detail.is_empty() {
                        lines.extend(sentence(&dns.dnssec_detail, Paint::new()));
                    }
                    if !dns.path.is_empty() {
                        lines.push(Line::default());
                        lines.extend(sentence(
                            "rvrs checks that DS and RRSIG records are present. It doesn't validate signatures.",
                            dim,
                        ));
                    }
                }
            }
        }
        (lines, at)
    }

    /// The selected record's details, in the column the hop's details take in a trace.
    fn draw_record(&self, frame: &mut Frame, area: Rect) {
        let Some(record) = self.dns_record() else {
            return;
        };
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let room = usize::from(area.width).saturating_sub(4).max(16);
        let fact = |label: &str, value: String| {
            Line::from(vec![
                Span::styled(
                    format!("  {:<10}", format!("{label}:")),
                    Paint::new().fg(LABEL),
                ),
                Span::styled(value, bold),
            ])
        };
        let mut lines: Vec<Line> = vec![
            Line::default(),
            Line::styled(
                format!(
                    "  {} {}",
                    record.type_name(),
                    truncate_middle(&record.name, room.saturating_sub(8))
                ),
                bold,
            ),
            Line::default(),
            fact(
                "Name",
                truncate_middle(&record.name, room.saturating_sub(10)),
            ),
            fact(
                "TTL",
                format!("{} s \u{00B7} {}", record.ttl, format::ttl(record.ttl)),
            ),
            fact(
                "Section",
                match record.section {
                    dns::Section::Answer => "Answer",
                    dns::Section::Authority => "Authority",
                    dns::Section::Additional => "Additional",
                }
                .to_string(),
            ),
        ];
        // The whole value, however long, broken where the column ends.
        let characters: Vec<char> = record.value.chars().collect();
        for (index, chunk) in characters
            .chunks(room.saturating_sub(10).max(8))
            .enumerate()
        {
            let text: String = chunk.iter().collect();
            lines.push(if index == 0 {
                fact("Value", text)
            } else {
                Line::styled(format!("  {:<10}{text}", ""), bold)
            });
        }
        lines.push(Line::default());
        for line in pretty::wrap(explain_record(&record), room) {
            lines.push(Line::styled(format!("  {line}"), dim));
        }
        lines.push(Line::default());
        lines.push(Line::styled("  ACTIONS", bold));
        for (index, action) in record_actions(&record).into_iter().enumerate() {
            let key = format!("  {:<6}", format!("<{}>", action.key()));
            let key = Span::styled(key, Paint::new().fg(KEY).add_modifier(Modifier::BOLD));
            lines.push(if self.details_focus && index == self.action {
                Line::from(vec![
                    key,
                    Span::styled(
                        format!(" {} ", action.title()),
                        selected().add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(vec![key, Span::raw(format!(" {}", action.title()))])
            });
        }
        let block = panel_with(
            "Record",
            &record.type_name(),
            &format::ttl(record.ttl),
            self.details_focus,
        );
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    /// An MTR as a live table: one hop per row with its figures and a strip of its newest
    /// rounds, the destination's latency round by round under it, and what it likely means.
    /// `d` gives the figures' room to the strips.
    fn mtr_lines(&mut self, inner: Rect) -> Vec<Line<'static>> {
        let Some(shown) = &self.shown else {
            return Vec::new();
        };
        let run = shown.run();
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let focused = !self.details_focus;
        let columns = usize::from(inner.width);
        let path = run.path();
        self.hop = self.hop.min(path.len().saturating_sub(1));
        // The hops a search leaves, by their place in the path.
        let words = words(&self.search);
        let visible: Vec<usize> = path
            .iter()
            .enumerate()
            .filter(|(_, hop)| hop_found(run, hop, &words))
            .map(|(index, _)| index)
            .collect();
        let at = visible
            .iter()
            .position(|index| *index >= self.hop)
            .unwrap_or(0);

        // The figures, most telling first, as many as fit. The history view keeps two of them.
        let all: [(&str, usize); 7] = [
            ("LOSS", 7),
            ("SNT", 5),
            ("LAST", 7),
            ("AVG", 7),
            ("BEST", 7),
            ("WORST", 7),
            ("JITTER", 8),
        ];
        let wanted: Vec<usize> = if self.mtr_history {
            vec![0, 3]
        } else {
            vec![0, 1, 2, 3, 4, 5, 6]
        };
        let host_floor = 18;
        let mut shown_figures: Vec<usize> = Vec::new();
        let mut figures_width = 0;
        // Loss and the average go in first, then the rest in their own order.
        for index in [0, 3, 1, 6, 2, 4, 5] {
            if wanted.contains(&index) && 6 + host_floor + figures_width + all[index].1 <= columns {
                shown_figures.push(index);
                figures_width += all[index].1;
            }
        }
        shown_figures.sort_unstable();
        let left = columns.saturating_sub(6 + figures_width);
        // Networks come back once the host has its room, and the strip takes what is left.
        let show_network = !self.mtr_history && left >= 30 + 22 + 18;
        let network_width = if show_network {
            (left / 4).clamp(16, 30)
        } else {
            0
        };
        let strip_floor = if self.mtr_history { 24 } else { 12 };
        let after_network = left.saturating_sub(network_width);
        let strip = if after_network >= host_floor + strip_floor + 2 {
            if self.mtr_history {
                after_network.saturating_sub(34 + 2).max(strip_floor)
            } else {
                (after_network - host_floor - 2).min(40)
            }
        } else {
            0
        };
        let host_width = after_network
            .saturating_sub(if strip > 0 { strip + 2 } else { 0 })
            .max(10);

        let mut header = format!(" {}{}", fit("HOP\u{2191}", 5), fit("HOST", host_width));
        if show_network {
            header.push_str(&fit("NETWORK", network_width));
        }
        for index in &shown_figures {
            header.push_str(&fit_right(all[*index].0, all[*index].1));
        }
        if strip > 0 {
            header.push_str("  ROUNDS");
        }
        let mut lines = Vec::new();
        lines.push(Line::styled(header, bold));

        let mut network_cells: Vec<String> = vec![String::new(); path.len()];
        for group in groups(run) {
            for (position, hop) in group.hops.iter().enumerate() {
                let index = usize::from(hop.ttl)
                    .saturating_sub(1)
                    .min(path.len().saturating_sub(1));
                // A search can hide the hop that names the network, so each row names its own.
                network_cells[index] = match (group.network, position) {
                    (Some(network), 0) => format!("{} {}", network.label(), network.name),
                    (Some(network), _) if !words.is_empty() => {
                        format!("{} {}", network.label(), network.name)
                    }
                    (Some(_), _) => "\u{2506}".to_string(),
                    (None, _) => String::new(),
                };
            }
        }
        let slowest = path
            .iter()
            .flat_map(|hop| hop.probes.iter().filter_map(Slot::rtt))
            .max()
            .unwrap_or_default();

        // The rate-limit note belongs to the table while it runs, the rest once it has ended.
        let mut notes: Vec<String> = Vec::new();
        if run.is_running() {
            notes.extend(run.rate_limit_note());
        } else {
            notes.extend(rvrs_core::report::Report::of(run).interpretation);
        }
        let note_lines: Vec<String> = notes
            .iter()
            .flat_map(|note| pretty::wrap(note, columns.saturating_sub(2).clamp(20, 110)))
            .collect();

        let room = usize::from(inner.height).saturating_sub(1);
        // Latency, jitter and loss side by side on a wide panel, one under the other on a
        // narrow one when it is tall enough, else latency alone.
        let wide = columns >= 120;
        let chart_rows = if wide { 7 } else { 17 };
        let below = if note_lines.is_empty() {
            0
        } else {
            note_lines.len() + 1
        };
        let chart_hop = run.chart_hop();
        let show_chart = chart_hop.is_some() && room > visible.len() + below + chart_rows;
        let table_room = room
            .saturating_sub(below + if show_chart { chart_rows } else { 0 })
            .max(1);
        if at < self.scroll {
            self.scroll = at;
        } else if at >= self.scroll + table_room {
            self.scroll = at + 1 - table_room;
        }

        // The charts sit on top of the hops, and what the run likely means under the table.
        if let (true, Some(hop)) = (show_chart, chart_hop) {
            let caption = if hop.is_destination {
                format!(" AT THE DESTINATION \u{00B7} HOP {}", hop.ttl)
            } else {
                format!(" AT THE FARTHEST REPLY \u{00B7} HOP {}", hop.ttl)
            };
            let mut charts = vec![Line::styled(caption, bold)];
            charts.extend(mtr_charts(hop, " ", if wide { 4 } else { 3 }, columns));
            charts.push(Line::default());
            lines.splice(0..0, charts);
        }

        if visible.is_empty() && !words.is_empty() {
            lines.push(Line::styled(" No hop matches", dim));
        }
        for &index in visible.iter().skip(self.scroll).take(table_room) {
            let hop = &path[index];
            let stats = hop.stats();
            let address = hop.address();
            let host = match address {
                Some(address) => run
                    .name(address)
                    .map_or_else(|| address.to_string(), str::to_string),
                None if stats.sent > 0 || !run.is_running() => "no reply".to_string(),
                None => "\u{2026}".to_string(),
            };
            let others = hop.addresses().len().saturating_sub(1);
            let host = if others > 0 {
                format!("{host} +{others}")
            } else {
                host
            };
            let marker = if hop.is_destination { "\u{25CF}" } else { " " };
            let lead = format!(
                "{marker}{}{}",
                fit(&hop.ttl.to_string(), 5),
                fit(
                    &truncate_middle(&host, host_width.saturating_sub(1)),
                    host_width
                )
            );
            let network = if show_network {
                fit(
                    &truncate_end(&network_cells[index], network_width - 1),
                    network_width,
                )
            } else {
                String::new()
            };
            let ms = |value: Option<Duration>| value.map_or_else(String::new, format::ms_bare);
            let last = hop.probes.iter().rev().find_map(Slot::rtt);
            let cell = |figure: usize| -> String {
                let text = match figure {
                    0 if stats.sent == 0 => String::new(),
                    0 => format::percent(stats.loss_fraction()),
                    1 => stats.sent.to_string(),
                    2 => ms(last),
                    3 => ms(stats.mean),
                    4 => ms(stats.min),
                    5 => ms(stats.max),
                    _ => ms(stats.jitter),
                };
                fit_right(&text, all[figure].1)
            };

            if focused && index == self.hop {
                let mut text = format!("{lead}{network}");
                for figure in &shown_figures {
                    text.push_str(&cell(*figure));
                }
                if strip > 0 {
                    text.push_str("  ");
                    text.push_str(&crate::mtr::sparkline(
                        &hop.probes,
                        Style::PLAIN,
                        strip,
                        slowest,
                    ));
                }
                lines.push(Line::styled(fit(&text, columns), selected()));
                continue;
            }
            let base = if address.is_none() {
                dim
            } else if hop.is_destination {
                bold.fg(Color::Green)
            } else {
                Paint::new()
            };
            // Loss is red only when it reaches the end of the path. A middle hop that drops
            // replies while later hops are clean is rate-limiting them, and reads gray.
            let loss_paint = if stats.lost() > 0 && hop.did_reply() && run.loss_carries(hop) {
                bold.fg(Color::Red)
            } else {
                dim
            };
            let mut spans = vec![Span::styled(lead, base), Span::styled(network, base)];
            for figure in &shown_figures {
                let paint = match figure {
                    0 => loss_paint,
                    3 => stats.mean.map_or(dim, |mean| bold.fg(latency_color(mean))),
                    2 => Paint::new(),
                    _ => dim,
                };
                spans.push(Span::styled(cell(*figure), paint));
            }
            if strip > 0 {
                spans.push(Span::raw("  "));
                spans.extend(
                    ansi_line(&crate::mtr::sparkline(
                        &hop.probes,
                        Style { color: true },
                        strip,
                        slowest,
                    ))
                    .spans,
                );
            }
            lines.push(Line::from(spans));
        }
        if run.is_running() && (run.hops.len() > path.len() || path.is_empty()) {
            lines.push(Line::styled(
                format!(" {}\u{2026}", fit(&(path.len() + 1).to_string(), 5)),
                dim,
            ));
        }
        if !note_lines.is_empty() {
            lines.push(Line::default());
            for line in &note_lines {
                lines.push(Line::styled(format!(" {line}"), dim));
            }
        }
        lines
    }

    /// A traceroute as a table, one hop per row, and under it the latency of every hop as a
    /// column chart when the panel is tall enough.
    fn hop_lines(&mut self, inner: Rect) -> Vec<Line<'static>> {
        let Some(shown) = &self.shown else {
            return Vec::new();
        };
        let run = shown.run();
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let focused = !self.details_focus;
        let columns = usize::from(inner.width);
        let path = run.path();
        self.hop = self.hop.min(path.len().saturating_sub(1));
        // The hops a search leaves, by their place in the path.
        let words = words(&self.search);
        let visible: Vec<usize> = path
            .iter()
            .enumerate()
            .filter(|(_, hop)| hop_found(run, hop, &words))
            .map(|(index, _)| index)
            .collect();
        let at = visible
            .iter()
            .position(|index| *index >= self.hop)
            .unwrap_or(0);

        // Columns go as the terminal narrows: the bar first, then best and worst, then the network.
        let any_v6 = path
            .iter()
            .any(|hop| hop.address().is_some_and(|address| address.is_ipv6()));
        let address_width = if any_v6 { 28 } else { 16 };
        let base = 1 + 5 + address_width + 7 + 9;
        let show_range = columns >= base + 18 + 40;
        let show_bar = columns >= base + 18 + 60 + 16;
        let show_network = columns >= base + 44;
        let fixed = base + if show_range { 18 } else { 0 } + if show_bar { 16 } else { 0 };
        let flex = columns.saturating_sub(fixed);
        let network_width = if show_network {
            (flex * 2 / 5).clamp(14, 34)
        } else {
            0
        };
        let host_width = flex.saturating_sub(network_width).max(10);

        let mut header = format!(
            " {}{}{}",
            fit("HOP\u{2191}", 5),
            fit("HOST", host_width),
            fit("ADDRESS", address_width)
        );
        if show_network {
            header.push_str(&fit("NETWORK", network_width));
        }
        header.push_str(&format!("{}{}", fit_right("LOSS", 6), fit_right("AVG", 9)));
        if show_range {
            header.push_str(&format!(
                "{}{}",
                fit_right("BEST", 9),
                fit_right("WORST", 9)
            ));
        }
        if show_bar {
            header.push_str("  LATENCY");
        }
        let mut lines = vec![Line::styled(header, bold)];

        // The network is named on the first hop of its group and carried down by a rule, so the
        // table keeps the app's grouping.
        let mut network_cells: Vec<String> = vec![String::new(); path.len()];
        for group in groups(run) {
            for (position, hop) in group.hops.iter().enumerate() {
                let index = usize::from(hop.ttl)
                    .saturating_sub(1)
                    .min(path.len().saturating_sub(1));
                // A search can hide the hop that names the network, so each row names its own.
                network_cells[index] = match (group.network, position) {
                    (Some(network), 0) => format!("{} {}", network.label(), network.name),
                    (Some(network), _) if !words.is_empty() => {
                        format!("{} {}", network.label(), network.name)
                    }
                    (Some(_), _) => "\u{2506}".to_string(),
                    (None, _) => String::new(),
                };
            }
        }
        let slowest = path
            .iter()
            .filter_map(|hop| hop.stats().mean)
            .max()
            .unwrap_or_default();

        let room = usize::from(inner.height).saturating_sub(1);
        let chart_rows = 7;
        let table_room = if room >= visible.len() + 2 + chart_rows {
            room - chart_rows
        } else {
            room
        };
        if at < self.scroll {
            self.scroll = at;
        } else if at >= self.scroll + table_room.max(1) {
            self.scroll = at + 1 - table_room.max(1);
        }
        if visible.is_empty() && !words.is_empty() {
            lines.push(Line::styled(" No hop matches", dim));
        }
        for &index in visible.iter().skip(self.scroll).take(table_room.max(1)) {
            let hop = &path[index];
            let stats = hop.stats();
            let address = hop.address();
            let host = match address {
                Some(address) => run
                    .name(address)
                    .map_or_else(|| address.to_string(), str::to_string),
                None if hop.is_settled() || !run.is_running() => "no reply".to_string(),
                None => "\u{2026}".to_string(),
            };
            let others = hop.addresses().len().saturating_sub(1);
            let address_text = match (address, others) {
                (Some(address), 0) => address.to_string(),
                (Some(address), others) => format!("{address} +{others}"),
                (None, _) => String::new(),
            };
            let marker = if hop.is_destination { "\u{25CF}" } else { " " };
            let lead = format!(
                "{marker}{}{}{}",
                fit(&hop.ttl.to_string(), 5),
                fit(
                    &truncate_middle(&host, host_width.saturating_sub(1)),
                    host_width
                ),
                fit(
                    &truncate_middle(&address_text, address_width - 1),
                    address_width
                )
            );
            let network = if show_network {
                fit(
                    &truncate_end(&network_cells[index], network_width - 1),
                    network_width,
                )
            } else {
                String::new()
            };
            let loss = if stats.sent == 0 {
                String::new()
            } else {
                format!("{:.0}%", stats.loss_fraction() * 100.0)
            };
            let figure = |value: Option<Duration>| {
                fit_right(&value.map_or_else(String::new, format::ms_bare), 9)
            };
            let mut tail = String::new();
            if show_range {
                tail.push_str(&format!("{}{}", figure(stats.min), figure(stats.max)));
            }
            let bar = match (show_bar, stats.mean) {
                (true, Some(mean)) => format!("  {}", pretty::bar(mean, slowest, 13)),
                _ => String::new(),
            };

            if focused && index == self.hop {
                let text = format!(
                    "{lead}{network}{}{}{tail}{bar}",
                    fit_right(&loss, 6),
                    figure(stats.mean)
                );
                lines.push(Line::styled(fit(&text, columns), selected()));
                continue;
            }
            let base = if address.is_none() {
                dim
            } else if hop.is_destination {
                bold.fg(Color::Green)
            } else {
                Paint::new()
            };
            // Loss that reaches the end of the path is red. A middle hop that drops replies while
            // later hops are clean is likely rate-limiting them, and reads yellow.
            let loss_paint = if stats.received == 0 || stats.lost() == 0 {
                dim
            } else if run.loss_carries(hop) {
                Paint::new().fg(Color::Red)
            } else {
                Paint::new().fg(Color::Yellow)
            };
            let latency = stats
                .mean
                .map_or(dim, |mean| Paint::new().fg(latency_color(mean)));
            lines.push(Line::from(vec![
                Span::styled(lead, base),
                Span::styled(network, base),
                Span::styled(fit_right(&loss, 6), loss_paint),
                Span::styled(figure(stats.mean), latency.add_modifier(Modifier::BOLD)),
                Span::styled(tail, dim),
                Span::styled(bar, latency),
            ]));
        }

        // What lies behind the farthest hop that answered.
        let sent = run.hops.len();
        if run.is_running() && (sent > path.len() || path.is_empty()) {
            lines.push(Line::styled(
                format!(" {}\u{2026}", fit(&(path.len() + 1).to_string(), 5)),
                dim,
            ));
        } else if run.outcome == Some(Outcome::Exhausted) && sent > path.len() {
            let range = if sent == path.len() + 1 {
                sent.to_string()
            } else {
                format!("{}\u{2013}{sent}", path.len() + 1)
            };
            lines.push(Line::styled(format!(" {}no reply", fit(&range, 5)), dim));
        }
        if !run.is_running() {
            for sentence in rvrs_core::report::Report::of(run).interpretation {
                lines.push(Line::default());
                for line in pretty::wrap(&sentence, columns.saturating_sub(2).clamp(20, 100)) {
                    lines.push(Line::styled(format!(" {line}"), dim));
                }
            }
        }

        if table_room < room && !path.is_empty() {
            let values: Vec<Option<f64>> = path
                .iter()
                .map(|hop| hop.stats().mean.map(format::ms_value))
                .collect();
            let labels: Vec<String> = path.iter().map(|hop| hop.ttl.to_string()).collect();
            while lines.len() + chart_rows < usize::from(inner.height) {
                lines.push(Line::default());
            }
            lines.push(Line::styled(" LATENCY BY HOP", bold));
            lines.extend(column_chart(&values, 4, 3, 1, Some(&labels), columns));
        }
        lines
    }

    /// A ping: the figures in a row, the verdict, every packet as a column, then the packets
    /// newest first.
    fn ping_lines(&self, run: &Run, inner: Rect) -> Vec<Line<'static>> {
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let columns = usize::from(inner.width);
        let stats = run.ping_stats();
        let ms =
            |value: Option<Duration>| value.map_or_else(|| "\u{2013}".to_string(), format::ms_bare);
        let tiles: Vec<(&str, String, Paint)> = vec![
            ("SENT", stats.sent.to_string(), bold),
            (
                "LOSS",
                format::percent(stats.loss_fraction()),
                if stats.lost() > 0 {
                    bold.fg(Color::Red)
                } else {
                    bold.fg(Color::Green)
                },
            ),
            (
                "AVG",
                ms(stats.mean),
                stats.mean.map_or(bold, |mean| bold.fg(latency_color(mean))),
            ),
            ("MIN", ms(stats.min), bold),
            ("MAX", ms(stats.max), bold),
            ("JITTER", ms(stats.jitter), bold),
            ("P50", ms(stats.p50), bold),
            ("P95", ms(stats.p95), bold),
        ];
        let mut spans = vec![Span::raw(" ")];
        let mut used = 1;
        for (label, value, paint) in tiles {
            let cell = width(label) + 1 + width(&value) + 3;
            if used + cell > columns {
                break;
            }
            used += cell;
            spans.push(Span::styled(format!("{label} "), Paint::new().fg(LABEL)));
            spans.push(Span::styled(format!("{value}   "), paint));
        }
        let mut lines = vec![Line::default(), Line::from(spans)];

        let verdict = Verdict::of(&stats);
        let verdict_line = if stats.sent > 0 && stats.received == 0 && !run.is_running() {
            Line::styled(format!(" {}", rvrs_core::report::SILENT_TARGET), dim)
        } else if verdict.grade == Grade::Collecting {
            Line::styled(format!(" {}. {}", verdict.title, verdict.detail), dim)
        } else {
            let color = match verdict.grade {
                Grade::Stable => Color::Green,
                Grade::Lossy => Color::Red,
                _ => Color::Yellow,
            };
            Line::from(vec![
                Span::styled(format!(" {}. ", verdict.title), bold.fg(color)),
                Span::raw(truncate_end(
                    &verdict.detail,
                    columns.saturating_sub(width(&verdict.title) + 4),
                )),
            ])
        };
        match run.reading() {
            // A run that came as a report says it in its maker's words.
            Some(reading) => {
                for sentence in reading {
                    for line in pretty::wrap(sentence, columns.saturating_sub(2).clamp(20, 110)) {
                        lines.push(Line::raw(format!(" {line}")));
                    }
                }
            }
            None => lines.push(verdict_line),
        }
        lines.push(Line::default());

        let packets = run.packets();
        let chart_width = columns.saturating_sub(12);
        let window = &packets[packets.len().saturating_sub(chart_width.max(1))..];
        let values: Vec<Option<f64>> = window
            .iter()
            .filter(|slot| **slot != Slot::Pending)
            .map(|slot| slot.rtt().map(format::ms_value))
            .collect();
        if !values.is_empty() && inner.height >= 16 {
            lines.push(Line::styled(" LATENCY BY PACKET", bold));
            lines.extend(column_chart(&values, 5, 1, 0, None, columns));
            lines.push(Line::default());
        }

        lines.push(Line::styled(
            format!(
                " {}{}{}",
                fit("SEQ\u{2193}", 7),
                fit_right("TIME", 10),
                fit_right("TTL", 6)
            ),
            bold,
        ));
        let room = usize::from(inner.height).saturating_sub(lines.len());
        for seq in (0..packets.len())
            .rev()
            .filter(|seq| packets[*seq] != Slot::Pending)
            .take(room)
        {
            let number = fit(&(seq + 1).to_string(), 7);
            lines.push(match &packets[seq] {
                Slot::Reply { micros, ttl, .. } => {
                    let time = Duration::from_micros(*micros);
                    Line::from(vec![
                        Span::styled(format!(" {number}"), dim),
                        Span::styled(
                            fit_right(&format::ms(time), 10),
                            Paint::new().fg(latency_color(time)),
                        ),
                        Span::styled(
                            fit_right(&ttl.map_or_else(String::new, |ttl| ttl.to_string()), 6),
                            dim,
                        ),
                    ])
                }
                Slot::Refused {
                    from,
                    time_exceeded: true,
                    ..
                } => Line::styled(
                    format!(" {number}TTL ran out at {from}"),
                    Paint::new().fg(Color::Yellow),
                ),
                Slot::Refused { from, .. } => Line::styled(
                    format!(" {number}{from} reports the target unreachable"),
                    Paint::new().fg(Color::Yellow),
                ),
                _ => Line::styled(
                    format!(" {number}{}", fit_right("no reply", 10)),
                    Paint::new().fg(Color::Red),
                ),
            });
        }
        lines
    }

    /// Where you are, as chips, and what just happened beside them.
    fn draw_crumbs(&self, frame: &mut Frame, area: Rect, with_keys: bool) {
        let dim = Paint::new().add_modifier(Modifier::DIM);
        // While a search is typed, the foot of the screen is its line, as in less and vim.
        if self.searching {
            let room = usize::from(area.width).saturating_sub(4);
            let typed = tail(&self.search, room);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(" /", Paint::new().fg(LABEL).add_modifier(Modifier::BOLD)),
                    Span::raw(typed.clone()),
                ])),
                area,
            );
            if self.help.is_none() {
                frame.set_cursor_position(Position::new(area.x + 2 + width(&typed) as u16, area.y));
            }
            return;
        }
        let chip = |color: Color| {
            Paint::new()
                .bg(color)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        };
        let on_start = self.pane == Pane::Start || self.shown.is_none();
        let mut spans = vec![Span::raw(" ")];
        // The sections, as tabs. The one you are in is lit.
        for section in Section::ALL {
            let label = format!(" \u{21E7}{} {} ", section.number(), section.title());
            spans.push(if section == self.section {
                Span::styled(label, chip(ACCENT))
            } else if with_keys {
                // A short terminal needs the room for the keys: the other tabs keep their number.
                Span::styled(format!(" \u{21E7}{} ", section.number()), dim)
            } else {
                Span::styled(label, dim)
            });
            spans.push(Span::raw(" "));
        }
        // Where you are inside it.
        if on_start && self.section == Section::Cases && self.case.is_some() {
            spans.push(Span::styled("\u{203A} ", dim));
            spans.push(Span::styled(format!(" {} ", self.list_name()), chip(LABEL)));
            spans.push(Span::raw(" "));
        }
        if !on_start {
            if let Some(run) = self.run() {
                spans.push(Span::styled("\u{203A} ", dim));
                spans.push(Span::styled(
                    format!(" {} ", run.tool.command()),
                    chip(LABEL),
                ));
                spans.push(Span::raw(" "));
            }
            if self.details {
                if let Some(hop) = self.run().and_then(|run| run.path().get(self.hop)) {
                    spans.push(Span::styled("\u{203A} ", dim));
                    spans.push(Span::styled(format!(" hop {} ", hop.ttl), chip(LABEL)));
                    spans.push(Span::raw(" "));
                }
            }
        }
        if let Some((text, _)) = &self.toast {
            spans.push(Span::styled(
                format!(" {text}"),
                Paint::new().fg(Color::Green),
            ));
        } else if with_keys {
            // No header on a short terminal, so the keys ride along here.
            for (key, label) in self.keys() {
                spans.push(Span::styled(format!(" <{key}>"), Paint::new().fg(KEY)));
                spans.push(Span::styled(format!(" {label} "), dim));
            }
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// The selected hop in a column of its own: what it is, how it answered, its network, then
    /// what can be done with it. It follows the selection in the table.
    fn draw_details(&self, frame: &mut Frame, area: Rect) {
        let Some(run) = self.run() else { return };
        if run.tool == Tool::Dns {
            self.draw_record(frame, area);
            return;
        }
        if run.tool == Tool::Bgp {
            self.draw_bgp_details(frame, area);
            return;
        }
        let Some(hop) = run.path().get(self.hop) else {
            return;
        };
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let address = hop.address();
        let name = address.and_then(|address| run.name(address));
        let title = name
            .map(str::to_string)
            .or_else(|| address.map(|address| address.to_string()))
            .unwrap_or_else(|| format!("Hop {}", hop.ttl));

        let fact = |label: &str, value: Vec<Span<'static>>| {
            let mut spans = vec![Span::styled(
                format!("  {:<14}", format!("{label}:")),
                Paint::new().fg(LABEL),
            )];
            spans.extend(value);
            Line::from(spans)
        };
        let plain = |text: String| vec![Span::styled(text, bold)];
        // The host, with a heart behind it when it is a favorite. f gives it one or takes it away.
        let favorite = address.is_some() && self.favorites.contains(&title);
        let mut lines: Vec<Line> = vec![
            Line::default(),
            Line::from(vec![
                Span::styled(
                    format!(
                        "  {}",
                        truncate_middle(&title, usize::from(area.width).saturating_sub(16))
                    ),
                    bold,
                ),
                if favorite {
                    Span::styled(" \u{2665}", bold.fg(TOOL))
                } else {
                    Span::styled(" \u{2661}", dim)
                },
                Span::styled("  <f>", Paint::new().fg(KEY).add_modifier(Modifier::BOLD)),
                Span::styled(
                    if favorite {
                        " Remove Favorite"
                    } else {
                        " Favorite"
                    },
                    dim,
                ),
            ]),
            Line::default(),
        ];
        if let (Some(_), Some(address)) = (name, address) {
            lines.push(fact("Address", plain(address.to_string())));
        }
        lines.push(fact("Hop", plain(hop.ttl.to_string())));
        let others = hop.addresses();
        if others.len() > 1 {
            let list: Vec<String> = others.iter().skip(1).map(IpAddr::to_string).collect();
            lines.push(fact("Also answered", plain(list.join(", "))));
        }
        lines.push(Line::default());

        let stats = hop.stats();
        let figure = |value: Option<Duration>| match value {
            Some(value) => vec![Span::styled(
                format::ms(value),
                bold.fg(latency_color(value)),
            )],
            None => vec![Span::styled("\u{2013}", dim)],
        };
        let loss_color = if stats.lost() == 0 {
            Color::Green
        } else if hop.did_reply() && run.loss_carries(hop) {
            Color::Red
        } else {
            Color::Yellow
        };
        lines.push(fact(
            "Loss",
            vec![
                Span::styled(format::percent(stats.loss_fraction()), bold.fg(loss_color)),
                Span::styled(
                    format!(
                        " \u{00B7} {} of {} probes answered",
                        stats.received, stats.sent
                    ),
                    dim,
                ),
            ],
        ));
        lines.push(fact("Avg", figure(stats.mean)));
        lines.push(fact("Min", figure(stats.min)));
        lines.push(fact("Max", figure(stats.max)));
        lines.push(fact(
            "Jitter",
            plain(
                stats
                    .jitter
                    .map_or_else(|| "\u{2013}".to_string(), format::ms),
            ),
        ));
        lines.push(Line::default());
        // An MTR has rounds enough to draw this hop's own latency, and it follows the selection.
        if run.tool == Tool::Mtr && !latency_series(hop).is_empty() {
            lines.extend(mtr_charts(
                hop,
                "  ",
                2,
                usize::from(area.width).saturating_sub(4).min(119),
            ));
            lines.push(Line::default());
        }

        match address.map(|address| (address, run.network(address))) {
            Some((_, Some(network))) => {
                lines.push(Line::styled("  NETWORK", bold));
                lines.push(fact("ASN", plain(network.label())));
                lines.push(fact("Organization", plain(network.name.clone())));
                if let Some(location) = network.location() {
                    lines.push(fact("Location", plain(location)));
                }
                if let Some(prefix) = &network.prefix {
                    lines.push(fact("Prefix", plain(prefix.clone())));
                }
            }
            Some((address, None)) if is_private(address) => {
                lines.push(Line::styled(
                    "  Private address. It belongs to no public network.",
                    dim,
                ));
            }
            _ => {}
        }
        lines.push(Line::default());

        // What can be done with this host. Its key works from the hops too; with the keys in
        // this column the arrows pick one and Return does it.
        lines.push(Line::styled("  ACTIONS", bold));
        for (index, action) in actions_for(run, hop).into_iter().enumerate() {
            let label = match action {
                Action::Favorite if self.favorites.contains(&title) => {
                    "Remove Favorite".to_string()
                }
                Action::Whois => format!("{} \u{2197}", action.title()),
                _ => action.title().to_string(),
            };
            let key = format!("  {:<6}", format!("<{}>", action.key()));
            lines.push(if self.details_focus && index == self.action {
                Line::from(vec![
                    Span::styled(key, Paint::new().fg(KEY).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        format!(" {label} "),
                        selected().add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(key, Paint::new().fg(KEY).add_modifier(Modifier::BOLD)),
                    Span::raw(format!(" {label}")),
                ])
            });
        }

        let block = panel_with(
            "Hop",
            &truncate_middle(&title, usize::from(area.width.saturating_sub(16))),
            &hop.ttl.to_string(),
            self.details_focus,
        );
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }
}

/// A source's answers in the order the Records tab draws them: by group, as the app groups them.
/// A search keeps the records it finds.
fn grouped_records(source: &SourceResult, search: &[String]) -> Vec<(RecordGroup, Record)> {
    RecordGroup::ALL
        .into_iter()
        .flat_map(|group| {
            source
                .answers()
                .filter(move |record| RecordGroup::of(record) == group)
                .filter(move |record| record_found(record, search))
                .map(move |record| (group, record.clone()))
        })
        .collect()
}

/// The records a DNS tab lists, top to bottom. Compare and Trace list none.
fn dns_records(dns: &DnsResult, tab: DnsTab, source: usize, search: &[String]) -> Vec<Record> {
    match tab {
        DnsTab::Overview => dns
            .key_records()
            .into_iter()
            .filter(|record| record_found(record, search))
            .cloned()
            .collect(),
        DnsTab::Records => dns
            .ordered_sources()
            .get(source)
            .map(|source| {
                grouped_records(source, search)
                    .into_iter()
                    .map(|(_, record)| record)
                    .collect()
            })
            .unwrap_or_default(),
        DnsTab::Compare | DnsTab::Trace => Vec::new(),
    }
}

/// What can be done with a record: the tools against the host it names, if it names one, and
/// the two ways to copy it.
fn record_actions(record: &Record) -> Vec<Action> {
    let mut actions = Vec::new();
    if tool_host(record).is_some() {
        actions.extend([Action::Trace, Action::Mtr, Action::Ping, Action::Bgp]);
    }
    actions.extend([Action::CopyValue, Action::CopyZoneLine]);
    actions
}

/// A framed panel with its title in the middle of the top edge: Name(scope)[count].
fn panel(name: &str, scope: &str, count: &str) -> Block<'static> {
    panel_with(name, scope, count, true)
}

/// The same panel, its frame bright when it has the keys and dim when the other column does.
fn panel_with(name: &str, scope: &str, count: &str, focused: bool) -> Block<'static> {
    panel_searched(name, scope, count, focused, "")
}

/// The panel of a list a search narrowed: the search follows the count, as k9s writes it,
/// `Recent(all)[3] </git>`.
fn panel_searched(
    name: &str,
    scope: &str,
    count: &str,
    focused: bool,
    search: &str,
) -> Block<'static> {
    let bold = Paint::new().add_modifier(Modifier::BOLD);
    let mut title = vec![
        Span::styled(format!(" {name}"), bold.fg(ACCENT)),
        Span::styled(format!("({scope})"), bold.fg(TOOL)),
        // A panel that counts nothing ends its title after the scope.
        Span::styled(
            if count.is_empty() {
                " ".to_string()
            } else {
                format!("[{count}] ")
            },
            bold,
        ),
    ];
    let search = search.trim();
    if !search.is_empty() {
        title.push(Span::styled(
            format!("</{}> ", truncate_end(search, 24)),
            bold.fg(LABEL),
        ));
    }
    Block::bordered()
        .border_style(if focused {
            Paint::new().fg(ACCENT)
        } else {
            Paint::new().add_modifier(Modifier::DIM)
        })
        .title_alignment(Alignment::Center)
        .title(Line::from(title))
}

/// The highlighted row: black on cyan across the whole width.
fn selected() -> Paint {
    Paint::new().bg(ACCENT).fg(Color::Black)
}

/// "<key> Label", the key in blue.
fn key_line(key: &str, label: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<8}", format!("<{key}>")),
            Paint::new().fg(KEY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label.to_string(), Paint::new().add_modifier(Modifier::DIM)),
    ])
}

/// Green under 50 ms, yellow up to 150 ms, red beyond. The number is always beside the color.
fn latency_color(latency: Duration) -> Color {
    match crate::style::latency_tone(latency) {
        crate::style::Tone::Fast => Color::Green,
        crate::style::Tone::Slow => Color::Yellow,
        crate::style::Tone::VerySlow => Color::Red,
    }
}

/// Cut or padded on the right to exactly `columns`.
fn fit(text: &str, columns: usize) -> String {
    let text = truncate_end(text, columns);
    format!("{text}{}", " ".repeat(columns.saturating_sub(width(&text))))
}

/// Cut, or padded on the left, to exactly `columns`.
fn fit_right(text: &str, columns: usize) -> String {
    let text = truncate_end(text, columns);
    format!("{}{text}", " ".repeat(columns.saturating_sub(width(&text))))
}

/// Values as columns, tallest at the scale's top, in eighths of a row so small differences
/// show. A missing value (a loss, a silent hop) is a red mark on the baseline. The scale is
/// written beside the top and bottom rows.
/// What a column chart draws, which sets its axis and its colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Metric {
    /// Milliseconds in the latency colors. A missing value is a loss, a red mark.
    Latency,
    /// Milliseconds between one reply and the next, in magenta, the terminal's violet. A missing
    /// value is a gap.
    Jitter,
    /// Percent of probes lost, in red.
    Loss,
}

/// Values as columns, tallest at the scale's top, in eighths of a row so small differences
/// show. A missing latency (a loss, a silent hop) is a red mark on the baseline. The scale is
/// written beside the top and bottom rows.
fn column_chart(
    values: &[Option<f64>],
    rows: usize,
    column: usize,
    gap: usize,
    labels: Option<&[String]>,
    columns: usize,
) -> Vec<Line<'static>> {
    metric_chart(Metric::Latency, values, rows, column, gap, labels, columns)
}

/// A chart of one metric. See [`column_chart`].
fn metric_chart(
    metric: Metric,
    values: &[Option<f64>],
    rows: usize,
    column: usize,
    gap: usize,
    labels: Option<&[String]>,
    columns: usize,
) -> Vec<Line<'static>> {
    const LEVELS: [char; 9] = [
        ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}',
        '\u{2588}',
    ];
    let dim = Paint::new().add_modifier(Modifier::DIM);
    let gutter = 10;
    let fits = columns.saturating_sub(gutter + 1) / (column + gap).max(1);
    let values = &values[values.len().saturating_sub(fits)..];
    let mut top = values.iter().flatten().copied().fold(0.0, f64::max);
    // A loss chart reads against at least 10%, so a single lost probe isn't a full column.
    if metric == Metric::Loss {
        top = top.max(10.0);
    }
    let mut lines = Vec::new();
    for row in (0..rows).rev() {
        let axis = if row == rows - 1 {
            match metric {
                Metric::Loss => format!("{:>6.0}% \u{2524}", top),
                _ => format!("{:>7.1} \u{2524}", top),
            }
        } else if row == 0 {
            format!(
                "{:>7} \u{2524}",
                if metric == Metric::Loss { "0%" } else { "0 ms" }
            )
        } else {
            format!("{:>7} \u{2502}", "")
        };
        let mut spans = vec![Span::styled(format!(" {axis}"), dim)];
        for value in values {
            let cell = match value {
                // Nothing to draw: no loss, no jitter. The baseline says the value is zero.
                Some(value) if *value <= 0.0 && metric != Metric::Latency => Span::styled(
                    format!(
                        "{}{}",
                        if row == 0 { "\u{2581}" } else { " " }.repeat(column),
                        " ".repeat(gap)
                    ),
                    dim,
                ),
                Some(value) => {
                    let eighths = if top > 0.0 {
                        ((value / top) * (rows * 8) as f64).round() as usize
                    } else {
                        0
                    }
                    .max(1);
                    let level = eighths.saturating_sub(row * 8).min(8);
                    Span::styled(
                        format!(
                            "{}{}",
                            LEVELS[level].to_string().repeat(column),
                            " ".repeat(gap)
                        ),
                        match metric {
                            Metric::Latency => Paint::new()
                                .fg(latency_color(Duration::from_secs_f64(value / 1000.0))),
                            Metric::Jitter => Paint::new().fg(Color::Magenta),
                            Metric::Loss => Paint::new().fg(Color::Red),
                        },
                    )
                }
                None if row == 0 && metric == Metric::Latency => Span::styled(
                    format!("{}{}", "\u{00D7}".repeat(column), " ".repeat(gap)),
                    Paint::new().fg(Color::Red),
                ),
                None => Span::raw(" ".repeat(column + gap)),
            };
            spans.push(cell);
        }
        lines.push(Line::from(spans));
    }
    if let Some(labels) = labels {
        let labels = &labels[labels.len().saturating_sub(values.len())..];
        let mut text = format!(" {:>7}  ", "hop");
        for label in labels {
            text.push_str(&fit(label, column + gap));
        }
        lines.push(Line::styled(text, dim));
    }
    lines
}

/// Each round's latency, in milliseconds. A lost probe is `None`.
fn latency_series(hop: &Hop) -> Vec<Option<f64>> {
    hop.probes
        .iter()
        .filter(|slot| **slot != Slot::Pending)
        .map(|slot| slot.rtt().map(format::ms_value))
        .collect()
}

/// How far each reply's latency moved from the reply of the round before, in milliseconds.
/// The first round and a round after a loss have nothing to compare with.
fn jitter_series(hop: &Hop) -> Vec<Option<f64>> {
    let latency = latency_series(hop);
    latency
        .iter()
        .enumerate()
        .map(|(index, now)| {
            let before = index.checked_sub(1).and_then(|before| latency[before])?;
            Some(((*now)? - before).abs())
        })
        .collect()
}

/// The rounds that make up the loss window.
const LOSS_WINDOW: usize = 10;

/// Loss in percent over the last [`LOSS_WINDOW`] rounds up to each round, so one lost probe
/// shows as a step and not as a spike to 100%.
fn loss_series(hop: &Hop) -> Vec<Option<f64>> {
    let latency = latency_series(hop);
    (0..latency.len())
        .map(|index| {
            let window = &latency[(index + 1).saturating_sub(LOSS_WINDOW)..=index];
            let lost = window.iter().filter(|value| value.is_none()).count();
            Some(lost as f64 * 100.0 / window.len() as f64)
        })
        .collect()
}

/// Charts drawn side by side, each in `width` columns.
fn beside(charts: Vec<Vec<Line<'static>>>, each: usize) -> Vec<Line<'static>> {
    let rows = charts.iter().map(Vec::len).max().unwrap_or(0);
    (0..rows)
        .map(|row| {
            let mut spans = Vec::new();
            for chart in &charts {
                let line = chart.get(row).cloned().unwrap_or_default();
                let used: usize = line.spans.iter().map(|span| width(&span.content)).sum();
                spans.extend(line.spans);
                spans.push(Span::raw(" ".repeat(each.saturating_sub(used))));
            }
            Line::from(spans)
        })
        .collect()
}

/// The latency, jitter and loss of one MTR hop, as three captioned charts. Side by side when
/// `columns` has room for three, one under the other otherwise.
fn mtr_charts(hop: &Hop, lead: &str, rows: usize, columns: usize) -> Vec<Line<'static>> {
    let bold = Paint::new().add_modifier(Modifier::BOLD);
    let parts = [
        (
            format!("{lead}LATENCY"),
            Metric::Latency,
            latency_series(hop),
        ),
        (format!("{lead}JITTER"), Metric::Jitter, jitter_series(hop)),
        (
            format!("{lead}LOSS \u{00B7} {LOSS_WINDOW}-ROUND WINDOW"),
            Metric::Loss,
            loss_series(hop),
        ),
    ];
    let side_by_side = columns >= 120;
    let each = if side_by_side { columns / 3 } else { columns };
    let charts: Vec<Vec<Line<'static>>> = parts
        .into_iter()
        .map(|(caption, metric, values)| {
            let mut chart = vec![Line::styled(caption, bold)];
            chart.extend(metric_chart(
                metric,
                &values,
                rows,
                1,
                0,
                None,
                each.saturating_sub(2),
            ));
            chart
        })
        .collect();
    if side_by_side {
        return beside(charts, each);
    }
    let mut lines = Vec::new();
    for (index, chart) in charts.into_iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        lines.extend(chart);
    }
    lines
}

/// What can be done with a hop's host, in the order the app's host sheet offers it.
fn actions_for(run: &Run, hop: &Hop) -> Vec<Action> {
    let Some(address) = hop.address() else {
        return Vec::new();
    };
    let mut actions = vec![
        Action::Trace,
        Action::Mtr,
        Action::Ping,
        Action::CopyAddress,
    ];
    if run.name(address).is_some() {
        actions.push(Action::CopyHostname);
    }
    if !is_private(address) {
        actions.push(Action::Bgp);
        actions.push(Action::Whois);
    }
    actions
}

/// The end of the text that fits, so the cursor stays visible in a short field.
fn tail(text: &str, room: usize) -> String {
    let characters: Vec<char> = text.chars().collect();
    let skip = characters.len().saturating_sub(room.max(1));
    characters[skip..].iter().collect()
}

/// Turns a line painted by [`Style`] into spans, so the Console and the command share one renderer.
fn ansi_line(text: &str) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut paint = Paint::new();
    let mut buffer = String::new();
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\x1b' {
            buffer.push(character);
            continue;
        }
        if !buffer.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut buffer), paint));
        }
        let mut codes = String::new();
        for next in characters.by_ref() {
            if next.is_ascii_alphabetic() {
                break;
            }
            if next != '[' {
                codes.push(next);
            }
        }
        for code in codes.split(';') {
            paint = match code {
                "0" | "" => Paint::new(),
                "1" => paint.add_modifier(Modifier::BOLD),
                "2" => paint.add_modifier(Modifier::DIM),
                "31" => paint.fg(Color::Red),
                "32" => paint.fg(Color::Green),
                "33" => paint.fg(Color::Yellow),
                "34" => paint.fg(Color::Blue),
                _ => paint,
            };
        }
    }
    if !buffer.is_empty() {
        spans.push(Span::styled(buffer, paint));
    }
    Line::from(spans)
}

/// Puts text on the clipboard: the system's own tool where there is one, the terminal's
/// clipboard escape (OSC 52) otherwise.
fn copy_to_clipboard(text: &str) {
    let tools: [(&str, &[&str]); 3] = [
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
    ];
    for (tool, arguments) in tools {
        let child = Command::new(tool)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            if child.wait().is_ok_and(|status| status.success()) {
                return;
            }
        }
    }
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let _ = stdout.flush();
}

fn open_in_browser(url: &str) -> bool {
    let tool = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(tool)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let block = u32::from(chunk[0]) << 16
            | u32::from(*chunk.get(1).unwrap_or(&0)) << 8
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(ALPHABET[(block >> 18) as usize & 63] as char);
        out.push(ALPHABET[(block >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(block >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[block as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pretty::tests::sample;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn screen(app: &mut App, columns: u16, rows: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(columns, rows)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..rows)
            .map(|y| {
                (0..columns)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// The color of each cell where `text` first shows on screen, top to bottom.
    fn colors_of(app: &mut App, columns: u16, rows: u16, text: &str) -> Vec<Color> {
        let mut terminal = Terminal::new(TestBackend::new(columns, rows)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let characters: Vec<String> = text.chars().map(String::from).collect();
        let length = characters.len() as u16;
        for y in 0..rows {
            for x in 0..=columns.saturating_sub(length) {
                let found = characters
                    .iter()
                    .zip(x..)
                    .all(|(character, x)| buffer[(x, y)].symbol() == character);
                if found {
                    return (x..x + length).map(|x| buffer[(x, y)].fg).collect();
                }
            }
        }
        panic!(
            "{text} is not on screen:\n{}",
            screen(app, columns, rows).join("\n")
        );
    }

    fn app_with(runs: Vec<Run>) -> App {
        let mut history = History::at(std::env::temp_dir().join(format!(
            "rvrs-console-test-{}-{}.json",
            std::process::id(),
            runs.len()
        )));
        history.runs = runs;
        let scratch = |name: &str| {
            let path = std::env::temp_dir().join(format!(
                "rvrs-console-test-{name}-{}-{:?}.json",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_file(&path);
            path
        };
        let saved = History::at(scratch("saved"));
        let cases = Cases::at(scratch("cases"), &saved);
        App::new(history, saved, cases, Favorites::at(scratch("favorites")))
    }

    fn clean(app: &App) {
        let _ = std::fs::remove_file(app.saved.path());
        let _ = std::fs::remove_file(app.cases.path());
    }

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            press(app, KeyCode::Char(character));
        }
    }

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn the_account_section_shows_who_is_signed_in_and_takes_its_own_keys() {
        use account_view::State;
        let mut app = app_with(Vec::new());
        // Shift and 4, on US and Danish layouts alike.
        press(&mut app, KeyCode::Char('$'));
        assert_eq!(app.section, Section::Account);
        press(&mut app, KeyCode::Char('!'));
        press(&mut app, KeyCode::Char('\u{00A4}'));
        assert_eq!(app.section, Section::Account);

        app.account.state = State::SignedOut;
        let text = screen(&mut app, 150, 30).join("\n");
        assert!(text.contains("Account(this install)"), "{text}");
        assert!(text.contains("Account:  not signed in"));
        assert!(text.contains("<s>     Sign In"), "{text}");
        assert!(text.contains("\u{21E7}4 Account"));
        // No field in this section: letters go nowhere.
        type_text(&mut app, "xyz");
        assert!(app.input.is_empty());

        app.account.state = State::SignedIn {
            email: "ada@example.com".into(),
            workspace: "Ada\u{2019}s workspace".into(),
            install: "laptop.local".into(),
        };
        let text = screen(&mut app, 150, 30).join("\n");
        assert!(text.contains("Account:  ada@example.com \u{00B7} Ada\u{2019}s workspace"));
        assert!(text.contains("Signed in  ada@example.com"));
        assert!(text.contains("<q>     Sign Out"), "{text}");

        app.account.state = State::Waiting {
            code: "BKDF-7QXM".into(),
            link: "https://app.rvrs.dev/activate?code=BKDF-7QXM".into(),
        };
        let text = screen(&mut app, 150, 30).join("\n");
        assert!(text.contains("Code       BKDF-7QXM"));
        assert!(text.contains("<esc>   Cancel"));

        // Out of a finished state, esc goes back to the Console.
        app.account.state = State::SignedOut;
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.section, Section::Console);
        // A tool's key goes back to the Console's field from here too.
        press(&mut app, KeyCode::Char('$'));
        press(&mut app, KeyCode::Char('p'));
        assert_eq!(
            (app.section, app.field, app.tool),
            (Section::Console, true, Tool::Ping)
        );
        clean(&app);
    }

    #[test]
    fn an_empty_console_says_how_to_start() {
        let mut app = app_with(Vec::new());
        let text = screen(&mut app, 150, 30).join("\n");
        assert!(text.contains("Origin:   this device"));
        // The field waits for a tool's key, and shows them.
        assert!(!app.field);
        assert!(
            text.contains(
                "traceroute> <t> traceroute  <m> mtr  <p> ping  <d> dig  <b> bgp  <:> traceroute"
            ),
            "{text}"
        );
        assert!(text.contains("Recent(all)[0]"));
        assert!(text.contains("No runs yet"));
        assert!(text.contains("<\u{21B5}>     Target"), "{text}");
        assert!(text.contains("===/__/__/"), "{text}");
        assert!(text.contains("\u{21E7}1 Console") && text.contains("\u{21E7}3 Cases"));
    }

    #[test]
    fn recent_is_a_table_and_return_opens_a_saved_run() {
        let mut app = app_with(vec![sample()]);
        let text = screen(&mut app, 130, 30).join("\n");
        assert!(text.contains("Recent(all)[1]"));
        assert!(text.contains("TOOL") && text.contains("SUMMARY") && text.contains("AGE"));
        assert!(text.contains("4 hops \u{00B7} 12.4 ms"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.pane, Pane::Result);
        let text = screen(&mut app, 130, 34).join("\n");
        assert!(text.contains("Traceroute(one.one.one.one)[4]"));
        assert!(
            text.contains("HOST")
                && text.contains("NETWORK")
                && text.contains("LOSS")
                && text.contains("WORST")
        );
        assert!(text.contains("AS13335 Cloudflare, Inc."));
        assert!(text.contains("Status:   Reached 1.1.1.1"));
        assert!(text.contains("Address:  1.1.1.1 \u{00B7} IPv4"));
        assert!(text.contains("LATENCY BY HOP"));
        assert!(text.contains("<R>     Run Again") && text.contains("<r>     Run Tool"));
        assert!(text.contains("\u{21E7}1 Console") && text.contains("\u{203A}  traceroute"));
        // The tools are for choosing one on the overview. An open run doesn't list them.
        assert!(!text.contains("iperf3"));
    }

    #[test]
    fn a_small_terminal_drops_the_header_and_keeps_the_keys() {
        let mut app = app_with(vec![sample()]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 100, 18).join("\n");
        assert!(text.contains("Traceroute(one.one.one.one)[4]"));
        assert!(!text.contains("Origin:"));
        assert!(
            text.contains("<d> Details") && text.contains("</> Search"),
            "{text}"
        );
        for line in screen(&mut app, 60, 18) {
            assert!(width(&line) <= 60);
        }
        press(&mut app, KeyCode::Esc);
        assert!(screen(&mut app, 80, 18)
            .join("\n")
            .contains("Recent(all)[1]"));
    }

    #[test]
    fn hop_details_open_beside_the_table_and_follow_the_selection() {
        let mut app = app_with(vec![sample()]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        // Hop 1 is private and named: no Whois.
        press(&mut app, KeyCode::Enter);
        assert!(app.details);
        let lines = screen(&mut app, 150, 40);
        let text = lines.join("\n");
        assert!(
            text.contains("Traceroute(one.one.one.one)[4]") && text.contains("Hop(fritz.box)[1]")
        );
        assert!(
            text.contains("<t>    Run Traceroute")
                && text.contains("<p>    Run Ping")
                && text.contains("<n>    Copy Hostname")
        );
        assert!(!text.contains("Whois"));
        assert!(text.contains("\u{203A}  hop 1"));
        // The host leads the details, with an empty heart while it is no favorite.
        assert!(text.contains("fritz.box \u{2661}"));
        // The network sits above the actions.
        let network = lines
            .iter()
            .position(|line| line.contains("Private address"))
            .unwrap();
        let actions = lines
            .iter()
            .position(|line| line.contains("ACTIONS"))
            .unwrap();
        assert!(network < actions);
        // The table is still there and still navigable; the details follow.
        assert!(text.contains("HOST") && text.contains("AS13335 Cloudflare, Inc."));
        press(&mut app, KeyCode::End);
        let lines = screen(&mut app, 150, 40);
        let text = lines.join("\n");
        assert!(text.contains("Hop(one.one.one.one)[4]") && text.contains("\u{203A}  hop 4"));
        assert!(text.contains("<w>    Whois"));
        let network = lines
            .iter()
            .position(|line| line.contains("Organization:"))
            .unwrap();
        let actions = lines
            .iter()
            .position(|line| line.contains("ACTIONS"))
            .unwrap();
        assert!(network < actions);
        // Tab moves the keys to the details: the arrows pick an action and leave the hop alone.
        press(&mut app, KeyCode::Tab);
        assert!(app.details_focus);
        press(&mut app, KeyCode::Down);
        assert_eq!((app.action, app.hop), (1, 3));
        assert!(screen(&mut app, 150, 40)
            .join("\n")
            .contains("<tab>   Hops"));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.hop, 2);
        // A narrow terminal puts the details under the table.
        let narrow = screen(&mut app, 90, 40).join("\n");
        assert!(narrow.contains("Traceroute(one.one.one.one)[4]") && narrow.contains("Hop("));
        // Left closes the details, then leaves the run.
        press(&mut app, KeyCode::Left);
        assert!(!app.details && app.pane == Pane::Result);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.pane, Pane::Start);
    }

    #[test]
    fn an_mtr_is_a_live_table_with_history_and_the_rate_limit_note() {
        let mut app = app_with(vec![crate::mtr::tests::sample(12, |round| round % 3 == 0)]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 170, 40).join("\n");
        assert!(text.contains("MTR(example.com)[12 rounds]"));
        for column in [
            "HOST", "NETWORK", "LOSS", "SNT", "LAST", "AVG", "BEST", "WORST", "JITTER", "ROUNDS",
        ] {
            assert!(text.contains(column), "{column}");
        }
        assert!(text.contains("AS64500 Example Net"));
        assert!(text.contains("ae1.core2.fra.example.net"));
        assert!(text.contains("Hop 2 answers fewer probes than the hops after it."));
        assert!(text.contains("Stable. No loss over 12 packets."));
        assert!(text.contains("AT THE DESTINATION \u{00B7} HOP 4"));
        assert!(
            text.contains("LATENCY")
                && text.contains("JITTER")
                && text.contains("LOSS \u{00B7} 10-ROUND WINDOW")
        );
        assert!(text.contains("Status:   12 pkt \u{00B7} 0.0% loss \u{00B7} 12.2 ms avg"));
        assert!(text.contains("<v>     Rounds"));
        // v gives the room to the strips: the figures go, the history stays.
        press(&mut app, KeyCode::Char('v'));
        let text = screen(&mut app, 170, 40).join("\n");
        assert!(text.contains("ROUNDS") && text.contains("LOSS") && text.contains("AVG"));
        // The table drops its JITTER column. The jitter chart under it stays.
        assert!(!text.contains("WORST") && !text.contains("NETWORK"));
        assert!(text.contains("<v>     Figures"));
        // The hop's details draw that hop's own latency, and offer MTR on it.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 170, 44).join("\n");
        assert!(text.contains("Hop(ae1.core2.fra.example.net)[2]"));
        assert!(
            text.contains("  LATENCY") && text.contains("  JITTER") && text.contains("  LOSS"),
            "{text}"
        );
        assert!(text.contains("<m>    Run MTR"));
        // A narrow terminal keeps the hops, loss and the average.
        for line in screen(&mut app, 60, 30) {
            assert!(width(&line) <= 60);
        }
    }

    #[test]
    fn tab_switches_the_tool_and_a_saved_ping_shows_its_figures() {
        let times: Vec<Option<f64>> = (0..12)
            .map(|index| Some(10.0 + f64::from(index % 2) * 0.4))
            .collect();
        let mut app = app_with(vec![crate::ping::tests::sample(&times)]);
        assert_eq!(app.tool, Tool::Traceroute);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tool, Tool::Mtr);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tool, Tool::Ping);
        assert!(screen(&mut app, 130, 30)
            .join("\n")
            .contains("ping> <t> traceroute"));
        // In the field, tab goes on through the tools.
        press(&mut app, KeyCode::Char(':'));
        assert!(app.field);
        assert!(screen(&mut app, 130, 30)
            .join("\n")
            .contains("ping> Host or address"));
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tool, Tool::Dns);
        assert!(screen(&mut app, 130, 30)
            .join("\n")
            .contains("dig> Host or address"));
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tool, Tool::Bgp);
        assert!(screen(&mut app, 130, 30)
            .join("\n")
            .contains("bgp> Host, address, prefix or AS number"));
        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.tool, Tool::Dns);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.tool, Tool::Traceroute);
        press(&mut app, KeyCode::Esc);
        assert!(!app.field);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 130, 36).join("\n");
        assert!(text.contains("Ping(one.one.one.one)[12 pkt]"));
        assert!(
            text.contains("SENT 12") && text.contains("LOSS 0.0%") && text.contains("P95 10.4")
        );
        assert!(text.contains("Stable. No loss over 12 packets."));
        assert!(text.contains("LATENCY BY PACKET"));
        assert!(text.contains("SEQ") && text.contains("TTL"));
        assert!(!text.contains("Details"));
    }

    #[test]
    fn s_saves_the_open_run_and_cases_lists_saved_runs_while_there_is_no_case() {
        let mut run = sample();
        run.source = Some("192.168.1.20".parse().unwrap());
        run.public_source = Some("203.0.113.9".parse().unwrap());
        let mut app = app_with(vec![run]);
        press(&mut app, KeyCode::Char('#'));
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("Saved(all)[0]") && text.contains("No saved runs"));
        press(&mut app, KeyCode::Char('!'));
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("SOURCE") && text.contains("PUBLIC"));
        assert!(text.contains("192.168.1.20") && text.contains("203.0.113.9"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("<s>     Save") && text.contains("<a>     Add to Case"));
        press(&mut app, KeyCode::Char('s'));
        assert!(app.is_saved());
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("Saved") && text.contains("public 203.0.113.9"));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('#'));
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("Saved(all)[1]") && text.contains("one.one.one.one"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('s'));
        assert!(!app.is_saved());
        clean(&app);
    }

    #[test]
    fn the_picker_makes_a_case_and_cases_go_in_and_out() {
        let mut first = sample();
        first.id = "aaa222".into();
        let mut second = sample();
        second.id = "bbb333".into();
        second.target = "example.com".into();
        let mut app = app_with(vec![first, second]);
        // a on a selected run opens the picker; typing names a new case and Return makes it.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('a'));
        assert!(app.picker.is_some());
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("Cases(one.one.one.one)[0]") && text.contains("New case"));
        type_text(&mut app, "vpn drop");
        press(&mut app, KeyCode::Enter);
        assert!(app.saved.contains("aaa222"));
        assert!(app.cases.find("vpn drop").unwrap().holds("aaa222"));
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("[\u{2713}] vpn drop") && text.contains("Saved to vpn drop"));
        // Space takes it out again, and back in; the run stays saved either way.
        press(&mut app, KeyCode::Char(' '));
        assert!(!app.cases.find("vpn drop").unwrap().holds("aaa222"));
        assert!(app.saved.contains("aaa222"));
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Left);
        assert!(app.picker.is_none());
        // s on the second run saves it into the case used last.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('s'));
        assert!(app.cases.find("vpn drop").unwrap().holds("bbb333"));
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("Saved to vpn drop"));
        assert!(text.contains("\u{2605}  traceroute example.com"));
        // With a case, the tab's first level is All saved and the cases.
        press(&mut app, KeyCode::Char('#'));
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("Cases(all)[2]") && text.contains("All saved"));
        assert!(text.contains("vpn drop") && text.contains("used last"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.case, Some(Scope::Case(app.cases.cases[0].id.clone())));
        assert_eq!(app.row, Some(0));
        let text = screen(&mut app, 140, 30).join("\n");
        assert!(text.contains("vpn drop(all)[2]") && text.contains("\u{203A}  vpn drop"));
        assert!(text.contains("<x>     Remove from Case"));
        // x takes a run out of the case only.
        press(&mut app, KeyCode::Char('x'));
        assert_eq!(app.cases.cases[0].run_ids.len(), 1);
        assert_eq!(app.saved.runs.len(), 2);
        // Left comes back out to the cases, with the case selected.
        press(&mut app, KeyCode::Left);
        assert_eq!((app.case.clone(), app.row), (None, Some(1)));
        // All saved holds both.
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.case, Some(Scope::All));
        assert!(screen(&mut app, 140, 30)
            .join("\n")
            .contains("All saved(all)[2]"));
        // Removing a run from saved runs takes it out of every case.
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(app.saved.runs.len(), 1);
        assert!(app.cases.cases[0]
            .run_ids
            .iter()
            .all(|id| app.saved.contains(id)));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.case, None);
        clean(&app);
    }

    #[test]
    fn keys_sit_in_columns_and_return_goes_back_to_the_open_run() {
        let mut app = app_with(vec![sample()]);
        let lines = screen(&mut app, 150, 30);
        // More than four keys: a second column starts beside the first.
        let first = lines
            .iter()
            .find(|line| line.contains("<\u{21B5}>"))
            .unwrap();
        assert!(
            first.contains("<F>")
                || first.contains("<B>")
                || first.contains("<?>")
                || first.contains("<esc>"),
            "{first}"
        );
        assert!(lines.join("\n").contains("\u{21E7}3 Cases"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.pane, Pane::Start);
        assert!(screen(&mut app, 150, 30)
            .join("\n")
            .contains("<\u{21B5}>     Result"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.pane, Pane::Result);
    }

    #[test]
    fn on_the_overview_s_saves_the_selected_run_and_arrows_go_in_and_out() {
        let mut app = app_with(vec![sample()]);
        // Nothing selected: s has no run to save, and the field doesn't take it either.
        press(&mut app, KeyCode::Char('s'));
        assert!(app.input.is_empty() && app.saved.runs.is_empty());
        press(&mut app, KeyCode::Down);
        assert!(screen(&mut app, 150, 30)
            .join("\n")
            .contains("<s>     Save"));
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(app.saved.runs.len(), 1);
        let text = screen(&mut app, 150, 30).join("\n");
        assert!(text.contains("\u{2605}  traceroute") && text.contains("Unsave"));
        // Right goes into the run, left comes back, left again leaves the row.
        press(&mut app, KeyCode::Right);
        assert_eq!(app.pane, Pane::Result);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.pane, Pane::Start);
        assert_eq!(app.row, Some(0));
        press(&mut app, KeyCode::Left);
        assert_eq!(app.row, None);
        // In Cases, s on the run removes it and the list is empty again.
        press(&mut app, KeyCode::Char('#'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('s'));
        assert!(app.saved.runs.is_empty());
        assert_eq!(app.row, None);
        press(&mut app, KeyCode::Char('!'));
        assert_eq!(app.section, Section::Console);
        clean(&app);
    }

    /// Recent's runs to example.com by traceroute and ping, and one to one.one.one.one.
    fn three_runs() -> Vec<Run> {
        let mut other = sample();
        other.target = "example.com".into();
        other.id = "zzz999".into();
        let times: Vec<Option<f64>> = (0..12).map(|_| Some(10.0)).collect();
        let mut ping = crate::ping::tests::sample(&times);
        ping.target = "example.com".into();
        ping.id = "ppp111".into();
        vec![sample(), other, ping]
    }

    #[test]
    fn slash_searches_the_list_and_lands_on_the_first_hit() {
        let mut app = app_with(three_runs());
        // / opens the search line at the foot. Typing narrows Recent by tool and host.
        press(&mut app, KeyCode::Char('/'));
        assert!(app.searching);
        type_text(&mut app, "exa");
        let text = screen(&mut app, 100, 26).join("\n");
        assert!(text.contains("Recent(all)[2] </exa>"), "{text}");
        assert!(text.contains("example.com") && !text.contains("one.one.one.one"));
        assert!(text.lines().last().unwrap().starts_with(" /exa"), "{text}");
        assert_eq!(app.row, Some(0));
        // A second word narrows by tool.
        type_text(&mut app, " ping");
        assert!(screen(&mut app, 100, 26)
            .join("\n")
            .contains("Recent(all)[1] </exa ping>"));
        // Return keeps the search and gives the keys back to the list. Return again opens the run.
        press(&mut app, KeyCode::Enter);
        assert!(!app.searching && app.search == "exa ping");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.run().map(|run| run.tool), Some(Tool::Ping));
        // The search belonged to that list. Back on it, it is gone.
        press(&mut app, KeyCode::Left);
        assert!(app.search.is_empty());
        // A search that finds nothing says so, and esc drops it before it quits.
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "zzzz");
        assert!(screen(&mut app, 100, 26)
            .join("\n")
            .contains("Nothing matches"));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert!(app.search.is_empty() && !app.quit);
        // While the search line is open, esc drops it at once, and j is a letter of it.
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "j");
        assert_eq!(app.search, "j");
        press(&mut app, KeyCode::Esc);
        assert!(!app.searching && app.search.is_empty());
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert!(app.quit);
    }

    #[test]
    fn a_tools_letter_opens_the_field_and_typing_suggests_hosts() {
        let mut app = app_with(three_runs());
        app.favorites.toggle("exact.example.net").unwrap();
        assert!(!app.field);
        // p picks ping and gives the field the keys. What follows is the target.
        press(&mut app, KeyCode::Char('p'));
        assert_eq!((app.tool, app.field), (Tool::Ping, true));
        type_text(&mut app, "exa");
        assert_eq!(app.input, "exa");
        // Recent stays whole. The field suggests hosts in a box of its own: favorites first.
        assert_eq!(app.suggestions(), ["exact.example.net", "example.com"]);
        let text = screen(&mut app, 150, 30).join("\n");
        assert!(
            text.contains("ping> exa") && text.contains("Recent(all)[3]"),
            "{text}"
        );
        assert!(
            text.contains("\u{2665} exact.example.net")
                && text.contains("example.com        traceroute \u{00B7}"),
            "{text}"
        );
        // The tools show beside the field while it has the keys.
        assert!(text.contains("\u{25CF} ping"), "{text}");
        // A host that starts with what is typed comes before one that only holds it.
        type_text(&mut app, "mple");
        assert_eq!(app.suggestions(), ["example.com", "exact.example.net"]);
        // The arrows pick within the suggestions, and the list below doesn't move.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        assert_eq!((app.suggestion, app.row), (Some(1), None));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert_eq!(app.suggestion, None);
        // Right completes the field with the one picked, behind a tool named in front.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Right);
        assert_eq!((app.input.as_str(), app.suggestion), ("example.com", None));
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        type_text(&mut app, "mtr one");
        assert_eq!(app.suggestions(), ["one.one.one.one"]);
        app.key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.input, "mtr one.one.one.one");
        // esc empties the field and gives the keys back to the list.
        press(&mut app, KeyCode::Esc);
        assert!(!app.field && app.input.is_empty() && !app.quit);
        assert!(screen(&mut app, 150, 30)
            .join("\n")
            .contains("Recent(all)[3]"));
        let _ = std::fs::remove_file(app.favorites.path());
        // A tool's key drops a search, so what is typed alone narrows the suggestions.
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "zzz");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('t'));
        assert!(app.field && app.search.is_empty());
        press(&mut app, KeyCode::Esc);
        // m, d and b pick their tools the same way, and : the tool chosen last.
        for (letter, tool) in [('m', Tool::Mtr), ('d', Tool::Dns), ('b', Tool::Bgp)] {
            press(&mut app, KeyCode::Char(letter));
            assert_eq!((app.tool, app.field), (tool, true));
            press(&mut app, KeyCode::Esc);
        }
        press(&mut app, KeyCode::Char(':'));
        assert_eq!((app.tool, app.field), (Tool::Bgp, true));
        press(&mut app, KeyCode::Esc);
        // Return on an empty list opens the field too, for whoever expects to type.
        let mut empty = app_with(Vec::new());
        press(&mut empty, KeyCode::Enter);
        assert!(empty.field);
        // From an open run, : goes back to the field for a new target.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.pane, Pane::Result);
        press(&mut app, KeyCode::Char(':'));
        assert_eq!((app.pane, app.field), (Pane::Start, true));
    }

    #[test]
    fn slash_finds_hops_and_d_shows_the_one_found() {
        // The feedback's own sequence on a saved trace: a search, a step down, the details.
        let mut app = app_with(vec![sample()]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "1.1");
        // Hop 3 (198.51.100.7) and hop 4 (1.1.1.1) have it. The first is selected.
        assert_eq!(app.hop, 2);
        let text = screen(&mut app, 150, 40).join("\n");
        assert!(
            text.contains("Traceroute(one.one.one.one)[4] </1.1>"),
            "{text}"
        );
        assert!(
            text.contains("198.51.100.7") && !text.contains("fritz.box"),
            "{text}"
        );
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.hop, 3);
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.hop, 3);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.hop, 2);
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.hop, 3);
        press(&mut app, KeyCode::Char('d'));
        assert!(app.details);
        assert!(screen(&mut app, 150, 40)
            .join("\n")
            .contains("Hop(one.one.one.one)[4]"));
        press(&mut app, KeyCode::Char('d'));
        assert!(!app.details);
        // A network's name finds its hops too.
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "example net");
        assert_eq!(app.hop, 2);
        // esc on the search line leaves the selected hop where it is.
        press(&mut app, KeyCode::Esc);
        assert_eq!((app.hop, app.search.is_empty()), (2, true));
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "example net");
        // esc drops the search and stays on the run. The next esc leaves it.
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        assert!(app.search.is_empty() && app.pane == Pane::Result);
        assert!(screen(&mut app, 150, 40).join("\n").contains("fritz.box"));
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "zzz");
        assert!(screen(&mut app, 150, 40)
            .join("\n")
            .contains("No hop matches"));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.pane, Pane::Start);
    }

    #[test]
    fn help_opens_on_the_tool_in_view_with_its_commands() {
        let mut app = app_with(vec![sample()]);
        // On the list, ? shows the Console's keys.
        press(&mut app, KeyCode::Char('?'));
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(
            text.contains("Help(Keys)") && text.contains("THE CONSOLE"),
            "{text}"
        );
        assert!(text.contains("<?> Keys  <t> traceroute  <m> mtr"), "{text}");
        press(&mut app, KeyCode::Char('?'));
        assert!(app.help.is_none());
        // On a traceroute, ? shows its keys and the commands of rvrs traceroute.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('?'));
        let text = screen(&mut app, 150, 60).join("\n");
        for part in [
            "Help(traceroute)",
            "IN THE CONSOLE",
            "Search the hops by address, name or network",
            "ON THE COMMAND LINE",
            "rvrs traceroute [flags] <target>",
            "-T, --tcp",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        // A tool's letter turns to its page, tab to the next one.
        press(&mut app, KeyCode::Char('m'));
        let text = screen(&mut app, 150, 60).join("\n");
        assert!(
            text.contains("Help(mtr)") && text.contains("--report-cycles"),
            "{text}"
        );
        press(&mut app, KeyCode::Tab);
        assert!(screen(&mut app, 150, 60).join("\n").contains("Help(ping)"));
        press(&mut app, KeyCode::Char('d'));
        assert!(screen(&mut app, 150, 60).join("\n").contains("Help(dig)"));
        // The arrows scroll a page that is taller than the screen.
        press(&mut app, KeyCode::Char('b'));
        screen(&mut app, 150, 30);
        press(&mut app, KeyCode::Char('G'));
        let text = screen(&mut app, 150, 30).join("\n");
        assert!(text.contains("No ROA found means"), "{text}");
        // esc closes it, and the run is as it was.
        press(&mut app, KeyCode::Esc);
        assert!(app.help.is_none() && app.pane == Pane::Result);
        for line in screen(&mut app, 60, 20) {
            assert!(width(&line) <= 60);
        }
    }

    #[test]
    fn f_favorites_a_host_and_capital_f_lists_favorites() {
        let mut app = app_with(vec![sample()]);
        press(&mut app, KeyCode::Char('@'));
        let text = screen(&mut app, 150, 30).join("\n");
        assert!(text.contains("Favorites(all)[0]") && text.contains("No favorites"));
        press(&mut app, KeyCode::Char('!'));
        // f on a selected run keeps its target.
        press(&mut app, KeyCode::Down);
        assert!(screen(&mut app, 150, 30)
            .join("\n")
            .contains("<f>     Favorite"));
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.favorites.hosts, vec!["one.one.one.one"]);
        assert!(screen(&mut app, 150, 30)
            .join("\n")
            .contains("\u{2665} traceroute"));
        // f on a hop whose details are open keeps that hop's host.
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        assert!(screen(&mut app, 150, 40)
            .join("\n")
            .contains("fritz.box \u{2661}  <f> Favorite"));
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.favorites.hosts, vec!["fritz.box", "one.one.one.one"]);
        assert!(screen(&mut app, 150, 40)
            .join("\n")
            .contains("fritz.box \u{2665}  <f> Remove Favorite"));
        // Capital F goes to Favorites from the run. The list shows each host with its newest run.
        press(&mut app, KeyCode::Char('@'));
        assert_eq!((app.pane, app.section), (Pane::Start, Section::Favorites));
        let text = screen(&mut app, 150, 30).join("\n");
        assert!(text.contains("Favorites(all)[2]") && text.contains("fritz.box"));
        assert!(text.contains("traceroute \u{00B7} 4 hops \u{00B7} 12.4 ms"));
        // f on a favorite takes it out.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.favorites.hosts, vec!["one.one.one.one"]);
        let _ = std::fs::remove_file(app.favorites.path());
    }

    #[test]
    fn two_words_that_are_not_a_tool_and_a_target_are_not_run() {
        let mut app = app_with(Vec::new());
        press(&mut app, KeyCode::Char(':'));
        for character in "foo bar".chars() {
            press(&mut app, KeyCode::Char(character));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.shown.is_none());
        assert!(screen(&mut app, 120, 26)
            .join("\n")
            .contains("a tool and a target"));
    }

    /// Every fixture of the report's contract as a run another install made.
    fn fixture_runs() -> Vec<(String, Run)> {
        let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schema/fixtures");
        let mut names: Vec<String> = std::fs::read_dir(&folder)
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with(".json"))
            .collect();
        names.sort();
        names
            .into_iter()
            .map(|name| {
                let report: rvrs_core::report::Report =
                    serde_json::from_slice(&std::fs::read(folder.join(&name)).unwrap()).unwrap();
                let run = Run::from_report(&report, "abc234", Some("iPhone".into())).unwrap();
                (name, run)
            })
            .collect()
    }

    #[test]
    fn every_fixture_opens_from_its_report_alone() {
        for (name, run) in fixture_runs() {
            let report = run.report.clone().unwrap();
            let mut app = app_with(vec![run]);
            press(&mut app, KeyCode::Down);
            press(&mut app, KeyCode::Enter);
            let text = screen(&mut app, 170, 50).join("\n");
            assert!(text.contains(&report.target), "{name}\n{text}");
            // The maker's sentences are on screen as they came.
            for sentence in &report.interpretation {
                let start: String = sentence.chars().take(40).collect();
                assert!(text.contains(&start), "{name}: {start}\n{text}");
            }
        }
    }

    #[test]
    fn a_dns_lookup_from_its_report_shows_records_and_sources_without_tabs() {
        let (_, run) = fixture_runs()
            .into_iter()
            .find(|(name, _)| name == "dns.json")
            .unwrap();
        let report = run.report.clone().unwrap();
        let mut app = app_with(vec![run]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 170, 50).join("\n");
        for part in [
            report.summary.as_str(),
            "Origin:   iPhone",
            "Records",
            "Sources",
            "Match",
            "Run Again",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        assert!(!text.contains("<1> Overview"), "{text}");
        // Left goes back to the list, as from any run.
        press(&mut app, KeyCode::Left);
        assert!(screen(&mut app, 170, 50).join("\n").contains("Recent"));
    }

    #[test]
    fn a_ping_from_its_report_keeps_its_figures_and_its_makers_verdict() {
        let (_, run) = fixture_runs()
            .into_iter()
            .find(|(name, _)| name == "ping.json")
            .unwrap();
        let report = run.report.clone().unwrap();
        let mut app = app_with(vec![run]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 170, 50).join("\n");
        assert!(text.contains("LATENCY BY PACKET"), "{text}");
        assert!(
            text.contains(&format!("SENT {}", report.figure("sent").unwrap())),
            "{text}"
        );
        assert!(text.contains("Origin:   iPhone"), "{text}");
    }

    #[test]
    fn a_dns_lookup_has_four_tabs_and_a_record_opens_beside_them() {
        let mut app = app_with(vec![crate::dig::tests::finished()]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 150, 44).join("\n");
        for part in [
            "DNS(example.com)[4 records]",
            "<1> Overview  <2> Records  <3> Compare  <4> Trace",
            "Resolver: Router \u{00B7} 192.168.1.1",
            "DNSSEC:   None",
            "Resolved \u{00B7} 4 ms \u{00B7} all agree \u{00B7} unsigned",
            "All sources agree",
            "KEY RECORDS",
            "RESOLUTION PATH",
            "Root a.root-servers.net",
            "First saved lookup of this name.",
            "<1-4>",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }

        // Right opens the selected record beside the records, with what can be done with it.
        press(&mut app, KeyCode::Right);
        let text = screen(&mut app, 150, 44).join("\n");
        for part in [
            "Record(A)[5m]",
            "TTL:      300 s \u{00B7} 5m",
            "Value:    192.0.2.10",
            "An IPv4 address for the name.",
            "<t>    Run Traceroute",
            "<z>    Copy as Zone Line",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        // Tab gives the keys to the details, left gives them back, then closes the details.
        press(&mut app, KeyCode::Tab);
        assert!(app.details_focus);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        assert!(!app.details && app.pane == Pane::Result);

        // A record that names no host offers the two copies and no tool.
        let txt = Record {
            name: "example.com".into(),
            type_code: dns::types::TXT,
            ttl: 300,
            value: "\"v=spf1 -all\"".into(),
            section: dns::Section::Answer,
        };
        assert_eq!(
            record_actions(&txt),
            [Action::CopyValue, Action::CopyZoneLine]
        );

        press(&mut app, KeyCode::Char('2'));
        let text = screen(&mut app, 150, 44).join("\n");
        for part in [
            "192.168.1.1 \u{00B7} 4 ms \u{00B7} from this device",
            "ADDRESSES",
            "EMAIL",
            "INFRASTRUCTURE",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        // / narrows the records to the ones it finds: here the MX records.
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "mx");
        let text = screen(&mut app, 150, 44).join("\n");
        assert!(
            text.contains("EMAIL") && !text.contains("ADDRESSES") && text.contains(" /mx"),
            "{text}"
        );
        press(&mut app, KeyCode::Enter);
        // d opens the record it found.
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(
            app.dns_record().map(|record| record.type_name()),
            Some("MX".into())
        );
        assert!(app.details);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert!(app.search.is_empty() && !app.details && app.pane == Pane::Result);
        press(&mut app, KeyCode::Char('o'));
        assert!(screen(&mut app, 150, 44)
            .join("\n")
            .contains("1.1.1.1 \u{00B7} 9 ms"));

        // Compare lists the sources, and Return on one opens its records.
        press(&mut app, KeyCode::Char('3'));
        let text = screen(&mut app, 150, 44).join("\n");
        for part in [
            "SOURCE",
            "STATUS",
            "Device, public resolvers and the zone agree",
            "Match",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!((app.dns_tab, app.dns_source), (DnsTab::Records, 2));
        assert!(screen(&mut app, 150, 44)
            .join("\n")
            .contains("authoritative"));

        press(&mut app, KeyCode::Char('4'));
        let text = screen(&mut app, 150, 44).join("\n");
        for part in [
            "DELEGATION PATH",
            "delegates com to 2 servers \u{00B7} DS",
            "No DNSSEC",
            "com publishes no DS for example.com",
            "It doesn't validate signatures.",
        ] {
            assert!(text.contains(part), "{part}\n{text}");
        }
        // Tab goes on to the next tab, around to the first.
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.dns_tab, DnsTab::Overview);
        press(&mut app, KeyCode::Left);
        assert!(app.pane == Pane::Start);
        clean(&app);
    }

    #[test]
    fn a_bgp_lookup_has_the_apps_tabs_and_a_path_opens_beside_them() {
        let mut app = app_with(vec![crate::bgp::tests::finished()]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(text.contains("BGP(example.com)[3 paths]"), "{text}");
        assert!(text.contains("Source:   RIPE RIS collectors"), "{text}");
        assert!(text.contains("Prefix:   192.0.2.0/24"), "{text}");
        assert!(text.contains("Network:  AS64500 Example Net"), "{text}");
        assert!(
            text.contains("AS64500 \u{00B7} Valid \u{00B7} 318/320 peers"),
            "{text}"
        );
        assert!(
            text.contains("<1> Overview  <2> Paths  <3> Prefixes  <4> History  <5> Sources"),
            "{text}"
        );
        assert!(text.contains("Routed and authorized"), "{text}");
        assert!(
            text.contains("Seen by 318/320 peers    Paths 3    First seen 2017"),
            "{text}"
        );
        assert!(
            text.contains("ROUTE") && text.contains("Announced prefix   192.0.2.0/24"),
            "{text}"
        );
        assert!(
            text.contains("REACHED THROUGH") && text.contains("Transit One"),
            "{text}"
        );
        assert!(
            text.contains("Trace the Path  traceroute to example.com"),
            "{text}"
        );

        // Paths: every distinct path, and the selected one network by network beside them.
        press(&mut app, KeyCode::Char('2'));
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(
            text.contains("Shortest 2 networks   Typical 3 networks   Direct 5.7% of peers"),
            "{text}"
        );
        assert!(text.contains("OBSERVED AS PATHS"), "{text}");
        assert!(
            text.contains("62.9%  AS64510 \u{203A} AS64501 \u{203A} AS64500"),
            "{text}"
        );
        press(&mut app, KeyCode::Right);
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(text.contains("AS Path(3 networks)[62.9%]"), "{text}");
        assert!(
            text.contains("Collector peer")
                && text.contains("Transit")
                && text.contains("Origin AS")
        );
        assert!(text.contains("Seen by:      200 of 318 peers"), "{text}");
        assert!(
            text.contains("COLLECTORS") && text.contains("RRC01 \u{00B7} London, GB"),
            "{text}"
        );
        assert!(
            text.contains("COMMUNITIES") && text.contains("64501:100"),
            "{text}"
        );
        assert!(text.contains("<tab>   Details"), "{text}");
        press(&mut app, KeyCode::Tab);
        assert!(app.details_focus);
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        assert!(!app.details);

        // Prefixes: the network's list, filtered, and a prefix's RPKI result on v.
        press(&mut app, KeyCode::Char('3'));
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(text.contains("ORIGINATED BY AS64500"), "{text}");
        assert!(
            text.contains("192.0.2.0/24   Valid     This route"),
            "{text}"
        );
        assert!(text.contains("2001:db8::/32"), "{text}");
        assert!(text.contains("<v>     RPKI"), "{text}");
        // / narrows the rows to what it finds and lands on the first.
        press(&mut app, KeyCode::Char('/'));
        type_text(&mut app, "2001");
        press(&mut app, KeyCode::Enter);
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(
            text.contains("2001:db8::/32") && !text.contains("192.0.2.0/24   Valid"),
            "{text}"
        );
        assert!(text.contains("</2001>"), "{text}");
        press(&mut app, KeyCode::Esc);
        assert!(app.search.is_empty() && app.pane == Pane::Result);
        app.bgp_row = 0;
        // d shows the selected prefix's RPKI result, as v does, and hides it again.
        press(&mut app, KeyCode::Char('d'));
        assert!(app.details);
        press(&mut app, KeyCode::Char('d'));
        assert!(!app.details);
        press(&mut app, KeyCode::Char('v'));
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(text.contains("RPKI(AS64500)[Valid]"), "{text}");
        assert!(text.contains("Origin authorized"), "{text}");
        assert!(text.contains("Prefix length allowed"), "{text}");
        assert!(text.contains("COVERING ROAS"), "{text}");
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Char('o'));
        press(&mut app, KeyCode::Char('o'));
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(
            text.contains("2001:db8::/32") && !text.contains("This route"),
            "{text}"
        );

        // History and Sources.
        press(&mut app, KeyCode::Char('4'));
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(
            text.contains(" 7 d ") && text.contains("No announcement on record"),
            "{text}"
        );
        assert!(
            text.contains("EVENTS") && text.contains("No changes"),
            "{text}"
        );
        press(&mut app, KeyCode::Char('o'));
        assert_eq!(app.bgp_days, 30);
        press(&mut app, KeyCode::Tab);
        let text = screen(&mut app, 150, 50).join("\n");
        assert!(
            text.contains("Origin AS and holder") && text.contains("RIPEstat prefix-overview"),
            "{text}"
        );
        assert!(
            text.contains("Each part was observed at its own time"),
            "{text}"
        );
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.bgp_tab, BgpTab::Overview);
        clean(&app);
    }

    #[test]
    fn an_as_number_or_a_prefix_typed_under_another_tool_goes_to_bgp() {
        let mut app = app_with(Vec::new());
        assert_eq!(app.tool, Tool::Traceroute);
        press(&mut app, KeyCode::Char(':'));
        type_text(&mut app, "bgp nonsense words");
        press(&mut app, KeyCode::Enter);
        assert!(app.shown.is_none());
        assert!(rvrs_core::bgp::Target::is_routing_only("AS64500"));
        assert!(rvrs_core::bgp::Target::is_routing_only("192.0.2.0/24"));
        clean(&app);
    }

    #[test]
    fn r_picks_a_tool_for_a_run_or_a_host_and_capital_r_runs_again() {
        let mut app = app_with(vec![sample()]);
        // On a run in Recent, r opens the picker with the run's own tool selected.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('r'));
        let launcher = app.launcher.as_ref().unwrap();
        assert_eq!(launcher.target, "one.one.one.one");
        assert_eq!(
            launcher.tools,
            [
                Tool::Traceroute,
                Tool::Mtr,
                Tool::Ping,
                Tool::Dns,
                Tool::Bgp
            ]
        );
        assert_eq!(launcher.tools[launcher.row], Tool::Traceroute);
        let text = screen(&mut app, 140, 34).join("\n");
        assert!(text.contains("Run(one.one.one.one)[5 tools]"), "{text}");
        assert!(
            text.contains("<m>  mtr") && !text.contains("Loss and latency"),
            "{text}"
        );
        assert!(text.contains("<\u{21B5}>     Run"), "{text}");
        press(&mut app, KeyCode::Down);
        assert_eq!(app.launcher.as_ref().unwrap().row, 1);
        press(&mut app, KeyCode::Esc);
        assert!(app.launcher.is_none());

        // In a result, r takes the selected hop's host. A private one has no BGP.
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.pane, Pane::Result);
        app.hop = 0;
        let hop_host = app.selected_result_host().unwrap();
        press(&mut app, KeyCode::Char('r'));
        let launcher = app.launcher.as_ref().unwrap();
        assert_eq!(launcher.target, hop_host);
        assert_eq!(launcher.row, 0);
        press(&mut app, KeyCode::Left);
        assert!(app.launcher.is_none());

        assert_eq!(run_picker::tools_for("AS64500"), [Tool::Bgp]);
        assert_eq!(run_picker::tools_for("192.0.2.0/24"), [Tool::Bgp]);
        assert_eq!(
            run_picker::tools_for("192.168.1.1"),
            [Tool::Traceroute, Tool::Mtr, Tool::Ping]
        );
        assert_eq!(
            run_picker::tools_for("192.0.2.1"),
            [Tool::Traceroute, Tool::Mtr, Tool::Ping, Tool::Bgp]
        );
        clean(&app);
    }

    #[test]
    fn the_last_tool_is_remembered_and_iperf3_is_listed_last() {
        let path = std::env::temp_dir().join(format!(
            "rvrs-console-tool-{}/console.json",
            std::process::id()
        ));
        assert_eq!(last_tool(&path), None);
        keep_tool(&path, Tool::Bgp);
        assert_eq!(last_tool(&path), Some(Tool::Bgp));
        keep_tool(&path, Tool::Iperf3);
        assert_eq!(last_tool(&path), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
        assert_eq!(Tool::ALL.last(), Some(&Tool::Iperf3));
    }

    #[test]
    fn page_up_and_page_down_move_a_screenful() {
        let runs: Vec<Run> = (0..40)
            .map(|index| {
                let mut run = sample();
                run.id = format!("page{index:02}");
                run
            })
            .collect();
        let mut app = app_with(runs);
        screen(&mut app, 120, 30);
        let page = app.page;
        assert!(page > 5, "{page}");
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.row, Some(0));
        press(&mut app, KeyCode::PageDown);
        assert_eq!(app.row, Some(page));
        for _ in 0..10 {
            press(&mut app, KeyCode::PageDown);
        }
        assert_eq!(app.row, Some(39));
        press(&mut app, KeyCode::PageUp);
        assert_eq!(app.row, Some(39 - page));
        for _ in 0..10 {
            press(&mut app, KeyCode::PageUp);
        }
        assert_eq!(app.row, None);
        clean(&app);
    }

    #[test]
    fn the_logo_is_whole_cut_or_gone_by_width() {
        for (row, mask) in LOGO.iter().zip(LOGO_MASK) {
            assert!(
                row.chars().count() >= mask.trim_end().chars().count(),
                "{row}"
            );
        }
        assert_eq!(logo_width(), 71);
        let text = |width: u16| screen(&mut app_with(Vec::new()), width, 30).join("\n");
        let wide = text(200);
        assert!(
            wide.contains("rvrs        \u{00B7}:\u{00B7}\u{00B7} \u{00B7}            : snp!"),
            "{wide}"
        );
        assert!(wide.contains("/__\\     \\  ____  \\   _\\ \\   networking"));
        let cut = text(140);
        assert!(
            cut.contains("===/__/__/========______/==============\u{00BB}"),
            "{cut}"
        );
        assert!(!cut.contains("snp!"));
        assert!(!text(120).contains("networking"));
    }

    #[test]
    fn charts_scale_to_the_tallest_value_and_mark_what_is_missing() {
        let lines = column_chart(&[Some(10.0), Some(5.0), None], 2, 1, 0, None, 40);
        let text: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect()
            })
            .collect();
        assert_eq!(text[0], "    10.0 \u{2524}\u{2588}  ");
        assert_eq!(text[1], "    0 ms \u{2524}\u{2588}\u{2588}\u{00D7}");
    }

    #[test]
    fn jitter_is_the_move_between_replies_and_loss_a_ten_round_window() {
        let reply = |millis: u64| Slot::Reply {
            from: "192.0.2.1".parse().unwrap(),
            micros: millis * 1000,
            unreachable: None,
            ttl: None,
        };
        let hop = Hop {
            ttl: 1,
            probes: vec![
                reply(10),
                reply(14),
                Slot::Lost,
                reply(12),
                reply(11),
                Slot::Pending,
            ],
            is_destination: false,
            reported: None,
        };
        let jitter = jitter_series(&hop);
        assert_eq!(jitter.len(), 5);
        assert_eq!(jitter[0], None);
        assert!((jitter[1].unwrap() - 4.0).abs() < 1e-9);
        assert_eq!((jitter[2], jitter[3]), (None, None));
        assert!((jitter[4].unwrap() - 1.0).abs() < 1e-9);
        let loss = loss_series(&hop);
        assert_eq!(loss[1], Some(0.0));
        assert!((loss[2].unwrap() - 100.0 / 3.0).abs() < 1e-9);
        assert_eq!(loss[4], Some(20.0));

        // No loss draws a baseline at zero, not an empty chart.
        let lines = metric_chart(Metric::Loss, &[Some(0.0), Some(0.0)], 2, 1, 0, None, 40);
        let text: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect()
            })
            .collect();
        assert_eq!(text[1], "      0% \u{2524}\u{2581}\u{2581}");
    }

    #[test]
    fn latency_has_a_color_and_always_a_number() {
        assert_eq!(latency_color(Duration::from_millis(12)), Color::Green);
        assert_eq!(latency_color(Duration::from_millis(80)), Color::Yellow);
        assert_eq!(latency_color(Duration::from_millis(300)), Color::Red);
    }

    #[test]
    fn each_figure_draws_in_its_own_color() {
        let colors = |metric: Metric, values: &[Option<f64>]| -> Vec<Option<Color>> {
            metric_chart(metric, values, 1, 1, 0, None, 40)[0]
                .spans
                .iter()
                .skip(1)
                .map(|span| span.style.fg)
                .collect()
        };
        // COLOR.md's colors for a terminal: jitter is magenta, as p99 and upload are, and loss
        // is red.
        assert_eq!(
            colors(Metric::Jitter, &[Some(4.0), Some(1.0)]),
            [Some(Color::Magenta); 2]
        );
        assert_eq!(colors(Metric::Loss, &[Some(20.0)]), [Some(Color::Red)]);
        // Latency keeps its three steps, and a probe without a reply is a red mark.
        assert_eq!(
            colors(
                Metric::Latency,
                &[Some(12.0), Some(80.0), Some(300.0), None]
            ),
            [
                Some(Color::Green),
                Some(Color::Yellow),
                Some(Color::Red),
                Some(Color::Red)
            ]
        );
    }

    #[test]
    fn loss_that_reaches_the_destination_is_red_and_a_network_is_ink() {
        // The sample's destination answers two probes of three.
        let mut run = sample();
        let destination = run.hops.iter_mut().find(|hop| hop.is_destination).unwrap();
        destination.probes[2] = Slot::Lost;
        let mut app = app_with(vec![run]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        // Hop 2 selected, so the rows of hop 1 and the destination show their own colors.
        press(&mut app, KeyCode::Down);
        let loss = colors_of(&mut app, 150, 40, "33%");
        assert!(loss.iter().all(|color| *color == Color::Red), "{loss:?}");
        let network = colors_of(&mut app, 150, 40, "AS64500 Example Net");
        assert!(
            network.iter().all(|color| *color == Color::Reset),
            "{network:?}"
        );
        clean(&app);

        // Every lost packet of a ping is loss at the target: the tile and the verdict are red.
        let times: Vec<Option<f64>> = (0..12)
            .map(|index| (index % 6 != 3).then_some(10.0))
            .collect();
        let mut app = app_with(vec![crate::ping::tests::sample(&times)]);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        let tile = colors_of(&mut app, 150, 40, "LOSS 16.7%");
        assert_eq!(tile[..5], [LABEL; 5]);
        assert!(
            tile[5..].iter().all(|color| *color == Color::Red),
            "{tile:?}"
        );
        let verdict = colors_of(&mut app, 150, 40, "Packet loss.");
        assert!(
            verdict.iter().all(|color| *color == Color::Red),
            "{verdict:?}"
        );
        clean(&app);
    }

    #[test]
    fn painted_lines_become_spans() {
        let style = Style { color: true };
        let line = ansi_line(&format!(
            "{} plain {}",
            style.bold_green("Reached"),
            style.dim("12.4 ms")
        ));
        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[0].content, "Reached");
        assert_eq!(line.spans[0].style.fg, Some(Color::Green));
    }

    #[test]
    fn base64_for_the_clipboard_escape() {
        assert_eq!(base64(b"1.1.1.1"), "MS4xLjEuMQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
    }
}
