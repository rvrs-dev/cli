//! Help: `?` opens it over the screen, on the page for what is on screen. A tool's page has the
//! keys its result takes, then the tool's commands as `rvrs <tool> --help` lists them, so the
//! Console is also where to look up a flag. The Keys page is the Console's own keys.

use std::cell::Cell;

use super::*;

/// One page of the help.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    Keys,
    Tool(Tool),
}

impl Page {
    /// Keys, then every tool rvrs has, in the Console's order.
    fn all() -> Vec<Page> {
        std::iter::once(Page::Keys)
            .chain(
                Tool::ALL
                    .into_iter()
                    .filter(|tool| tool.is_available())
                    .map(Page::Tool),
            )
            .collect()
    }

    fn title(self) -> &'static str {
        match self {
            Page::Keys => "Keys",
            Page::Tool(tool) => tool.command(),
        }
    }

    /// The key that opens the page: `?` for Keys, the tool's letter for a tool.
    fn key(self) -> char {
        match self {
            Page::Keys => '?',
            Page::Tool(tool) => run_picker::letter(tool),
        }
    }
}

/// The help while it is open: its page, and how far down it is scrolled.
pub(super) struct Help {
    page: Page,
    scroll: usize,
    /// How far it can scroll, as the last frame drew it.
    room: Cell<usize>,
}

/// The keys a tool's result takes, as rows of the help.
fn tool_keys(tool: Tool) -> Vec<(&'static str, &'static str)> {
    let hop = [
        ("\u{2191} \u{2193} j k", "Select a hop"),
        ("/", "Search the hops by address, name or network"),
        (
            "d  \u{2192}",
            "Show the hop's details beside the hops, and hide them",
        ),
        ("tab", "Move the keys between the hops and the details"),
        (
            "t m p b",
            "On a hop: run traceroute, MTR, ping or BGP against its host",
        ),
        (
            "c n w",
            "On a hop: copy its address, copy its hostname, open Whois",
        ),
        ("r", "Pick any tool for the selected hop's host"),
    ];
    let kept = [
        (
            "s a S f",
            "Save the run, put it in cases, share it, keep its target as a favorite",
        ),
        ("\u{2190} esc", "Back to the list"),
    ];
    let mut rows: Vec<(&'static str, &'static str)> = match tool {
        Tool::Traceroute => {
            let mut rows = vec![(
                "t, a target, \u{21B5}",
                "Trace the path to the target from this device",
            )];
            rows.extend(hop);
            rows.push(("R", "Trace the path again"));
            rows
        }
        Tool::Mtr => {
            let mut rows = vec![
                (
                    "m, a target, \u{21B5}",
                    "Start an MTR, which runs round after round until s stops it",
                ),
                ("s", "Stop the MTR. Once it has stopped, s saves it"),
            ];
            rows.extend(hop);
            rows.push((
                "v",
                "Switch between each hop's figures and a wide strip of its rounds",
            ));
            rows.push(("R", "Start the MTR again"));
            rows
        }
        Tool::Ping => vec![
            (
                "p, a target, \u{21B5}",
                "Send 20 packets a second apart and chart every reply",
            ),
            ("s", "Stop the ping. Once it has ended, s saves it"),
            ("c", "Copy the address that was pinged"),
            ("r", "Pick another tool for the target"),
            ("R", "Ping again"),
        ],
        Tool::Dns => vec![
            (
                "d, a name, \u{21B5}",
                "Ask this device's resolver, public resolvers and the authoritative servers",
            ),
            ("1 2 3 4", "Overview, Records, Compare, Trace"),
            (
                "tab",
                "The next tab, or between the records and a record's details",
            ),
            ("\u{2191} \u{2193} j k", "Select a record, a source or a step"),
            ("/", "Search the records by name, type or value"),
            (
                "d  \u{2192}",
                "Show the record's details beside the records, and hide them",
            ),
            ("o", "In Records: the next source's answers"),
            (
                "c z",
                "Copy the record's value, or the record as a zone line",
            ),
            (
                "t m p b",
                "On a record that names a host: run traceroute, MTR, ping or BGP against it",
            ),
            ("R", "Look the name up again"),
        ],
        Tool::Bgp => vec![
            (
                "b, a target, \u{21B5}",
                "Look up a host, an address, a prefix or an AS number",
            ),
            (
                "1 2 3 4 5",
                "Overview, Paths, Prefixes, History, Sources. A network has Neighbors in place of Paths",
            ),
            ("tab", "The next tab"),
            ("\u{2191} \u{2193} j k", "Select a row"),
            ("/", "Search the rows: a network, a prefix, an AS path"),
            ("\u{2192}", "Open the selected network's or prefix's own lookup"),
            ("d  v", "Show an AS path or an RPKI result beside the list"),
            (
                "o",
                "The next side of Neighbors, filter of Prefixes or range of History",
            ),
            ("c", "Copy the selected row"),
            (
                "t m p",
                "Run traceroute, MTR or ping against the host that was looked up",
            ),
            ("R", "Look it up again"),
        ],
        Tool::Iperf3 => Vec::new(),
    };
    rows.extend(kept);
    rows
}

