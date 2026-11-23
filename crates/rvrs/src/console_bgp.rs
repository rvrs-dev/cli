//! The BGP result in the Console: the app's tabs on a terminal. For a host, an address or a
//! prefix they are Overview, Paths, Prefixes and History. For a network they are Overview,
//! Neighbors and Prefixes. Sources is the last tab of both. Every network and prefix on these
//! screens opens its own lookup, and a path or an RPKI result opens beside the list.

use rvrs_core::bgp::glass::GlassPath;
use rvrs_core::bgp::result::{count, counted, date, date_time, no_route_line};
use rvrs_core::bgp::{
    BgpResult, Glasses, Grade as BgpGrade, Path, RpkiStatus, Side, Target, TargetKind,
    TraceComparison, Validation,
};

use super::*;
use crate::bgp as cli;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BgpTab {
    Overview,
    Paths,
    Neighbors,
    Prefixes,
    History,
    Sources,
}

impl BgpTab {
    fn title(self) -> &'static str {
        match self {
            BgpTab::Overview => "Overview",
            BgpTab::Paths => "Paths",
            BgpTab::Neighbors => "Neighbors",
            BgpTab::Prefixes => "Prefixes",
            BgpTab::History => "History",
            BgpTab::Sources => "Sources",
        }
    }

    /// The tabs a result has, in the app's order.
    fn of(bgp: &BgpResult) -> Vec<BgpTab> {
        if bgp.unrouted_reason.is_some() {
            vec![BgpTab::Overview]
        } else if bgp.is_network() {
            vec![
                BgpTab::Overview,
                BgpTab::Neighbors,
                BgpTab::Prefixes,
                BgpTab::Sources,
            ]
        } else {
            vec![
                BgpTab::Overview,
                BgpTab::Paths,
                BgpTab::Prefixes,
                BgpTab::History,
                BgpTab::Sources,
            ]
        }
    }
}

/// Which of a network's prefixes the Prefixes tab lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrefixFilter {
    All,
    V4,
    V6,
    Valid,
    Invalid,
    NotFound,
}

impl PrefixFilter {
    const ALL: [PrefixFilter; 6] = [
        PrefixFilter::All,
        PrefixFilter::V4,
        PrefixFilter::V6,
        PrefixFilter::Valid,
        PrefixFilter::Invalid,
        PrefixFilter::NotFound,
    ];

    fn title(self) -> &'static str {
        match self {
            PrefixFilter::All => "All",
            PrefixFilter::V4 => "IPv4",
            PrefixFilter::V6 => "IPv6",
            PrefixFilter::Valid => "Valid",
            PrefixFilter::Invalid => "Invalid",
            PrefixFilter::NotFound => "Not Found",
        }
    }

    fn takes(self, prefix: &str, status: Option<RpkiStatus>) -> bool {
        match self {
            PrefixFilter::All => true,
            PrefixFilter::V4 => !prefix.contains(':'),
            PrefixFilter::V6 => prefix.contains(':'),
            PrefixFilter::Valid => status == Some(RpkiStatus::Valid),
            PrefixFilter::Invalid => status.is_some_and(RpkiStatus::is_invalid),
            PrefixFilter::NotFound => status == Some(RpkiStatus::NotFound),
        }
    }
}

/// What a selectable row stands for, and so what Return does with it.
#[derive(Debug, Clone, PartialEq)]
enum Item {
    /// Opens the network's own lookup.
    Network(u32),
    /// Opens the prefix's own lookup. With an RPKI result, `v` shows it beside the list.
    Prefix(String, Option<Validation>),
    /// Another address of the hostname. Opens its lookup.
    Address(String),
    Validation(Validation),
    Path(Path),
    GlassPath(usize, GlassPath),
    /// Goes to another tab.
    Tab(BgpTab),
    /// The traceroute that was laid against the paths. Opens it.
    OpenRun(Box<Run>),
    Start(Tool, String),
    Whois(String),
    /// A fact to read and copy.
    Text(String),
}

impl Item {
    /// What `c` copies.
    fn text(&self) -> Option<String> {
        match self {
            Item::Network(asn) => Some(format!("AS{asn}")),
            Item::Prefix(prefix, _) => Some(prefix.clone()),
            Item::Address(address) => Some(address.clone()),
            Item::Validation(validation) => Some(validation.status.title().to_string()),
            Item::Path(path) => Some(asns_text(&path.asns)),
            Item::GlassPath(_, path) => Some(asns_text(&path.asns)),
            Item::Whois(url) => Some(url.clone()),
            Item::Text(text) => Some(text.clone()),
            Item::Tab(_) | Item::OpenRun(_) | Item::Start(..) => None,
        }
    }
}

fn asns_text(asns: &[u32]) -> String {
    asns.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

/// What opens beside the list for the selected row.
enum Detail {
    Path(Path),
    GlassPath(usize, GlassPath),
    Validation(Validation),
}

impl Detail {
    /// The networks the details list, each of which opens its own lookup.
    fn networks(&self) -> Vec<u32> {
        match self {
            Detail::Path(path) => path.asns.clone(),
            Detail::GlassPath(_, path) => path.asns.clone(),
            Detail::Validation(validation) => vec![validation.origin],
        }
    }
}

/// One tab, drawn: its lines, what each selectable row stands for, and the line the selected
/// row is on.
struct Screen {
    lines: Vec<Line<'static>>,
    items: Vec<Item>,
    at: usize,
    row: usize,
    columns: usize,
}

impl Screen {
    fn blank(&mut self) {
        self.lines.push(Line::default());
    }

    fn header(&mut self, text: &str) {
        if !self.lines.is_empty() {
            self.blank();
        }
        self.lines.push(Line::styled(
            format!(" {text}"),
            Paint::new().add_modifier(Modifier::BOLD),
        ));
    }

    fn sentence(&mut self, text: &str, paint: Paint) {
        for line in pretty::wrap(text, self.columns.saturating_sub(2).min(100)) {
            self.lines.push(Line::styled(format!(" {line}"), paint));
        }
    }

    fn note(&mut self, text: &str) {
        self.sentence(text, Paint::new().add_modifier(Modifier::DIM));
    }

    /// A selectable row. The selected one is drawn across the whole width.
    fn item(&mut self, spans: Vec<Span<'static>>, item: Item) {
        if self.items.len() == self.row {
            self.at = self.lines.len();
            let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
            self.lines
                .push(Line::styled(fit(&text, self.columns), selected()));
        } else {
            self.lines.push(Line::from(spans));
        }
        self.items.push(item);
    }

    fn fact(&mut self, label: &str, value: Span<'static>, item: Item) {
        self.item(
            vec![
                Span::styled(format!(" {:<19}", label), Paint::new().fg(LABEL)),
                value,
            ],
            item,
        );
    }
}

fn dim() -> Paint {
    Paint::new().add_modifier(Modifier::DIM)
}

fn bold() -> Paint {
    Paint::new().add_modifier(Modifier::BOLD)
}

fn grade_color(grade: BgpGrade) -> Option<Color> {
    match grade {
        BgpGrade::Good => Some(Color::Green),
        BgpGrade::Attention => Some(Color::Yellow),
        BgpGrade::Bad => Some(Color::Red),
        BgpGrade::Neutral => None,
    }
}

fn badge(status: RpkiStatus) -> Span<'static> {
    let paint = match grade_color(status.grade()) {
        Some(color) => bold().fg(color),
        None => dim(),
    };
    Span::styled(status.badge(), paint)
}

