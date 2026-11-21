//! The Console's Account section (⇧4): who this install is signed in as, and signing in and out
//! without leaving the Console. The login is the one `rvrs login` runs, on a thread of its own,
//! so the Console keeps drawing while it waits for the person to confirm the code.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style as Paint};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::account::{self, Credential, Poll};

/// Where this install stands with its account.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    SignedOut,
    /// `RVRS_TOKEN` is set and takes the place of any stored credential.
    Token,
    SignedIn {
        email: String,
        workspace: String,
        install: String,
    },
    /// Asking the API for a login.
    Starting,
    /// The code is shown and the person has to confirm it.
    Waiting {
        code: String,
        link: String,
    },
    SigningOut,
    /// The last sign-in or sign-out ended in this sentence.
    Failed(String),
}

/// What the thread doing the work tells the Console.
enum Message {
    Waiting { code: String, link: String },
    SignedIn(Credential),
    SignedOut(String),
    Failed(String),
}

pub struct Account {
    pub state: State,
    messages: Option<Receiver<Message>>,
    stop: Option<Arc<AtomicBool>>,
}

impl Account {
    /// As this device has it now, read from the environment and the stored credential. Asks
    /// the API nothing, so the Console opens at once.
    pub fn load() -> Self {
        Self {
            state: stored_state(),
            messages: None,
            stop: None,
        }
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self.state,
            State::Starting | State::Waiting { .. } | State::SigningOut
        )
    }

    /// Starts a login on a thread: it asks the API, opens the link, then asks every few
    /// seconds until the person confirms or declines.
    pub fn sign_in(&mut self) {
        if self.is_busy() || account::env_token() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        std::thread::spawn(move || {
            let started = match account::start_login() {
                Ok(started) => started,
                Err(message) => {
                    let _ = sender.send(Message::Failed(message));
                    return;
                }
            };
            let _ = sender.send(Message::Waiting {
                code: started.user_code.clone(),
                link: started.link.clone(),
            });
            account::open_link(&started.link);
            let mut pause = started.interval;
            loop {
                let wake = Instant::now() + pause;
                while Instant::now() < wake {
                    if stopped.load(Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                match account::poll_login(&started) {
                    Poll::Waiting => {}
                    Poll::SlowDown => pause += Duration::from_secs(5),
                    Poll::SignedIn(credential) => {
                        let _ = sender.send(Message::SignedIn(credential));
                        return;
                    }
                    Poll::Failed(message) => {
                        let _ = sender.send(Message::Failed(message));
                        return;
                    }
                }
            }
        });
        self.state = State::Starting;
        self.messages = Some(receiver);
        self.stop = Some(stop);
    }

    /// Revokes the credential at the API on a thread and forgets it here.
    pub fn sign_out(&mut self) {
        if !matches!(self.state, State::SignedIn { .. }) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let sentence = account::sign_out().unwrap_or_else(|| "Not signed in.".to_string());
            let _ = sender.send(Message::SignedOut(sentence));
        });
        self.state = State::SigningOut;
        self.messages = Some(receiver);
        self.stop = None;
    }

    /// Stops a login that is waiting. Nothing is signed in.
    pub fn cancel(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.messages = None;
        self.state = stored_state();
    }

    /// Folds in what the thread said since the last frame. The answer is a sentence to show
    /// as a toast when something ended.
    pub fn tick(&mut self) -> Option<String> {
        let message = self.messages.as_ref()?.try_recv().ok()?;
        match message {
            Message::Waiting { code, link } => {
                self.state = State::Waiting { code, link };
                None
            }
            Message::SignedIn(credential) => {
                self.finish();
                crate::sync::start();
                let toast = format!("Signed in as {}", credential.email);
                self.state = State::SignedIn {
                    email: credential.email,
                    workspace: credential.workspace,
                    install: credential.install,
                };
                Some(toast)
            }
            Message::SignedOut(sentence) => {
                self.finish();
                self.state = stored_state();
                Some(sentence.trim_end_matches('.').to_string())
            }
            Message::Failed(message) => {
                self.finish();
                self.state = State::Failed(message);
                None
            }
        }
    }

    fn finish(&mut self) {
        self.messages = None;
        self.stop = None;
    }

    /// The Account fact in the header.
    pub fn header(&self) -> (String, bool) {
        match &self.state {
            State::SignedIn {
                email, workspace, ..
            } => (format!("{email} \u{00B7} {workspace}"), true),
            State::Token => ("RVRS_TOKEN".into(), true),
            State::Starting | State::Waiting { .. } => ("signing in".into(), false),
            State::SigningOut => ("signing out".into(), false),
            State::SignedOut | State::Failed(_) => ("not signed in".into(), false),
        }
    }

    /// The keys that work in the section now.
    pub fn keys(&self) -> Vec<(&'static str, &'static str)> {
        match self.state {
            State::SignedOut | State::Failed(_) => vec![("s", "Sign In")],
            State::SignedIn { .. } => vec![("q", "Sign Out")],
            State::Waiting { .. } => {
                vec![("o", "Open Link"), ("c", "Copy Link"), ("esc", "Cancel")]
            }
            State::Starting => vec![("esc", "Cancel")],
            State::Token | State::SigningOut => Vec::new(),
        }
    }

    /// The section's panel content.
    pub fn draw(&self, frame: &mut Frame, area: Rect, frame_number: usize, label: Color) {
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let bold = Paint::new().add_modifier(Modifier::BOLD);
        let fact = |name: &str, value: String, paint: Paint| {
            Line::from(vec![
                Span::styled(format!("  {name:<11}"), Paint::new().fg(label)),
                Span::styled(value, paint),
            ])
        };
        let spinner = crate::style::SPINNER[frame_number % crate::style::SPINNER.len()];
        let mut lines = vec![Line::raw("")];
        match &self.state {
            State::SignedOut => {
                lines.push(fact("Account", "not signed in".into(), dim));
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    "  Signing in keeps your runs, saved runs, cases and favorites in your account at app.rvrs.dev, the same on every device you sign in on.",
                    dim,
                ));
            }
            State::Token => {
                lines.push(fact("Account", "signed in with RVRS_TOKEN".into(), bold));
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    "  RVRS_TOKEN takes the place of a login. Unset it to sign in or out here.",
                    dim,
                ));
            }
            State::SignedIn {
                email,
                workspace,
                install,
            } => {
                lines.push(fact("Signed in", email.clone(), bold));
                lines.push(fact("Workspace", workspace.clone(), Paint::new()));
                lines.push(fact("Install", install.clone(), Paint::new()));
                lines.push(fact("API", account::api_base(), dim));
            }
            State::Starting => {
                lines.push(Line::styled(format!("  {spinner} Starting the login"), dim));
            }
            State::Waiting { code, link } => {
                lines.push(fact("Code", code.clone(), bold.fg(Color::Cyan)));
                lines.push(fact("Open", link.clone(), Paint::new()));
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    format!("  {spinner} Waiting for you to confirm the code in the browser"),
                    dim,
                ));
            }
            State::SigningOut => {
                lines.push(Line::styled(format!("  {spinner} Signing out"), dim));
            }
            State::Failed(message) => {
                lines.push(Line::styled(
                    format!("  {message}"),
                    Paint::new().fg(Color::Red),
                ));
            }
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    }
}

/// The state this device's environment and stored credential give.
fn stored_state() -> State {
    if account::env_token() {
        return State::Token;
    }
    match Credential::stored() {
        Some(credential) => State::SignedIn {
            email: credential.email,
            workspace: credential.workspace,
            install: credential.install,
        },
        None => State::SignedOut,
    }
}