/// The Console's own keys: the lists, the field, the search and the sections.
fn console_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "t m p d b",
            "Pick traceroute, MTR, ping, dig or BGP and type a target",
        ),
        (":", "Type a target for the tool chosen last"),
        (
            "\u{21B5}",
            "In the field: run it. ping example.com picks the tool too",
        ),
        (
            "\u{2191} \u{2193}  \u{2192}",
            "In the field: pick a host it suggests, from favorites, then history, and complete it",
        ),
        ("tab", "In the field: the next tool"),
        (
            "/",
            "Search the list on screen. \u{21B5} keeps the search, esc drops it",
        ),
        ("\u{2191} \u{2193} j k", "Move through the list"),
        ("g G", "The first row, the last row"),
        ("pg up  pg dn", "Move a screen at a time"),
        (
            "\u{2192} \u{21B5}",
            "Open the selected run, case or favorite",
        ),
        (
            "\u{2190} esc",
            "Go back one level. esc with nothing to go back from quits",
        ),
        (
            "s",
            "On a run: save it, or remove it from saved runs. While it runs: stop it",
        ),
        ("a", "On a run: put it in cases, or make a new one"),
        (
            "S",
            "On a run: choose what its public page shows, then generate the link",
        ),
        ("x", "Inside a case: take the run out of it. It stays saved"),
        ("f", "Keep the host as a favorite, or stop keeping it"),
        ("r", "Pick a tool to run against the selected run's host"),
        (
            "\u{21E7}1 to \u{21E7}4",
            "Console, Favorites, Cases, Account, the tabs in the footer",
        ),
        ("?", "This help. A tool's letter opens that tool's page"),
    ]
}

/// A row of keys: the keys in blue, then what they do.
fn row(key: &str, what: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {:<20}", key), Paint::new().fg(KEY)),
        Span::raw(what.to_string()),
    ])
}

/// What a page says, top to bottom.
fn page_lines(page: Page) -> Vec<Line<'static>> {
    let bold = Paint::new().add_modifier(Modifier::BOLD);
    let dim = Paint::new().add_modifier(Modifier::DIM);
    let mut lines = vec![Line::default()];
    match page {
        Page::Keys => {
            lines.push(Line::styled("  THE CONSOLE", bold));
            lines.extend(console_keys().into_iter().map(|(key, what)| row(key, what)));
            lines.push(Line::default());
            lines.push(Line::from(vec![
                Span::styled("  Latency  ", dim),
                Span::styled("under 50 ms  ", Paint::new().fg(Color::Green)),
                Span::styled("50 to 150 ms  ", Paint::new().fg(Color::Yellow)),
                Span::styled("over 150 ms", Paint::new().fg(Color::Red)),
            ]));
        }
        Page::Tool(tool) => {
            lines.push(Line::styled("  IN THE CONSOLE", bold));
            lines.extend(
                tool_keys(tool)
                    .into_iter()
                    .map(|(key, what)| row(key, what)),
            );
            lines.push(Line::default());
            lines.push(Line::styled("  ON THE COMMAND LINE", bold));
            let style = Style { color: true };
            let text = match tool {
                Tool::Traceroute => crate::help::traceroute(style),
                Tool::Mtr => crate::help::mtr(style),
                Tool::Ping => crate::help::ping(style),
                Tool::Dns => crate::help::dig(style),
                Tool::Bgp => crate::help::bgp(style),
                Tool::Iperf3 => String::new(),
            };
            lines.extend(text.lines().map(|line| ansi_line(&format!("  {line}"))));
        }
    }
    lines.push(Line::default());
    lines
}

