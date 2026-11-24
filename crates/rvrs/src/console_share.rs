//! The share sheet: `S` on a run that has ended. It opens over the screen and reads, on a
//! thread, the run's live link and the workspace's rules for links. Then it says what the link
//! does and has the switches for what the page shows, starting where the live link has them or
//! at the workspace's defaults, and makes nothing until Return (the user's rule for every
//! surface, 2026-11-23: the sheet first, then the link). The link is made on a thread, so the
//! Console keeps drawing, then shown with Open Link, Copy Link and Stop Sharing, and Return
//! changes what it shows when a switch moved. As on the web, a member of a workspace where only
//! owners and admins share reads why there is no link, and can still stop one they made before.

use std::sync::mpsc::{self, Receiver};

use crate::share::{self, Rules, Share, Shows, Standing};

use super::*;

/// The switches, in the order the web's Share shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Choice {
    PrivateNetworks,
    PublicAddress,
    InstallName,
}

impl Choice {
    fn title(self) -> &'static str {
        match self {
            Choice::PrivateNetworks => "Private Networks",
            Choice::PublicAddress => "Public Address",
            Choice::InstallName => "Device Name",
        }
    }

    fn on(self, shows: Shows) -> bool {
        match self {
            Choice::PrivateNetworks => shows.private_networks,
            Choice::PublicAddress => shows.public_address,
            Choice::InstallName => shows.install_name,
        }
    }

    fn flip(self, shows: &mut Shows) {
        match self {
            Choice::PrivateNetworks => shows.private_networks = !shows.private_networks,
            Choice::PublicAddress => shows.public_address = !shows.public_address,
            Choice::InstallName => shows.install_name = !shows.install_name,
        }
    }
}

/// Where the sheet stands.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Stage {
    /// Asking the API for the run's live link and the workspace's rules.
    Reading,
    /// The switches, with the live link when the run has one. Nothing is made until Return.
    Choosing,
    /// Asking the API for the link, or to change what it shows.
    Sharing,
    /// Asked whether to end the link.
    Stopping,
    /// Asking the API to end the link.
    Ending,
    Failed(String),
}

/// What the thread doing the work tells the Console.
enum Message {
    Read(Standing),
    Shared(Share),
    Ended,
    Failed(String),
}

/// What the Console does once the thread is done.
pub(super) enum Done {
    /// A new link, to copy, or a link that shows something else from now on.
    Shared {
        url: String,
        new: bool,
    },
    Ended,
}

pub(super) struct Sheet {
    pub(super) run: Run,
    shows: Shows,
    /// The switches this run has something for, each with what it covers in the run.
    choices: Vec<(Choice, String)>,
    row: usize,
    /// The workspace's rules, once the API has answered.
    rules: Option<Rules>,
    /// The run's live link.
    link: Option<Share>,
    pub(super) stage: Stage,
    messages: Option<Receiver<Message>>,
}

/// The first address of the private network the run started in: its source address when that
/// is private, otherwise the first hop at a private address before any other answered.
fn private_network(run: &Run) -> Option<String> {
    if let Some(source) = run.source.filter(|address| is_private(*address)) {
        return Some(source.to_string());
    }
    let first = run.path().iter().find_map(|hop| hop.address())?;
    is_private(first).then(|| first.to_string())
}

impl Sheet {
    /// The sheet for a run, reading where its share stands at once.
    fn new(run: Run, install: Option<String>) -> Self {
        let mut choices = Vec::new();
        if let Some(address) = private_network(&run) {
            choices.push((Choice::PrivateNetworks, address));
        }
        if let Some(address) = run.public_source {
            choices.push((Choice::PublicAddress, address.to_string()));
        }
        if let Some(name) = run.from.clone().or(install) {
            choices.push((Choice::InstallName, name));
        }
        let mut sheet = Self {
            run,
            shows: Shows::default(),
            choices,
            row: 0,
            rules: None,
            link: None,
            stage: Stage::Reading,
            messages: None,
        };
        sheet.read();
        sheet
    }

    fn busy(&self) -> bool {
        matches!(self.stage, Stage::Reading | Stage::Sharing | Stage::Ending)
    }

    /// Whether this account makes links and changes what they show.
    fn can_share(&self) -> bool {
        self.rules.is_some_and(|rules| rules.can_share)
    }