/// "AS64510 › AS64501 › AS64500", the last network in bold.
fn chain(asns: &[u32]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (index, asn) in asns.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" \u{203A} ", dim()));
        }
        spans.push(if index + 1 == asns.len() {
            Span::styled(format!("AS{asn}"), bold())
        } else {
            Span::raw(format!("AS{asn}"))
        });
    }
    spans
}

/// Choices in a row, the open one marked: the sides of Neighbors, the filters of Prefixes.
fn chips(labels: Vec<(String, bool)>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (label, on) in labels {
        spans.push(if on {
            Span::styled(
                format!(" {label} "),
                selected().add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(format!(" {label} "), dim())
        });
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

impl App {
    fn bgp(&self) -> Option<&BgpResult> {
        self.run()?.bgp.as_ref()
    }

    /// The open tab, or Overview when this result doesn't have it.
    fn bgp_shown_tab(&self, bgp: &BgpResult) -> BgpTab {
        if BgpTab::of(bgp).contains(&self.bgp_tab) {
            self.bgp_tab
        } else {
            BgpTab::Overview
        }
    }

    /// Starts a BGP lookup with the user's looking glasses. False when the text is none of a
    /// hostname, an address, a prefix or an AS number.
    pub(super) fn bgp_session(target: &str) -> Option<Session> {
        let target = Target::parse(target)?;
        Some(Session::start_bgp(
            &target,
            Glasses::open().list,
            Lookups::ALL,
        ))
    }

    pub(super) fn reset_bgp(&mut self) {
        self.bgp_tab = BgpTab::Overview;
        self.bgp_row = 0;
        self.bgp_side = Side::Left;
        self.bgp_filter = PrefixFilter::All;
    }

    fn set_bgp_tab(&mut self, tab: BgpTab) {
        if self.bgp_tab != tab {
            self.bgp_tab = tab;
            self.bgp_row = 0;
            self.details = false;
            self.details_focus = false;
        }
    }

    /// What opens beside the list for the selected row, if it has details.
    fn bgp_detail(&self) -> Option<Detail> {
        let run = self.run()?;
        let screen = self.bgp_screen(run, 100);
        match screen.items.into_iter().nth(self.bgp_row)? {
            Item::Path(path) => Some(Detail::Path(path)),
            Item::GlassPath(glass, path) => Some(Detail::GlassPath(glass, path)),
            Item::Validation(validation) | Item::Prefix(_, Some(validation)) => {
                Some(Detail::Validation(validation))
            }
            _ => None,
        }
    }

    /// The host the selected row names, for the run picker.
    pub(super) fn bgp_selected_host(&self) -> Option<String> {
        let screen = self.bgp_screen(self.run()?, 100);
        match screen.items.into_iter().nth(self.bgp_row)? {
            Item::Network(asn) => Some(format!("AS{asn}")),
            Item::Prefix(prefix, _) => Some(prefix),
            Item::Address(address) | Item::Start(_, address) => Some(address),
            Item::Validation(validation) => Some(validation.prefix),
            _ => None,
        }
    }

    pub(super) fn bgp_has_details(&self) -> bool {
        self.bgp_detail().is_some()
    }

    /// "BGP" and what the panel's title counts: paths for a route, prefixes for a network.
    pub(super) fn bgp_count(run: &Run) -> String {
        let Some(bgp) = &run.bgp else {
            return "0 paths".to_string();
        };
        if bgp.is_network() {
            let total = bgp
                .network
                .as_ref()
                .and_then(|network| network.prefix_total())
                .unwrap_or(0);
            counted(total, "prefix", "prefixes")
        } else if bgp.path_count == 1 {
            "1 path".to_string()
        } else {
            format!("{} paths", bgp.path_count)
        }
    }

    /// The Status line of the header: how far the lookup is, or its headline in the verdict's
    /// color.
    pub(super) fn bgp_status(&self, run: &Run, style: Style) -> String {
        let found = run.bgp.as_ref().filter(|bgp| !bgp.is_empty());
        if run.is_running() {
            let progress = run
                .bgp
                .as_ref()
                .map_or("Finding the route".to_string(), BgpResult::progress);
            return format!(
                "{} {progress}",
                style.blue(SPINNER[(self.frame / 2) % SPINNER.len()])
            );
        }
        if matches!(run.outcome, Some(Outcome::Failed(_))) {
            return style.red("Failed");
        }
        match found {
            None => style.yellow(&run.summary()),
            Some(bgp) => {
                let headline = if run.outcome == Some(Outcome::Stopped) {
                    format!("Stopped \u{00B7} {}", bgp.headline())
                } else {
                    bgp.headline()
                };
                match bgp.verdict().grade {
                    BgpGrade::Good => style.bold_green(&headline),
                    BgpGrade::Attention => style.yellow(&headline),
                    BgpGrade::Bad => style.red(&headline),
                    BgpGrade::Neutral => style.bold(&headline),
                }
            }
        }
    }

    /// The header's rows a lookup has in place of an address and its network: who was asked,
    /// the prefix and the origin AS.
    pub(super) fn bgp_info(run: &Run) -> [(&'static str, String); 3] {
        let dash = || "\u{2013}".to_string();
        let Some(bgp) = &run.bgp else {
            return [("Source", dash()), ("Prefix", dash()), ("Network", dash())];
        };
        let glasses = bgp.glasses.len();
        let source = match glasses {
            0 => "RIPE RIS collectors".to_string(),
            1 => "RIPE RIS collectors \u{00B7} 1 looking glass".to_string(),
            _ => format!("RIPE RIS collectors \u{00B7} {glasses} looking glasses"),
        };
        let network = bgp
            .origin()
            .map(|origin| origin.asn)
            .or(bgp.network.as_ref().map(|network| network.asn))
            .map_or_else(dash, |asn| bgp.label(asn));
        [
            ("Source", source),
            ("Prefix", bgp.prefix.clone().unwrap_or_else(dash)),
            ("Network", network),
        ]
    }

    pub(super) fn bgp_keys(&self) -> Vec<(&'static str, &'static str)> {
        if self.details {
            return if self.details_focus {
                vec![
                    ("\u{2191}\u{2193}", "Network"),
                    ("\u{21B5}", "Open Network"),
                    ("tab", "List"),
                    ("\u{2190}", "List"),
                ]
            } else {
                vec![
                    ("\u{2191}\u{2193}", "Row"),
                    ("tab", "Details"),
                    ("c", "Copy"),
                    ("\u{2190}", "Close"),
                ]
            };
        }
        let Some(run) = self.run() else {
            return Vec::new();
        };
        let empty = BgpResult::new(TargetKind::Host, &run.target);
        let bgp = run.bgp.as_ref().unwrap_or(&empty);
        let tab = self.bgp_shown_tab(bgp);
        let screen = self.bgp_screen(run, 100);
        let enter = match screen.items.get(self.bgp_row) {
            Some(Item::Network(_)) => Some("Open Network"),
            Some(Item::Prefix(..)) => Some("Open Prefix"),
            Some(Item::Address(_)) => Some("Open Address"),
            Some(Item::Validation(_)) => Some("RPKI"),
            Some(Item::Path(_) | Item::GlassPath(..)) => Some("AS Path"),
            Some(Item::Tab(tab)) => Some(tab.title()),
            Some(Item::OpenRun(_)) => Some("Open Traceroute"),
            Some(Item::Start(..)) => Some("Trace the Path"),
            Some(Item::Whois(_)) => Some("Whois"),
            Some(Item::Text(_)) | None => None,
        };
        let mut keys = vec![("\u{2191}\u{2193}", "Row")];
        if let Some(enter) = enter {
            keys.push(("\u{2192}", enter));
        }
        keys.push(("tab", "Next Tab"));
        match tab {
            BgpTab::Neighbors => keys.push(("o", "Next Side")),
            BgpTab::Prefixes => {
                keys.push(("o", "Next Filter"));
                if matches!(
                    screen.items.get(self.bgp_row),
                    Some(Item::Prefix(_, Some(_)))
                ) {
                    keys.push(("v", "RPKI"));
                }
            }
            BgpTab::History => keys.push(("d", "Next Range")),
            _ => {}
        }
        keys.push(("c", "Copy"));
        if matches!(bgp.kind, TargetKind::Host | TargetKind::Address)
            && bgp.unrouted_reason.is_none()
        {
            keys.push(("t", "Run Traceroute"));
        }
        if !run.is_running() {
            keys.push(("R", "Run Again"));
        }
        keys.push(("r", "Run Tool"));
        if run.is_running() {
            keys.push(("s", "Stop"));
        } else {
            keys.push(("s", if self.is_saved() { "Unsave" } else { "Save" }));
            keys.push(("a", "Add to Case"));
            if self.can_share() {
                keys.push(("S", "Share"));
            }
        }
        keys.push(("\u{2190}", "Back"));
        keys
    }

    /// Keys of a BGP result: the tabs, the rows in them, and a row's details.
    pub(super) fn bgp_key(&mut self, key: KeyEvent) {
        let Some(run) = self.run() else { return };
        let running = run.is_running();
        let target = run.target.clone();
        let empty = BgpResult::new(TargetKind::Host, &run.target);
        let bgp = run.bgp.clone().unwrap_or(empty);
        let tabs = BgpTab::of(&bgp);
        let tab = self.bgp_shown_tab(&bgp);
        let screen = self.bgp_screen(run, 100);
        let rows = screen.items.len();
        self.bgp_row = self.bgp_row.min(rows.saturating_sub(1));
        let item = screen.items.get(self.bgp_row).cloned();
        let detail_networks = self
            .bgp_detail()
            .map(|detail| detail.networks())
            .unwrap_or_default();
        // The host a probing tool can take: the name that was looked up, or its address.
        let host = match bgp.kind {
            TargetKind::Host => Some(bgp.query.clone()),
            TargetKind::Address => bgp.address.clone().or(Some(bgp.query.clone())),
            _ => None,
        }
        .filter(|_| bgp.unrouted_reason.is_none());
        match key.code {
            KeyCode::Tab if self.details => {
                self.details_focus = !self.details_focus;
                self.action = 0;
            }
            KeyCode::Tab => {
                let at = tabs.iter().position(|known| *known == tab).unwrap_or(0);
                self.set_bgp_tab(tabs[(at + 1) % tabs.len()]);
            }
            KeyCode::Char(digit @ '1'..='9') => {
                if let Some(tab) = tabs.get(digit as usize - '1' as usize) {
                    self.set_bgp_tab(*tab);
                }
            }
            KeyCode::Esc | KeyCode::Left if self.details_focus => self.details_focus = false,
            KeyCode::Esc | KeyCode::Left if self.details => self.details = false,
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('/') => self.pane = Pane::Start,
            KeyCode::Down | KeyCode::Char('j') if self.details_focus => {
                self.action = (self.action + 1).min(detail_networks.len().saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') if self.details_focus => {
                self.action = self.action.saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Right if self.details_focus => {
                if let Some(asn) = detail_networks.get(self.action) {
                    self.start_with(Tool::Bgp, &format!("AS{asn}"));
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.bgp_row = (self.bgp_row + 1).min(rows.saturating_sub(1));
            }
            KeyCode::Up | KeyCode::Char('k') => self.bgp_row = self.bgp_row.saturating_sub(1),
            KeyCode::PageDown => {
                self.bgp_row = (self.bgp_row + self.page).min(rows.saturating_sub(1));
            }
            KeyCode::PageUp => self.bgp_row = self.bgp_row.saturating_sub(self.page),
            KeyCode::Home | KeyCode::Char('g') => self.bgp_row = 0,
            KeyCode::End | KeyCode::Char('G') => self.bgp_row = rows.saturating_sub(1),
            KeyCode::Char('v') if matches!(item, Some(Item::Prefix(_, Some(_)))) => {
                self.details = true;
            }
            KeyCode::Enter | KeyCode::Right => match item {
                Some(Item::Network(asn)) => {
                    if !(bgp.is_network() && bgp.query == format!("AS{asn}")) {
                        self.start_with(Tool::Bgp, &format!("AS{asn}"));
                    }
                }
                Some(Item::Prefix(prefix, _)) => {
                    if !(bgp.kind == TargetKind::Prefix && bgp.query == prefix) {
                        self.start_with(Tool::Bgp, &prefix);
                    }
                }
                Some(Item::Address(address)) => {
                    if bgp.address.as_deref() != Some(address.as_str()) {
                        self.start_with(Tool::Bgp, &address);
                    }
                }
                Some(Item::Validation(_) | Item::Path(_) | Item::GlassPath(..)) => {
                    self.details = true;
                }
                Some(Item::Tab(tab)) => self.set_bgp_tab(tab),
                Some(Item::OpenRun(run)) => {
                    self.retire();
                    self.shown = Some(Shown::Saved(run));
                    self.hop = 0;
                    self.scroll = 0;
                    self.details = false;
                    self.details_focus = false;
                }
                Some(Item::Start(tool, host)) => self.start_with(tool, &host),
                Some(Item::Whois(url)) => {
                    if open_in_browser(&url) {
                        self.toast("Opened RIPEstat in the browser");
                    } else {
                        self.copy(&url);
                    }
                }
                Some(Item::Text(_)) | None => {}
            },
            KeyCode::Char('o') if tab == BgpTab::Neighbors => {
                let at = Side::ALL
                    .iter()
                    .position(|side| *side == self.bgp_side)
                    .unwrap_or(0);
                self.bgp_side = Side::ALL[(at + 1) % Side::ALL.len()];
                self.bgp_row = 0;
            }
            KeyCode::Char('o') if tab == BgpTab::Prefixes => {
                let at = PrefixFilter::ALL
                    .iter()
                    .position(|filter| *filter == self.bgp_filter)
                    .unwrap_or(0);
                self.bgp_filter = PrefixFilter::ALL[(at + 1) % PrefixFilter::ALL.len()];
                self.bgp_row = 0;
                self.details = false;
                self.details_focus = false;
            }
            KeyCode::Char('d') if tab == BgpTab::History => {
                self.bgp_days = match self.bgp_days {
                    7 => 30,
                    30 => 90,
                    _ => 7,
                };
                self.bgp_row = 0;
            }
            KeyCode::Char('c') => {
                if let Some(text) = item.as_ref().and_then(Item::text) {
                    self.copy(&text);
                }
            }
            KeyCode::Char('t') => {
                if let Some(host) = host {
                    self.start_with(Tool::Traceroute, &host);
                }
            }
            KeyCode::Char('m') => {
                if let Some(host) = host {
                    self.start_with(Tool::Mtr, &host);
                }
            }
            KeyCode::Char('p') => {
                if let Some(host) = host {
                    self.start_with(Tool::Ping, &host);
                }
            }
            KeyCode::Char('f') => self.toggle_favorite(&target),
            KeyCode::Char('R') => self.start_with(Tool::Bgp, &target),
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
            KeyCode::Char('?') => self.help = true,
            _ => {}
        }
    }

    /// "<1> Overview  <2> Paths  …", the open one marked.
    fn bgp_tabs(&self, bgp: &BgpResult) -> Line<'static> {
        let open = self.bgp_shown_tab(bgp);
        let mut spans = vec![Span::raw(" ")];
        for (index, tab) in BgpTab::of(bgp).into_iter().enumerate() {
            spans.push(Span::styled(
                format!("<{}>", index + 1),
                Paint::new().fg(KEY).add_modifier(Modifier::BOLD),
            ));
            spans.push(if tab == open {
                Span::styled(
                    format!(" {} ", tab.title()),
                    selected().add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(format!(" {} ", tab.title()), dim())
            });
            spans.push(Span::raw(" "));
        }
        Line::from(spans)
    }

    /// Draws the tabs and the open one under them, scrolled so the selected row shows.
    pub(super) fn draw_bgp(&self, run: &Run, frame: &mut Frame, inner: Rect) {
        let empty = BgpResult::new(TargetKind::Host, &run.target);
        let bgp = run.bgp.as_ref().unwrap_or(&empty);
        let [tabs, body] =
            Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(inner);
        let screen = self.bgp_screen(run, usize::from(body.width).max(20));
        let offset = (screen.at + 3).saturating_sub(usize::from(body.height));
        frame.render_widget(Paragraph::new(self.bgp_tabs(bgp)), tabs);
        frame.render_widget(
            Paragraph::new(screen.lines).scroll((offset as u16, 0)),
            body,
        );
    }

    /// One network with a bar for its share.
    fn share_spans(
        bgp: &BgpResult,
        own: Option<u32>,
        asn: u32,
        fraction: f64,
        figure: Option<String>,
        columns: usize,
    ) -> Vec<Span<'static>> {
        const BAR: usize = 14;
        let mut name = bgp.name_of(asn).unwrap_or("").to_string();
        if Some(asn) == own {
            name = if name.is_empty() {
                "Your network".to_string()
            } else {
                format!("{name} \u{00B7} Your network")
            };
        }
        let room = columns.saturating_sub(1 + 13 + BAR + 10).clamp(10, 40);
        vec![
            Span::styled(format!(" {:<13}", format!("AS{asn}")), bold()),
            Span::raw(fit(&name, room)),
            Span::raw(format!(" {}", fit(&cli::share_bar(fraction, BAR), BAR))),
            Span::styled(fit_right(&figure.unwrap_or_default(), 8), dim()),
        ]
    }

    /// Builds the open tab: its lines and what each selectable row stands for.
    fn bgp_screen(&self, run: &Run, columns: usize) -> Screen {
        let empty = BgpResult::new(TargetKind::Host, &run.target);
        let bgp = run.bgp.as_ref().unwrap_or(&empty);
        let running = run.is_running();
        let own = run.own_asn();
        let now = match &self.shown {
            // A saved result is read against the time it was made.
            Some(Shown::Saved(saved)) => saved.started_at,
            _ => format::now(),
        };
        let mut screen = Screen {
            lines: Vec::new(),
            items: Vec::new(),
            at: 0,
            row: self.bgp_row,
            columns,
        };
        if bgp.is_empty() {
            screen.lines.push(Line::styled(
                if running {
                    " Finding the route\u{2026}".to_string()
                } else {
                    format!(" {}", run.summary())
                },
                dim(),
            ));
            return screen;
        }
        match self.bgp_shown_tab(bgp) {
            BgpTab::Overview => self.bgp_overview(&mut screen, run, bgp, own, now),
            BgpTab::Paths => Self::bgp_paths(&mut screen, bgp, own, running),
            BgpTab::Neighbors => self.bgp_neighbors(&mut screen, bgp, own, running),
            BgpTab::Prefixes => self.bgp_prefixes(&mut screen, bgp, running),
            BgpTab::History => self.bgp_history(&mut screen, bgp, running, now),
            BgpTab::Sources => Self::bgp_sources(&mut screen, bgp),
        }
        screen
    }

    fn bgp_verdict(screen: &mut Screen, bgp: &BgpResult, running: bool) {
        let verdict = bgp.verdict();
        if !running || verdict.title != "Looking up" {
            let paint = match grade_color(verdict.grade) {
                Some(color) => bold().fg(color),
                None => bold(),
            };
            screen
                .lines
                .push(Line::styled(format!(" {}", verdict.title), paint));
            screen.sentence(&verdict.detail, Paint::new());
        }
    }

    fn bgp_overview(
        &self,
        screen: &mut Screen,
        run: &Run,
        bgp: &BgpResult,
        own: Option<u32>,
        now: u64,
    ) {
        let running = run.is_running();
        let placeholder = if running { "\u{2026}" } else { "\u{2014}" };
        Self::bgp_verdict(screen, bgp, running);
        let tile = |label: &str, value: String, paint: Paint| {
            vec![
                Span::styled(format!(" {label} "), dim()),
                Span::styled(value, paint),
                Span::raw("   "),
            ]
        };

        if bgp.is_network() {
            let Some(network) = &bgp.network else {
                return;
            };
            let prefixes = |number: Option<u64>| {
                number.map_or(placeholder.to_string(), |number| {
                    counted(number, "prefix", "prefixes")
                })
            };
            let mut tiles = tile("IPv4", prefixes(network.v4_count), bold());
            tiles.extend(tile("IPv6", prefixes(network.v6_count), bold()));
            tiles.extend(tile(
                "ROA",
                network
                    .coverage()
                    .map_or(placeholder.to_string(), |(valid, checked)| {
                        format!("{valid}/{checked} checked")
                    }),
                if network.invalid_count() > 0 {
                    bold().fg(Color::Red)
                } else {
                    bold()
                },
            ));
            screen.blank();
            screen.lines.push(Line::from(tiles));

            screen.header("REGISTRATION");
            let holder = network.holder.clone().unwrap_or_else(|| {
                if running {
                    "\u{2026}".to_string()
                } else {
                    "Not listed".to_string()
                }
            });
            screen.fact(
                "Holder",
                Span::styled(holder.clone(), bold()),
                Item::Text(holder),
            );
            if let Some(registry) = &network.registry {
                screen.fact(
                    "Registry",
                    Span::styled(registry.clone(), bold()),
                    Item::Text(registry.clone()),
                );
            }
            if let Some(country) = &network.country {
                screen.fact(
                    "Country",
                    Span::styled(country.clone(), bold()),
                    Item::Text(country.clone()),
                );
            }
            if let Some(announced) = network.announced {
                let word = if announced { "Seen" } else { "Not seen" };
                screen.fact(
                    "In public routing",
                    Span::styled(word, bold()),
                    Item::Text(word.to_string()),
                );
            }
            let url = format!("https://stat.ripe.net/AS{}", network.asn);
            screen.fact("Whois", Span::styled(url.clone(), dim()), Item::Whois(url));

            let upstreams = network.side(Side::Left);
            if !upstreams.is_empty() {
                screen.header("SEEN UPSTREAM");
                let top = upstreams[0].power.max(1);
                for neighbor in upstreams.iter().take(4) {
                    screen.item(
                        Self::share_spans(
                            bgp,
                            own,
                            neighbor.asn,
                            neighbor.power as f64 / top as f64,
                            None,
                            screen.columns,
                        ),
                        Item::Network(neighbor.asn),
                    );
                }
            }
        } else {
            if bgp.prefix.is_some() && bgp.unrouted_reason.is_none() {
                let seen = match (bgp.seeing, bgp.total_peers) {
                    (Some(seeing), Some(total)) => format!("{seeing}/{total} peers"),
                    _ => placeholder.to_string(),
                };
                let mut tiles = tile(
                    "Seen by",
                    seen,
                    if bgp.visibility().is_some_and(|share| share < 0.8) {
                        bold().fg(Color::Yellow)
                    } else {
                        bold()
                    },
                );
                tiles.extend(tile(
                    "Paths",
                    if bgp.path_count > 0 {
                        bgp.path_count.to_string()
                    } else {
                        placeholder.to_string()
                    },
                    bold(),
                ));
                tiles.extend(tile(
                    "First seen",
                    bgp.first_seen
                        .map_or(placeholder.to_string(), |seen| date(seen)[..4].to_string()),
                    bold(),
                ));
                screen.blank();
                screen.lines.push(Line::from(tiles));
            }

            if let Some(prefix) = &bgp.prefix {
                screen.header("ROUTE");
                if bgp.kind == TargetKind::Prefix {
                    screen.fact(
                        "Prefix",
                        Span::styled(prefix.clone(), bold()),
                        Item::Text(prefix.clone()),
                    );
                } else {
                    if let Some(address) = &bgp.address {
                        screen.fact(
                            "Address",
                            Span::styled(address.clone(), bold()),
                            Item::Text(address.clone()),
                        );
                    }
                    screen.fact(
                        "Announced prefix",
                        Span::styled(prefix.clone(), bold()),
                        Item::Prefix(prefix.clone(), None),
                    );
                }
                for origin in &bgp.origins {
                    let label = truncate_end(origin.holder.as_deref().unwrap_or("Origin AS"), 18);
                    screen.fact(
                        &label,
                        Span::styled(format!("AS{}", origin.asn), bold()),
                        Item::Network(origin.asn),
                    );
                    let label = if bgp.origins.len() > 1 {
                        format!("RPKI for AS{}", origin.asn)
                    } else {
                        "RPKI".to_string()
                    };
                    match bgp.validation_for(origin.asn) {
                        Some(validation) => screen.fact(
                            &label,
                            badge(validation.status),
                            Item::Validation(validation.clone()),
                        ),
                        None if running => {
                            screen.fact(
                                &label,
                                Span::styled("\u{2026}", dim()),
                                Item::Text(String::new()),
                            );
                        }
                        None => {}
                    }
                }
                if let Some(origin) = bgp.origin() {
                    let objects = if origin.route_objects.is_empty() {
                        "None on record".to_string()
                    } else {
                        origin
                            .route_objects
                            .iter()
                            .map(|registry| registry.to_uppercase())
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    screen.fact(
                        "Route object",
                        Span::styled(objects.clone(), bold()),
                        Item::Text(objects),
                    );
                }
                if let Some(registry) = &bgp.registry {
                    screen.fact(
                        "Registry",
                        Span::styled(registry.clone(), bold()),
                        Item::Text(registry.clone()),
                    );
                }
                for glass in &bgp.glasses {
                    screen.fact(
                        &truncate_end(&glass.name, 18),
                        Span::styled(glass.summary(), bold()),
                        Item::Tab(BgpTab::Paths),
                    );
                }
            }

            if bgp.addresses.len() > 1 {
                screen.header("ADDRESSES");
                let widest = bgp
                    .addresses
                    .iter()
                    .map(|entry| entry.address.len())
                    .max()
                    .unwrap_or(0);
                for entry in &bgp.addresses {
                    let mut route: Vec<String> = entry.prefix.iter().cloned().collect();
                    route.extend(entry.asn.map(|asn| format!("AS{asn}")));
                    let route = if route.is_empty() {
                        "No route observed".to_string()
                    } else {
                        route.join(" \u{00B7} ")
                    };
                    let mut spans = vec![
                        Span::styled(format!(" {:<widest$}  ", entry.address), bold()),
                        Span::styled(route, dim()),
                    ];
                    if Some(&entry.address) == bgp.address.as_ref() {
                        spans.push(Span::styled("  Shown", dim()));
                    }
                    screen.item(spans, Item::Address(entry.address.clone()));
                }
            }

            if !bgp.upstreams.is_empty() {
                screen.header("REACHED THROUGH");
                let top = bgp.upstreams[0].peers.max(1);
                for share in bgp.upstreams.iter().take(4) {
                    screen.item(
                        Self::share_spans(
                            bgp,
                            own,
                            share.asn,
                            share.peers as f64 / top as f64,
                            Some(format::percent(bgp.share(share.peers))),
                            screen.columns,
                        ),
                        Item::Network(share.asn),
                    );
                }
            }

            if bgp.origin().is_some() && matches!(bgp.kind, TargetKind::Host | TargetKind::Address)
            {
                screen.header("YOUR PATH");
                let trace = cli::last_trace(&self.history, run);
                match trace.and_then(|trace| Some((trace, TraceComparison::new(trace, bgp)?))) {
                    Some((trace, comparison)) => {
                        let mut spans = vec![Span::raw(" ")];
                        spans.extend(chain(&comparison.trace_asns));
                        screen.item(spans, Item::OpenRun(Box::new(trace.clone())));
                        screen.note(&comparison.sentence);
                        screen.note(&format!(
                            "{} \u{00B7} {}",
                            trace.tool.title(),
                            format::relative(trace.started_at, now)
                        ));
                    }
                    None => {
                        let host = match bgp.kind {
                            TargetKind::Host => bgp.query.clone(),
                            _ => bgp.address.clone().unwrap_or_else(|| bgp.query.clone()),
                        };
                        screen.item(
                            vec![
                                Span::styled(" Trace the Path  ", bold()),
                                Span::styled(format!("traceroute to {host}"), dim()),
                            ],
                            Item::Start(Tool::Traceroute, host),
                        );
                    }
                }
            }

            if let Some(network) = &bgp.network {
                screen.header("NETWORK");
                screen.item(
                    vec![Span::styled(format!(" About AS{}", network.asn), bold())],
                    Item::Network(network.asn),
                );
                screen.note(&network.line());
            }

            if bgp.prefix.is_some() && (!bgp.spans.is_empty() || !bgp.activity.is_empty()) {
                screen.header("RECENT CHANGES");
                let changes = bgp.changes(7, now);
                let line = if changes.is_empty() {
                    Span::raw(" No origin AS changes in 7 days")
                } else {
                    Span::styled(
                        format!(
                            " {} {} in 7 days",
                            changes.len(),
                            if changes.len() == 1 {
                                "change"
                            } else {
                                "changes"
                            }
                        ),
                        Paint::new().fg(Color::Yellow),
                    )
                };
                screen.item(vec![line], Item::Tab(BgpTab::History));
                if !bgp.activity.is_empty() {
                    screen.note(&format!(
                        "{} BGP updates seen by collectors in that time.",
                        count(bgp.updates())
                    ));
                }
            }

            if !bgp.related.is_empty() {
                screen.header("RELATED PREFIXES");
                let own_origin = bgp.origin().map(|origin| origin.asn);
                let shown: Vec<_> = bgp.related.iter().take(8).collect();
                let widest = shown
                    .iter()
                    .map(|related| related.prefix.len())
                    .max()
                    .unwrap_or(0);
                let mut takes_traffic = false;
                for related in shown {
                    let mut facts = vec![if related.is_more_specific {
                        "More specific".to_string()
                    } else {
                        "Covering".to_string()
                    }];
                    facts.extend(related.origin.map(|asn| format!("AS{asn}")));
                    let other = related.origin.is_some() && related.origin != own_origin;
                    takes_traffic |= other && related.is_more_specific;
                    let mut spans = vec![
                        Span::styled(format!(" {:<widest$}  ", related.prefix), bold()),
                        Span::styled(facts.join(" \u{00B7} "), dim()),
                    ];
                    if other && own_origin.is_some() {
                        spans.push(Span::styled(
                            "  Other origin AS",
                            Paint::new().fg(Color::Yellow),
                        ));
                    }
                    screen.item(spans, Item::Prefix(related.prefix.clone(), None));
                }
                if takes_traffic {
                    screen.note("A more specific prefix from another origin AS takes the traffic for its part of the range.");
                }
            }
        }

        if !bgp.sources.is_empty() {
            screen.header("SOURCES");
            let mut line = ansi_line(&format!(
                " {}",
                cli::sources_line(bgp, Style { color: true }, now)
            ));
            line.spans.insert(0, Span::raw(""));
            screen.item(line.spans, Item::Tab(BgpTab::Sources));
        }
    }

    fn path_item(screen: &mut Screen, bgp: &BgpResult, path: &Path) {
        let mut spans = vec![Span::styled(
            format!(" {:>6}  ", format::percent(bgp.share(path.peers))),
            dim(),
        )];
        spans.extend(chain(&path.asns));
        screen.item(spans, Item::Path(path.clone()));
        let mut facts = bgp.path_line(path);
        if path.prepended {
            facts.push_str(" \u{00B7} prepending");
        }
        screen
            .lines
            .push(Line::styled(format!("         {facts}"), dim()));
    }

    fn bgp_paths(screen: &mut Screen, bgp: &BgpResult, own: Option<u32>, running: bool) {
        if bgp.paths.is_empty() {
            screen.lines.push(Line::styled(
                if running {
                    " Getting collector paths\u{2026}"
                } else {
                    " No paths on record"
                },
                bold(),
            ));
            if !running {
                screen.note("No collector peer reported a route for this prefix.");
            }
        } else {
            screen.lines.push(Line::from(vec![
                Span::styled(" Shortest ", dim()),
                Span::styled(
                    counted(bgp.shortest_path as u64, "network", "networks"),
                    bold(),
                ),
                Span::styled("   Typical ", dim()),
                Span::styled(
                    counted(bgp.typical_path as u64, "network", "networks"),
                    bold(),
                ),
                Span::styled("   Direct ", dim()),
                Span::styled(
                    format!("{} of peers", format::percent(bgp.share(bgp.direct_peers))),
                    bold(),
                ),
            ]));
            if let Some(summary) = bgp.path_summary() {
                screen.note(&summary);
            }
            let from_own: Vec<&Path> = bgp
                .paths
                .iter()
                .filter(|path| own.is_some() && path.asns.first() == own.as_ref())
                .collect();
            if let (Some(own), false) = (own, from_own.is_empty()) {
                screen.header("FROM YOUR NETWORK");
                for path in &from_own {
                    Self::path_item(screen, bgp, path);
                }
                screen.note(&if from_own[0].asns.len() == 1 {
                    format!("This device is in AS{own}, which announces the prefix itself.")
                } else {
                    format!("Reported by a collector peer inside AS{own}, the network this device is in. Your packets likely leave the same way.")
                });
            }
            screen.header("OBSERVED AS PATHS");
            for path in &bgp.paths {
                Self::path_item(screen, bgp, path);
            }
            screen.note(&format!(
                "{} distinct {} from {} peers at {} collectors.",
                bgp.path_count,
                if bgp.path_count == 1 { "path" } else { "paths" },
                bgp.path_peers,
                bgp.collector_count
            ));
        }
        for (index, glass) in bgp.glasses.iter().enumerate() {
            screen.blank();
            screen.lines.push(Line::from(vec![
                Span::styled(format!(" {}", glass.name.to_uppercase()), bold()),
                Span::styled(
                    format!(
                        "  {} \u{00B7} {} \u{00B7} {}",
                        glass.kind.title(),
                        glass.host,
                        glass.summary()
                    ),
                    dim(),
                ),
            ]));
            if glass.failed {
                screen.note(&format!("{} didn't answer.", glass.host));
            } else if glass.routes.is_empty() {
                screen.note(&no_route_line(
                    glass,
                    bgp.prefix.as_deref().unwrap_or("this prefix"),
                ));
            } else {
                for path in glass.paths() {
                    let mut spans = vec![Span::raw(" ")];
                    spans.extend(chain(&path.asns));
                    if let Some(state) = cli::glass_path_state(glass, &path) {
                        spans.push(Span::styled(
                            format!("  {state}"),
                            if path.is_filtered {
                                Paint::new().fg(Color::Yellow)
                            } else {
                                dim()
                            },
                        ));
                    }
                    let facts = cli::glass_path_line(glass, &path);
                    screen.item(spans, Item::GlassPath(index, path));
                    screen
                        .lines
                        .push(Line::styled(format!("   {facts}"), dim()));
                }
            }
        }
    }

    fn bgp_neighbors(&self, screen: &mut Screen, bgp: &BgpResult, own: Option<u32>, running: bool) {
        let Some(network) = bgp
            .network
            .as_ref()
            .filter(|network| !network.neighbors.is_empty())
        else {
            screen.lines.push(Line::styled(
                if running {
                    " Getting neighbors\u{2026}"
                } else {
                    " No neighbors on record"
                },
                bold(),
            ));
            if !running {
                screen.note("Collectors report no network next to this one in an AS path.");
            }
            return;
        };
        screen.lines.push(chips(
            Side::ALL
                .into_iter()
                .map(|side| {
                    let total = network
                        .side_count(side)
                        .map_or(String::new(), |total| format!(" {total}"));
                    (format!("{}{total}", side.title()), side == self.bgp_side)
                })
                .collect(),
        ));
        screen.blank();
        let shown = network.side(self.bgp_side);
        let top = shown.first().map_or(1, |neighbor| neighbor.power.max(1));
        for neighbor in &shown {
            screen.item(
                Self::share_spans(
                    bgp,
                    own,
                    neighbor.asn,
                    neighbor.power as f64 / top as f64,
                    None,
                    screen.columns,
                ),
                Item::Network(neighbor.asn),
            );
        }
        screen.blank();
        screen.note(&network.side_note(self.bgp_side, shown.len()));
    }

    fn bgp_prefixes(&self, screen: &mut Screen, bgp: &BgpResult, running: bool) {
        let Some(network) = bgp
            .network
            .as_ref()
            .filter(|network| !network.prefixes.is_empty())
        else {
            screen.lines.push(Line::styled(
                if running {
                    " Getting prefixes\u{2026}"
                } else {
                    " No prefixes on record"
                },
                bold(),
            ));
            if !running {
                screen.note("Collectors see no prefix originated by this network.");
            }
            return;
        };
        screen.lines.push(chips(
            PrefixFilter::ALL
                .into_iter()
                .map(|filter| (filter.title().to_string(), filter == self.bgp_filter))
                .collect(),
        ));
        screen.header(&format!("ORIGINATED BY AS{}", network.asn));
        let widest = network.prefixes.iter().map(String::len).max().unwrap_or(0);
        let mut listed = 0;
        for prefix in &network.prefixes {
            let validation = network.validation_for(prefix);
            if !self
                .bgp_filter
                .takes(prefix, validation.map(|validation| validation.status))
            {
                continue;
            }
            listed += 1;
            let mut spans = vec![Span::styled(format!(" {prefix:<widest$}  "), bold())];
            match validation {
                Some(validation) => {
                    let badge = badge(validation.status);
                    let pad = 10usize.saturating_sub(badge.content.len());
                    spans.push(badge);
                    spans.push(Span::raw(" ".repeat(pad)));
                }
                None => spans.push(Span::raw(" ".repeat(10))),
            }
            if Some(prefix) == bgp.prefix.as_ref() {
                spans.push(Span::styled("This route", dim()));
            }
            screen.item(spans, Item::Prefix(prefix.clone(), validation.cloned()));
        }
        if listed == 0 {
            screen
                .lines
                .push(Line::styled(" No prefix matches.", dim()));
        }
        let footer = cli::prefix_footer(network, running);
        if !footer.is_empty() {
            screen.blank();
            screen.note(&footer);
        }
    }

    fn bgp_history(&self, screen: &mut Screen, bgp: &BgpResult, running: bool, now: u64) {
        let days = self.bgp_days;
        let since = now.saturating_sub(days * 86_400);
        let cells = screen.columns.saturating_sub(3).clamp(20, 90);
        screen.lines.push(chips(
            [7u64, 30, 90]
                .into_iter()
                .map(|range| (format!("{range} d"), range == days))
                .collect(),
        ));
        let axis = |screen: &mut Screen| {
            let (from, to) = (date(since), date(now));
            screen.lines.push(Line::styled(
                format!(
                    " {from}{}{to}",
                    " ".repeat(cells.saturating_sub(from.len() + to.len()))
                ),
                dim(),
            ));
        };
        let lanes = bgp.lanes(since);
        if lanes.is_empty() {
            screen.blank();
            screen.lines.push(Line::styled(
                if running {
                    " Getting the routing history\u{2026}"
                } else {
                    " No announcement on record"
                },
                bold(),
            ));
            if !running {
                screen.note(&format!("No origin AS was seen for this prefix in the last {days} days by at least ten full-table peers."));
            }
        } else {
            screen.header("ANNOUNCED BY");
            for (index, (origin, stretches)) in lanes.iter().enumerate() {
                let label = match bgp.name_of(*origin) {
                    Some(name) => format!("AS{origin} \u{00B7} {name}"),
                    None => format!("AS{origin}"),
                };
                screen.item(
                    vec![Span::styled(
                        format!(" {}", truncate_end(&label, cells)),
                        dim(),
                    )],
                    Item::Network(*origin),
                );
                screen.lines.push(Line::styled(
                    format!(" {}", cli::lane(stretches, since, now, cells)),
                    Paint::new().fg(if index == 0 {
                        Color::Green
                    } else {
                        Color::Yellow
                    }),
                ));
            }
            axis(screen);

            let peers = bgp.peers_over(since, now, cells);
            let top = peers.iter().copied().fold(0.0, f64::max);
            screen.blank();
            screen.lines.push(Line::from(vec![
                Span::styled(" PEERS SEEING IT", bold()),
                Span::styled(format!("  up to {}", top as u64), dim()),
            ]));
            screen.lines.push(Line::styled(
                format!(" {}", cli::spark(&peers)),
                Paint::new().fg(Color::Blue),
            ));
            axis(screen);
        }

        screen.header("EVENTS");
        let changes = bgp.changes(days, now);
        if changes.is_empty() {
            screen.lines.push(Line::raw(" No changes"));
            screen.note(&format!(
                "Same origin AS and steady visibility over the last {days} days."
            ));
        } else {
            for change in &changes {
                let (mark, color) = if change.is_good() {
                    ("+", Color::Green)
                } else {
                    ("\u{2212}", Color::Yellow)
                };
                screen.item(
                    vec![
                        Span::styled(format!(" {mark} "), bold().fg(color)),
                        Span::styled(change.title(), bold()),
                        Span::styled(format!("  {}", date_time(change.date)), dim()),
                    ],
                    Item::Network(change.origin),
                );
                screen.lines.push(Line::styled(
                    format!("   {}", bgp.change_line(change)),
                    dim(),
                ));
            }
        }
        screen.note("Read from 8-hour collector snapshots, so times are approximate.");

        if !bgp.activity.is_empty() {
            let series = |pick: fn(&rvrs_core::bgp::Activity) -> u64| -> Vec<f64> {
                bgp.activity
                    .iter()
                    .map(|bucket| pick(bucket) as f64)
                    .collect()
            };
            let announcements = series(|bucket| bucket.announcements);
            let withdrawals = series(|bucket| bucket.withdrawals);
            // Both rows share one scale, so a withdrawal reads against the announcements.
            let top = announcements
                .iter()
                .chain(&withdrawals)
                .copied()
                .fold(0.0, f64::max);
            let scaled = |values: &[f64]| {
                let mut with_top = values.to_vec();
                with_top.push(top);
                let mut drawn = cli::spark(&with_top);
                drawn.pop();
                drawn
            };
            screen.blank();
            screen.lines.push(Line::from(vec![
                Span::styled(" BGP UPDATES, 7 DAYS", bold()),
                Span::styled(format!("  {} total", count(bgp.updates())), dim()),
            ]));
            screen.lines.push(Line::from(vec![
                Span::styled(" Announcements  ", dim()),
                Span::styled(scaled(&announcements), Paint::new().fg(Color::Blue)),
            ]));
            screen.lines.push(Line::from(vec![
                Span::styled(" Withdrawals    ", dim()),
                Span::styled(scaled(&withdrawals), Paint::new().fg(Color::Red)),
            ]));
        }
    }

    fn bgp_sources(screen: &mut Screen, bgp: &BgpResult) {
        let mut sources: Vec<_> = bgp.sources.iter().collect();
        sources.sort_by(|a, b| a.title.cmp(&b.title));
        for source in sources {
            let mut spans = vec![Span::styled(format!(" {}", source.title), bold())];
            if source.failed {
                spans.push(Span::styled(
                    "  Unavailable",
                    Paint::new().fg(Color::Yellow),
                ));
            }
            screen.item(spans, Item::Text(source.line()));
            screen
                .lines
                .push(Line::styled(format!("   {}", source.basis), dim()));
            screen.lines.push(Line::styled(
                format!(
                    "   {}",
                    truncate_end(&source.line(), screen.columns.saturating_sub(4))
                ),
                dim(),
            ));
        }
        screen.blank();
        screen
            .note("Each part was observed at its own time, so together they are not one snapshot.");
    }

    /// The selected row's details: an AS path network by network, or an RPKI result spelled out.
    pub(super) fn draw_bgp_details(&self, frame: &mut Frame, area: Rect) {
        let (Some(run), Some(bgp), Some(detail)) = (self.run(), self.bgp(), self.bgp_detail())
        else {
            return;
        };
        let own = run.own_asn();
        let room = usize::from(area.width).saturating_sub(4).max(16);
        let fact = |label: &str, value: String| {
            Line::from(vec![
                Span::styled(
                    format!("  {:<14}", format!("{label}:")),
                    Paint::new().fg(LABEL),
                ),
                Span::styled(value, bold()),
            ])
        };
        let network_row =
            |index: usize, asn: u32, role: String, last: bool| -> Vec<Line<'static>> {
                let mark = if last { "\u{25CF}" } else { "\u{25CB}" };
                let name = truncate_end(bgp.name_of(asn).unwrap_or(""), room.saturating_sub(16));
                let text = format!("  {mark} {:<12} {name}", format!("AS{asn}"));
                let first = if self.details_focus && index == self.action {
                    Line::styled(fit(&text, room + 2), selected())
                } else if last {
                    Line::styled(text, bold())
                } else {
                    Line::raw(text)
                };
                let role = if Some(asn) == own {
                    format!("{role} \u{00B7} your network")
                } else {
                    role
                };
                vec![first, Line::styled(format!("    {role}"), dim())]
            };
        let mut lines: Vec<Line> = vec![Line::default()];
        let (title, scope, counted) = match &detail {
            Detail::Path(path) => {
                let last = path.asns.len().saturating_sub(1);
                for (index, asn) in path.asns.iter().enumerate() {
                    let role = if index == last {
                        "Origin AS"
                    } else if index == 0 {
                        "Collector peer"
                    } else {
                        "Transit"
                    };
                    lines.extend(network_row(index, *asn, role.to_string(), index == last));
                }
                lines.push(Line::default());
                lines.push(Line::styled("  OBSERVED", bold()));
                lines.push(fact("Networks", path.asns.len().to_string()));
                lines.push(fact(
                    "Seen by",
                    format!("{} of {} peers", path.peers, bgp.path_peers),
                ));
                if let Some(changed) = path.last_changed {
                    lines.push(fact("Last changed", date_time(changed)));
                }
                if path.prepended {
                    lines.push(fact("Prepending", "Yes".to_string()));
                }
                lines.push(Line::default());
                lines.push(Line::styled("  COLLECTORS", bold()));
                for collector in &path.collectors {
                    lines.push(Line::raw(format!("  {}", truncate_end(collector, room))));
                }
                if !path.communities.is_empty() {
                    lines.push(Line::default());
                    lines.push(Line::styled("  COMMUNITIES", bold()));
                    for line in pretty::wrap(&path.communities.join("  "), room) {
                        lines.push(Line::styled(format!("  {line}"), dim()));
                    }
                    lines.push(Line::styled(
                        "  Read from one collector peer's route.",
                        dim(),
                    ));
                }
                (
                    "AS Path",
                    format!("{} networks", path.asns.len()),
                    format::percent(bgp.share(path.peers)),
                )
            }
            Detail::GlassPath(index, path) => {
                let glass = bgp.glasses.get(*index);
                let routers = glass.is_some_and(|glass| glass.kind.has_routers());
                let last = path.asns.len().saturating_sub(1);
                for (index, asn) in path.asns.iter().enumerate() {
                    let role = if index == last {
                        "Origin AS"
                    } else if index == 0 && routers {
                        "Looking glass network"
                    } else if index == 0 {
                        "Route server peer"
                    } else {
                        "Transit"
                    };
                    lines.extend(network_row(index, *asn, role.to_string(), index == last));
                }
                lines.push(Line::default());
                lines.push(Line::styled("  OBSERVED", bold()));
                lines.push(fact("Networks", path.asns.len().to_string()));
                if let Some(glass) = glass {
                    lines.push(fact(
                        "Looking glass",
                        truncate_end(&glass.name, room.saturating_sub(16)),
                    ));
                    lines.push(fact("Type", glass.kind.title().to_string()));
                    lines.push(fact(
                        "Host",
                        truncate_end(&glass.host, room.saturating_sub(16)),
                    ));
                    if let Some(observed) = glass.observed_at {
                        lines.push(fact("Data from", date_time(observed)));
                    }
                    if let Some(state) = cli::glass_path_state(glass, path) {
                        lines.push(fact("State", state.to_string()));
                    }
                }
                lines.push(Line::default());
                lines.push(Line::styled(
                    if routers {
                        "  ROUTERS"
                    } else {
                        "  ROUTE SERVERS"
                    },
                    bold(),
                ));
                for server in &path.route_servers {
                    lines.push(Line::raw(format!("  {}", truncate_end(server, room))));
                }
                if !path.communities.is_empty() {
                    lines.push(Line::default());
                    lines.push(Line::styled("  COMMUNITIES", bold()));
                    for line in pretty::wrap(&path.communities.join("  "), room) {
                        lines.push(Line::styled(format!("  {line}"), dim()));
                    }
                }
                (
                    "AS Path",
                    format!("{} networks", path.asns.len()),
                    format!(
                        "{} {}",
                        path.route_servers.len(),
                        if routers { "routers" } else { "route servers" }
                    ),
                )
            }
            Detail::Validation(validation) => {
                let paint = match grade_color(validation.status.grade()) {
                    Some(color) => bold().fg(color),
                    None => bold(),
                };
                lines.push(Line::styled(
                    format!("  {}", validation.status.title()),
                    paint,
                ));
                for line in pretty::wrap(&validation.explanation(), room) {
                    lines.push(Line::styled(format!("  {line}"), dim()));
                }
                lines.push(Line::default());
                lines.push(Line::styled("  ROUTE", bold()));
                lines.push(fact("Prefix", validation.prefix.clone()));
                lines.extend(network_row(
                    0,
                    validation.origin,
                    "Origin AS".to_string(),
                    true,
                ));
                let checks = validation.checks();
                if !checks.is_empty() {
                    lines.push(Line::default());
                    lines.push(Line::styled("  VALIDATION", bold()));
                    for check in checks {
                        let (mark, color) = if check.passed {
                            ("\u{2713}", Color::Green)
                        } else {
                            ("\u{00D7}", Color::Red)
                        };
                        lines.push(Line::from(vec![
                            Span::styled(format!("  {mark} "), bold().fg(color)),
                            Span::raw(check.title),
                        ]));
                        for line in pretty::wrap(&check.detail, room.saturating_sub(4)) {
                            lines.push(Line::styled(format!("    {line}"), dim()));
                        }
                    }
                }
                if !validation.roas.is_empty() {
                    lines.push(Line::default());
                    lines.push(Line::styled("  COVERING ROAS", bold()));
                    for roa in &validation.roas {
                        lines.push(Line::from(vec![
                            Span::styled(format!("  {}  ", roa.prefix), bold()),
                            Span::styled(
                                format!("AS{} \u{00B7} max /{}  ", roa.origin, roa.max_length),
                                dim(),
                            ),
                            badge(RpkiStatus::from_provider(&roa.validity)),
                        ]));
                    }
                    for line in pretty::wrap(
                        &format!(
                            "Validated by Routinator through RIPEstat {}.",
                            format::relative(validation.checked_at, format::now()).to_lowercase()
                        ),
                        room,
                    ) {
                        lines.push(Line::styled(format!("  {line}"), dim()));
                    }
                }
                (
                    "RPKI",
                    format!("AS{}", validation.origin),
                    validation.status.badge().to_string(),
                )
            }
        };
        let block = panel_with(title, &scope, &counted, self.details_focus);
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }
}