impl App {
    /// Opens the help on the page for what is on screen: the open run's tool, the tool the
    /// field runs, or the Console's keys.
    pub(super) fn open_help(&mut self) {
        let on_start = self.pane == Pane::Start || self.shown.is_none();
        let tool = if on_start {
            self.field.then_some(self.tool)
        } else {
            self.run().map(|run| run.tool)
        };
        let page = tool
            .filter(|tool| tool.is_available())
            .map_or(Page::Keys, Page::Tool);
        self.help = Some(Help {
            page,
            scroll: 0,
            room: Cell::new(0),
        });
    }

    /// The help takes every key while it is open: tab turns the page, a tool's letter opens its
    /// page, the arrows scroll, and esc closes it.
    pub(super) fn help_key(&mut self, key: KeyEvent) {
        let Some(help) = &mut self.help else { return };
        let pages = Page::all();
        let at = pages
            .iter()
            .position(|page| *page == help.page)
            .unwrap_or(0);
        let turn = |help: &mut Help, page: Page| {
            help.page = page;
            help.scroll = 0;
        };
        let room = help.room.get();
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => self.help = None,
            KeyCode::Char('?') if help.page == Page::Keys => self.help = None,
            KeyCode::Tab => turn(help, pages[(at + 1) % pages.len()]),
            KeyCode::BackTab => turn(help, pages[(at + pages.len() - 1) % pages.len()]),
            KeyCode::Down | KeyCode::Char('j') => help.scroll = (help.scroll + 1).min(room),
            KeyCode::Up | KeyCode::Char('k') => help.scroll = help.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                help.scroll = (help.scroll + self.page).min(room)
            }
            KeyCode::PageUp => help.scroll = help.scroll.saturating_sub(self.page),
            KeyCode::Home | KeyCode::Char('g') => help.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => help.scroll = room,
            KeyCode::Char(typed) => {
                if let Some(page) = pages.iter().find(|page| page.key() == typed) {
                    turn(help, *page);
                }
            }
            _ => {}
        }
    }

    /// The keys while the help is open.
    pub(super) fn help_keys(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("tab", "Next Page"),
            ("t m p d b", "Tool"),
            ("?", "Keys"),
            ("\u{2191}\u{2193}", "Scroll"),
            ("esc", "Close"),
        ]
    }

    /// The help over the middle of the screen: the pages as tabs, then the open one.
    pub(super) fn draw_help(&self, frame: &mut Frame, area: Rect) {
        let Some(help) = &self.help else { return };
        let dim = Paint::new().add_modifier(Modifier::DIM);
        let box_width = 112.min(area.width.saturating_sub(4));
        let lines = page_lines(help.page);
        let box_height = (lines.len() as u16 + 4).min(area.height.saturating_sub(2));
        let rect = Rect {
            x: area.x + (area.width - box_width) / 2,
            y: area.y + (area.height - box_height) / 2,
            width: box_width,
            height: box_height,
        };
        let block = panel("Help", help.page.title(), "");
        let inner = block.inner(rect);
        frame.render_widget(Clear, rect);
        frame.render_widget(block, rect);
        let [tabs, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        let mut spans = vec![Span::raw(" ")];
        for page in Page::all() {
            spans.push(Span::styled(
                format!("<{}>", page.key()),
                Paint::new().fg(KEY).add_modifier(Modifier::BOLD),
            ));
            spans.push(if page == help.page {
                Span::styled(
                    format!(" {} ", page.title()),
                    selected().add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(format!(" {} ", page.title()), dim)
            });
            spans.push(Span::raw(" "));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), tabs);
        let room = lines.len().saturating_sub(usize::from(body.height));
        help.room.set(room);
        let scroll = help.scroll.min(room);
        frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), body);
    }
}