    /// Whether ↑↓ and space move between the switches and turn them.
    fn switching(&self) -> bool {
        self.stage == Stage::Choosing && self.can_share() && !self.choices.is_empty()
    }

    /// Whether the switches say something else than the live link shows.
    fn changed(&self) -> bool {
        self.link
            .as_ref()
            .is_some_and(|link| link.shows != self.shows)
    }

    /// Whether Return makes the link or changes what it shows.
    fn can_send(&self) -> bool {
        self.can_share() && (self.link.is_none() || self.changed())
    }

    fn work(&mut self, stage: Stage, job: impl FnOnce() -> Message + Send + 'static) {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(job());
        });
        self.stage = stage;
        self.messages = Some(receiver);
    }

    fn read(&mut self) {
        let run = self.run.clone();
        self.work(Stage::Reading, move || match share::standing(&run) {
            Ok(standing) => Message::Read(standing),
            Err(message) => Message::Failed(message),
        });
    }

    fn share(&mut self) {
        let (run, shows) = (self.run.clone(), self.shows);
        self.work(Stage::Sharing, move || match share::make(&run, shows) {
            Ok(share) => Message::Shared(share),
            Err(message) => Message::Failed(message),
        });
    }

    fn end(&mut self, id: String) {
        self.work(Stage::Ending, move || match share::end(&id) {
            Ok(()) => Message::Ended,
            Err(message) => Message::Failed(message),
        });
    }

    /// Folds in what the thread said since the last frame.
    pub(super) fn tick(&mut self) -> Option<Done> {
        let message = self.messages.as_ref()?.try_recv().ok()?;
        self.messages = None;
        match message {
            Message::Read(standing) => {
                self.shows = standing.start();
                self.rules = Some(standing.rules);
                self.link = standing.link;
                self.stage = Stage::Choosing;
                None
            }
            Message::Shared(share) => {
                let done = Done::Shared {
                    url: share.url.clone(),
                    new: self.link.is_none(),
                };
                self.shows = share.shows;
                self.link = Some(share);
                self.stage = Stage::Choosing;
                Some(done)
            }
            Message::Ended => Some(Done::Ended),
            Message::Failed(sentence) => {
                self.stage = Stage::Failed(sentence);
                None
            }
        }
    }

    pub(super) fn keys(&self) -> Vec<(&'static str, &'static str)> {
        match self.stage {
            Stage::Reading | Stage::Sharing | Stage::Ending => Vec::new(),
            Stage::Stopping => vec![("\u{21B5}", "Stop Sharing"), ("\u{2190}", "Cancel")],
            Stage::Failed(_) => vec![("\u{21B5}", "Try Again"), ("\u{2190}", "Close")],
            Stage::Choosing => {
                let mut keys = Vec::new();
                if self.switching() {
                    keys.push(("\u{2191}\u{2193}", "Option"));
                    keys.push(("space", "Switch"));
                }
                if self.can_send() {
                    keys.push((
                        "\u{21B5}",
                        if self.link.is_some() {
                            "Update Link"
                        } else {
                            "Generate Link"
                        },
                    ));
                }
                if self.link.is_some() {
                    keys.push(("o", "Open Link"));
                    keys.push(("c", "Copy Link"));
                    keys.push(("x", "Stop Sharing"));
                }
                keys.push(("\u{2190}", "Close"));
                keys
            }
        }
    }

    /// The sentences at the top of the sheet: what the link does, or why there is none, and
    /// when it stops working.
    fn sentence(&self) -> String {
        match (&self.link, self.can_share()) {
            (Some(link), _) => {
                let mut sentence = "This run is a public page. Anyone with the link can open it without signing in.".to_string();
                if let Some(expiry) = link.expires_at.as_deref().and_then(share::expires_on) {
                    sentence.push(' ');
                    sentence.push_str(&expiry);
                }
                sentence
            }
            (None, true) => {
                let mut sentence = "Generate Link makes a public page of this run. Anyone with the link can open it without signing in.".to_string();
                if let Some(days) = self.rules.and_then(|rules| rules.expires_in_days) {
                    sentence.push(' ');
                    sentence.push_str(&share::expires_after(days));
                }
                sentence
            }
            (None, false) => share::NOT_ALLOWED.to_string(),
        }
    }
}

