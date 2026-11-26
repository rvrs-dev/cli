//! Color and width helpers. rvrs paints with the terminal's own sixteen colors, so it follows
//! the person's theme in light and dark, and never says something by color alone. COLOR.md at
//! the repo root says which color each figure and each state takes.

use std::io::IsTerminal;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

impl ColorChoice {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "always" => Some(Self::Always),
            "never" => Some(Self::Never),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub color: bool,
}

impl Style {
    #[allow(dead_code)]
    pub const PLAIN: Style = Style { color: false };

    /// `--color` wins, then NO_COLOR and CLICOLOR_FORCE, then whether stdout is a terminal.
    pub fn detect(choice: ColorChoice) -> Self {
        let color = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                let set =
                    |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
                if set("NO_COLOR") {
                    false
                } else if set("CLICOLOR_FORCE") {
                    true
                } else {
                    std::io::stdout().is_terminal()
                        && std::env::var("TERM").map_or(true, |term| term != "dumb")
                }
            }
        };
        Self { color }
    }

    /// For messages on stderr.
    pub fn detect_stderr() -> Self {
        let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        Self {
            color: !set("NO_COLOR") && std::io::stderr().is_terminal(),
        }
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.color && !text.is_empty() {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    pub fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    pub fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    pub fn yellow(self, text: &str) -> String {
        self.paint("33", text)
    }

    pub fn red(self, text: &str) -> String {
        self.paint("31", text)
    }

    pub fn blue(self, text: &str) -> String {
        self.paint("34", text)
    }

    /// The p99's, jitter's and upload's color in a terminal (COLOR.md).
    pub fn magenta(self, text: &str) -> String {
        self.paint("35", text)
    }

    /// Latency in its tone: green, yellow or red. The number itself is always the text.
    pub fn latency(self, latency: std::time::Duration, text: &str) -> String {
        match latency_tone(latency) {
            Tone::Fast => self.green(text),
            Tone::Slow => self.yellow(text),
            Tone::VerySlow => self.red(text),
        }
    }

    pub fn bold_green(self, text: &str) -> String {
        self.paint("1;32", text)
    }
}

/// How a latency reads at a glance. The thresholds are fixed and stated in the help: under
/// 50 ms, up to 150 ms, beyond.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Fast,
    Slow,
    VerySlow,
}

pub fn latency_tone(latency: std::time::Duration) -> Tone {
    match latency.as_micros() {
        0..=49_999 => Tone::Fast,
        50_000..=150_000 => Tone::Slow,
        _ => Tone::VerySlow,
    }
}

pub fn width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Shortens to `max` columns by cutting the middle, where hostnames carry the least.
pub fn truncate_middle(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max <= 1 {
        return "\u{2026}".repeat(max);
    }
    let keep = max - 1;
    let head_budget = keep.div_ceil(2);
    let tail_budget = keep - head_budget;
    let mut head = String::new();
    let mut used = 0;
    for character in text.chars() {
        let cell = character.width().unwrap_or(0);
        if used + cell > head_budget {
            break;
        }
        used += cell;
        head.push(character);
    }
    let mut tail: Vec<char> = Vec::new();
    used = 0;
    for character in text.chars().rev() {
        let cell = character.width().unwrap_or(0);
        if used + cell > tail_budget {
            break;
        }
        used += cell;
        tail.push(character);
    }
    tail.reverse();
    format!("{head}\u{2026}{}", tail.into_iter().collect::<String>())
}

/// Shortens to `max` columns by cutting the end.
pub fn truncate_end(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for character in text.chars() {
        let cell = character.width().unwrap_or(0);
        if used + cell > max - 1 {
            break;
        }
        used += cell;
        out.push(character);
    }
    out.push('\u{2026}');
    out
}

/// The width of text that may carry color codes.
pub fn visible_width(text: &str) -> usize {
    let mut total = 0;
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            for next in characters.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            total += character.width().unwrap_or(0);
        }
    }
    total
}

pub const SPINNER: [&str; 10] = [
    "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280F}",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_middle_goes_first() {
        assert_eq!(
            truncate_middle("ae1.cr1.fra.example.net", 11),
            "ae1.c\u{2026}e.net"
        );
        assert_eq!(truncate_middle("short", 10), "short");
        assert_eq!(width(&truncate_middle("c63364d7.dsl.example.net", 16)), 16);
    }

    #[test]
    fn color_codes_have_no_width() {
        let style = Style { color: true };
        assert_eq!(visible_width(&style.bold_green("Reached")), 7);
        assert_eq!(Style::PLAIN.red("x"), "x");
    }
}
