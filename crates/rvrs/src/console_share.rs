//! The share sheet: `S` on a run that has ended. It opens over the screen with what the link
//! does and the switches for what the page shows, and makes nothing until Return (the user's
//! rule for every surface, 2026-11-23: the sheet first, then the link). The link is made on a
//! thread, so the Console keeps drawing, then shown with Open Link, Copy Link and Stop Sharing.

use std::sync::mpsc::{self, Receiver};

use crate::share::{self, Share, Shows};

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
            Choice::InstallName => "Install Name",
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
    /// The switches, before anything is made.
    Choosing,
    /// Asking the API for the link.
    Sharing,
    Shared(Share),
    /// Asked whether to end the link.
    Stopping(Share),
    /// Asking the API to end the link.
    Ending,
    Failed(String),
}

/// What the thread doing the work tells the Console.
enum Message {
    Shared(Share),
    Ended,
    Failed(String),
}

/// What the Console does once the thread is done.
pub(super) enum Done {
    Shared(String),
    Ended,
}

pub(super) struct Sheet {
    pub(super) run: Run,
    shows: Shows,
    /// The switches this run has something for, each with what it covers in the run.
    choices: Vec<(Choice, String)>,
    row: usize,
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
        Self {
            run,
            shows: Shows::default(),
            choices,
            row: 0,
            stage: Stage::Choosing,
            messages: None,
        }
    }

    fn busy(&self) -> bool {
        matches!(self.stage, Stage::Sharing | Stage::Ending)
    }

    fn share(&mut self) {
        let (sender, receiver) = mpsc::channel();
        let (run, shows) = (self.run.clone(), self.shows);
        std::thread::spawn(move || {
            let _ = sender.send(match share::make(&run, shows) {
                Ok(share) => Message::Shared(share),
                Err(message) => Message::Failed(message),
            });
        });
        self.stage = Stage::Sharing;
        self.messages = Some(receiver);
    }

    fn end(&mut self, id: String) {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(match share::end(&id) {
                Ok(()) => Message::Ended,
                Err(message) => Message::Failed(message),
            });
        });
        self.stage = Stage::Ending;
        self.messages = Some(receiver);
    }

    /// Folds in what the thread said since the last frame.
    pub(super) fn tick(&mut self) -> Option<Done> {
        let message = self.messages.as_ref()?.try_recv().ok()?;
        self.messages = None;
        match message {
            Message::Shared(share) => {
                let url = share.url.clone();
                self.stage = Stage::Shared(share);
                Some(Done::Shared(url))
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
            Stage::Choosing if self.choices.is_empty() => {
                vec![("\u{21B5}", "Generate Link"), ("\u{2190}", "Close")]
            }
            Stage::Choosing => vec![
                ("\u{2191}\u{2193}", "Option"),
                ("space", "Switch"),
                ("\u{21B5}", "Generate Link"),
                ("\u{2190}", "Close"),
            ],
            Stage::Shared(_) => vec![
                ("o", "Open Link"),
                ("c", "Copy Link"),
                ("x", "Stop Sharing"),
                ("\u{2190}", "Close"),
            ],
            Stage::Stopping(_) => vec![("\u{21B5}", "Stop Sharing"), ("\u{2190}", "Cancel")],
            Stage::Failed(_) => vec![("\u{21B5}", "Try Again"), ("\u{2190}", "Close")],
            Stage::Sharing | Stage::Ending => Vec::new(),
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
            Done::Shared(url) => {
                copy_to_clipboard(&url);
                self.toast("Link copied");
            }
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
        match (&sheet.stage, key.code) {
            (Stage::Stopping(share), _) if close => sheet.stage = Stage::Shared(share.clone()),
            (_, _) if close => self.share = None,
            (Stage::Choosing, KeyCode::Down | KeyCode::Char('j')) => {
                sheet.row = (sheet.row + 1).min(sheet.choices.len().saturating_sub(1));
            }
            (Stage::Choosing, KeyCode::Up | KeyCode::Char('k')) => {
                sheet.row = sheet.row.saturating_sub(1);
            }
            (Stage::Choosing, KeyCode::Char(' ') | KeyCode::Right) => {
                if let Some((choice, _)) = sheet.choices.get(sheet.row) {
                    choice.flip(&mut sheet.shows);
                }
            }
            (Stage::Choosing | Stage::Failed(_), KeyCode::Enter) => sheet.share(),
            (Stage::Shared(share), KeyCode::Char('o')) => {
                let url = share.url.clone();
                if !account::open_link(&url) {
                    self.toast("Couldn't open a browser. Copy the link with c");
                }
            }
            (Stage::Shared(share), KeyCode::Char('c')) => {
                let url = share.url.clone();
                self.copy(&url);
            }
            (Stage::Shared(share), KeyCode::Char('x')) => {
                sheet.stage = Stage::Stopping(share.clone());
            }
            (Stage::Stopping(share), KeyCode::Enter) => {
                let id = share.id.clone();
                sheet.end(id);
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
        match &sheet.stage {
            Stage::Choosing | Stage::Sharing | Stage::Failed(_) => {
                lines.extend(wrapped(
                    "Generate Link makes a public page of this run. Anyone with the link can open it without signing in.",
                    dim,
                ));
                if !sheet.choices.is_empty() {
                    lines.push(Line::default());
                }
                let choosing = sheet.stage == Stage::Choosing;
                for (index, (choice, detail)) in sheet.choices.iter().enumerate() {
                    let check = if choice.on(sheet.shows) {
                        "[\u{2713}]"
                    } else {
                        "[ ]"
                    };
                    let text = format!("  {check} {:<18}", choice.title());
                    let room = columns.saturating_sub(width(&text) + 1);
                    if choosing && index == sheet.row {
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
                lines.push(Line::default());
                match &sheet.stage {
                    Stage::Sharing => lines.push(Line::styled(
                        format!("  {spinner} Generating the link"),
                        dim,
                    )),
                    Stage::Failed(sentence) => {
                        lines.extend(wrapped(sentence, Paint::new().fg(Color::Red)))
                    }
                    _ => lines.push(Line::from(vec![
                        Span::styled(
                            "  <\u{21B5}> ",
                            Paint::new().fg(KEY).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled("Generate Link", bold),
                    ])),
                }
            }
            Stage::Shared(share) | Stage::Stopping(share) => {
                lines.push(Line::styled(
                    format!("  {}", truncate_end(&share.url, columns.saturating_sub(2))),
                    bold.fg(ACCENT),
                ));
                lines.push(Line::default());
                lines.extend(wrapped(
                    &format!(
                        "This run is a public page. Anyone with the link can open it without signing in. {}",
                        share::what_it_shows(share.shows)
                    ),
                    dim,
                ));
                if matches!(sheet.stage, Stage::Stopping(_)) {
                    lines.push(Line::default());
                    lines.extend(wrapped(
                        "The link stops working for everyone.",
                        Paint::new().fg(Color::Yellow),
                    ));
                }
            }
            Stage::Ending => {
                lines.push(Line::styled(format!("  {spinner} Stopping the link"), dim));
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

    #[test]
    fn sentences_wrap_at_words() {
        assert_eq!(
            wrap("Anyone with the link can open it", 12),
            ["Anyone with", "the link can", "open it"]
        );
        assert_eq!(wrap("", 12), Vec::<String>::new());
    }
}