impl App {
    /// Opens the share sheet for a run that has ended. Sharing needs a person's login.
    pub(super) fn open_share(&mut self, run: Run) {
        if run.is_running() {
            return;
        }
        match &self.account.state {
            account_view::State::SignedIn { install, .. } => {
                let install = install.clone();
                self.share = Some(Sheet::new(run, Some(install)));
            }
            account_view::State::Token => {
                self.toast("RVRS_TOKEN only sends runs. Sign in with rvrs login to share one")
            }
            _ => self.toast("Sharing a run needs an account. \u{21E7}4 Account signs in"),
        }
    }

    pub(super) fn share_tick(&mut self) {
        let Some(done) = self.share.as_mut().and_then(Sheet::tick) else {
            return;
        };
        match done {
            Done::Shared { url, new: true } => {
                copy_to_clipboard(&url);
                self.toast("Link copied");
            }
            Done::Shared { new: false, .. } => self.toast("Link updated"),
            Done::Ended => {
                self.share = None;
                self.toast("Sharing stopped");
            }
        }
    }

    pub(super) fn share_key(&mut self, key: KeyEvent) {
        let Some(sheet) = &mut self.share else {
            return;
        };
        // The API is being asked. The answer comes in a moment.
        if sheet.busy() {
            return;
        }
        let close = matches!(key.code, KeyCode::Esc | KeyCode::Left);
        let url = sheet.link.as_ref().map(|link| link.url.clone());
        match (&sheet.stage, key.code) {
            (Stage::Stopping, _) if close => sheet.stage = Stage::Choosing,
            (_, _) if close => self.share = None,
            (Stage::Choosing, KeyCode::Down | KeyCode::Char('j')) if sheet.switching() => {
                sheet.row = (sheet.row + 1).min(sheet.choices.len().saturating_sub(1));
            }
            (Stage::Choosing, KeyCode::Up | KeyCode::Char('k')) if sheet.switching() => {
                sheet.row = sheet.row.saturating_sub(1);
            }
            (Stage::Choosing, KeyCode::Char(' ') | KeyCode::Right) if sheet.switching() => {
                if let Some((choice, _)) = sheet.choices.get(sheet.row) {
                    choice.flip(&mut sheet.shows);
                }
            }
            (Stage::Choosing, KeyCode::Enter) if sheet.can_send() => sheet.share(),
            // A read that failed is read again. A share that failed is sent again.
            (Stage::Failed(_), KeyCode::Enter) if sheet.rules.is_none() => sheet.read(),
            (Stage::Failed(_), KeyCode::Enter) => sheet.share(),
            (Stage::Choosing, KeyCode::Char('o')) => {
                if let Some(url) = url {
                    if !account::open_link(&url) {
                        self.toast("Couldn't open a browser. Copy the link with c");
                    }
                }
            }
            (Stage::Choosing, KeyCode::Char('c')) => {
                if let Some(url) = url {
                    self.copy(&url);
                }
            }
            (Stage::Choosing, KeyCode::Char('x')) if sheet.link.is_some() => {
                sheet.stage = Stage::Stopping;
            }
            (Stage::Stopping, KeyCode::Enter) => {
                if let Some(id) = sheet.link.as_ref().map(|link| link.id.clone()) {
                    sheet.end(id);
                }
            }
            _ => {}
        }
    }

    /// The sheet, over the middle of the screen.
    pub(super) fn draw_share(&self, frame: &mut Frame, area: Rect) {
        let Some(sheet) = &self.share else { return };
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let box_width = 64.min(area.width.saturating_sub(4));
        let columns = usize::from(box_width.saturating_sub(2));
        let spinner = crate::style::SPINNER[self.frame % crate::style::SPINNER.len()];
        // Sentences wrap to the box, two columns in from each side.
        let wrapped = |text: &str, paint: Paint| -> Vec<Line<'static>> {
            wrap(text, columns.saturating_sub(4))
                .into_iter()
                .map(|line| Line::styled(format!("  {line}"), paint))
                .collect()
        };
        let mut lines: Vec<Line> = vec![Line::default()];
        match (&sheet.stage, sheet.rules) {
            (Stage::Reading, _) => lines.push(Line::styled(
                format!("  {spinner} Reading the workspace\u{2019}s sharing settings"),
                dim,
            )),
            (Stage::Ending, _) => {
                lines.push(Line::styled(format!("  {spinner} Stopping the link"), dim));
            }
            // The read failed, so there is nothing to show but why.
            (Stage::Failed(sentence), None) => {
                lines.extend(wrapped(sentence, Paint::new().fg(Color::Red)));
            }
            (stage, _) => {
                if let Some(link) = &sheet.link {
                    lines.push(Line::styled(
                        format!("  {}", truncate_end(&link.url, columns.saturating_sub(2))),
                        bold.fg(ACCENT),
                    ));
                    lines.push(Line::default());
                }
                lines.extend(wrapped(&sheet.sentence(), dim));
                // A member who may not share sees no switches until they have a link.
                let shown = sheet.link.is_some() || sheet.can_share();
                if shown && !sheet.choices.is_empty() {
                    lines.push(Line::default());
                    for (index, (choice, detail)) in sheet.choices.iter().enumerate() {
                        let check = if choice.on(sheet.shows) {
                            "[\u{2713}]"
                        } else {
                            "[ ]"
                        };
                        let text = format!("  {check} {:<18}", choice.title());
                        let room = columns.saturating_sub(width(&text) + 1);
                        if sheet.switching() && index == sheet.row {
                            lines.push(Line::styled(
                                fit(&format!("{text}{}", truncate_end(detail, room)), columns),
                                selected(),
                            ));
                        } else {
                            lines.push(Line::from(vec![
                                Span::raw(text),
                                Span::styled(truncate_end(detail, room), dim),
                            ]));
                        }
                    }
                }
                let last: Vec<Line> = match stage {
                    Stage::Sharing if sheet.link.is_some() => {
                        vec![Line::styled(format!("  {spinner} Updating the link"), dim)]
                    }
                    Stage::Sharing => vec![Line::styled(
                        format!("  {spinner} Generating the link"),
                        dim,
                    )],
                    Stage::Failed(sentence) => wrapped(sentence, Paint::new().fg(Color::Red)),
                    Stage::Stopping => wrapped(
                        "The link stops working for everyone.",
                        Paint::new().fg(Color::Yellow),
                    ),
                    _ if sheet.can_send() => vec![Line::from(vec![
                        Span::styled(
                            "  <\u{21B5}> ",
                            Paint::new().fg(KEY).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            if sheet.link.is_some() {
                                "Update Link"
                            } else {
                                "Generate Link"
                            },
                            bold,
                        ),
                    ])],
                    _ => Vec::new(),
                };
                if !last.is_empty() {
                    lines.push(Line::default());
                    lines.extend(last);
                }
            }
        }
        let box_height = (lines.len() as u16 + 3).min(area.height.saturating_sub(2));
        let rect = Rect {
            x: area.x + (area.width - box_width) / 2,
            y: area.y + area.height.saturating_sub(box_height) / 2,
            width: box_width,
            height: box_height,
        };
        let block = panel(
            "Share Run",
            &truncate_middle(&sheet.run.target, usize::from(box_width).saturating_sub(20)),
            "",
        );
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

/// Words in lines of at most `columns`, a word longer than that on a line of its own.
fn wrap(text: &str, columns: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && width(&line) + 1 + width(word) > columns.max(1) {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use rvrs_core::model::Tool;
    use rvrs_core::FamilyChoice;

    #[test]
    fn sentences_wrap_at_words() {
        assert_eq!(
            wrap("Anyone with the link can open it", 12),
            ["Anyone with", "the link can", "open it"]
        );
        assert_eq!(wrap("", 12), Vec::<String>::new());
    }

    /// A sheet that read nothing yet, with one switch, and no thread behind it.
    fn sheet() -> Sheet {
        Sheet {
            run: Run::new(Tool::Ping, "192.0.2.1", FamilyChoice::Auto, 0, 0),
            shows: Shows::default(),
            choices: vec![(Choice::InstallName, "MacBook Pro".to_string())],
            row: 0,
            rules: None,
            link: None,
            stage: Stage::Reading,
            messages: None,
        }
    }

    fn link(shows: Shows, expires_at: Option<&str>) -> Share {
        Share {
            id: "q3Lr8XwT2a".to_string(),
            url: "https://app.rvrs.dev/r/q3Lr8XwT2a".to_string(),
            run: "4f6c8f0e-2b7d-4c55-9a8e-1d2f3a4b5c6d".to_string(),
            shows,
            created_at: "2026-11-23T20:15:04.000Z".to_string(),
            expires_at: expires_at.map(str::to_string),
        }
    }

    /// The sheet once the API answered this.
    fn read(standing: Standing) -> Sheet {
        let mut sheet = sheet();
        let (sender, receiver) = mpsc::channel();
        sender.send(Message::Read(standing)).unwrap();
        sheet.messages = Some(receiver);
        assert!(sheet.tick().is_none());
        sheet
    }

    fn rules(defaults: Shows, expires_in_days: Option<u32>, can_share: bool) -> Rules {
        Rules {
            defaults,
            expires_in_days,
            can_share,
        }
    }

    fn names(sheet: &Sheet) -> Vec<&'static str> {
        sheet.keys().into_iter().map(|(_, name)| name).collect()
    }

    #[test]
    fn a_new_link_starts_at_the_workspaces_defaults_and_says_when_it_stops() {
        let defaults = Shows {
            install_name: true,
            ..Shows::default()
        };
        let sheet = read(Standing {
            link: None,
            rules: rules(defaults, Some(30), true),
        });
        assert_eq!(sheet.stage, Stage::Choosing);
        assert_eq!(sheet.shows, defaults);
        assert_eq!(
            sheet.sentence(),
            "Generate Link makes a public page of this run. Anyone with the link can open it without signing in. The link stops working after 30 days."
        );
        assert_eq!(
            names(&sheet),
            ["Option", "Switch", "Generate Link", "Close"]
        );
    }

    #[test]
    fn a_shared_run_opens_on_its_link() {
        let shows = Shows {
            public_address: true,
            ..Shows::default()
        };
        let mut sheet = read(Standing {
            link: Some(link(shows, None)),
            rules: rules(Shows::default(), Some(30), true),
        });
        assert_eq!(sheet.shows, shows);
        assert_eq!(
            sheet.sentence(),
            "This run is a public page. Anyone with the link can open it without signing in."
        );
        assert_eq!(
            names(&sheet),
            [
                "Option",
                "Switch",
                "Open Link",
                "Copy Link",
                "Stop Sharing",
                "Close"
            ]
        );
        // A switch that moved makes Return change what the link shows.
        Choice::InstallName.flip(&mut sheet.shows);
        assert!(names(&sheet).contains(&"Update Link"));
    }

    #[test]
    fn a_link_that_expires_says_the_day() {
        let sheet = read(Standing {
            link: Some(link(Shows::default(), Some("2026-12-24T12:00:00.000Z"))),
            rules: rules(Shows::default(), Some(30), true),
        });
        let sentence = sheet.sentence();
        assert!(
            sentence.starts_with(
                "This run is a public page. Anyone with the link can open it without signing in. The link stops working on Dec 2"
            ),
            "{sentence}"
        );
    }

    #[test]
    fn a_member_who_may_not_share_reads_why_and_keeps_stop_sharing() {
        let sheet = read(Standing {
            link: None,
            rules: rules(Shows::default(), Some(30), false),
        });
        assert_eq!(
            sheet.sentence(),
            "Only owners and admins of this workspace share runs."
        );
        assert_eq!(names(&sheet), ["Close"]);
        assert!(!sheet.can_send() && !sheet.switching());
        let held = read(Standing {
            link: Some(link(Shows::default(), None)),
            rules: rules(Shows::default(), None, false),
        });
        assert_eq!(
            names(&held),
            ["Open Link", "Copy Link", "Stop Sharing", "Close"]
        );
    }

    #[test]
    fn a_new_link_is_copied_and_a_changed_one_is_not() {
        let mut sheet = read(Standing {
            link: None,
            rules: Rules::default(),
        });
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Message::Shared(link(Shows::default(), None)))
            .unwrap();
        sheet.messages = Some(receiver);
        assert!(matches!(sheet.tick(), Some(Done::Shared { new: true, .. })));
        let (sender, receiver) = mpsc::channel();
        let wider = Shows {
            public_address: true,
            ..Shows::default()
        };
        sender.send(Message::Shared(link(wider, None))).unwrap();
        sheet.messages = Some(receiver);
        assert!(matches!(
            sheet.tick(),
            Some(Done::Shared { new: false, .. })
        ));
        assert_eq!(sheet.shows, wider);
        assert_eq!(sheet.stage, Stage::Choosing);
    }
}
