//! The terminal itself: its size, Ctrl-C, and a region of lines redrawn in place.

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratatui::crossterm::terminal;

static INTERRUPTS: AtomicUsize = AtomicUsize::new(0);

extern "C" fn on_interrupt(_signal: libc::c_int) {
    INTERRUPTS.fetch_add(1, Ordering::Relaxed);
}

/// Counts Ctrl-C instead of dying on it, so a run can stop cleanly and still print what it has.
pub fn catch_interrupts() {
    // SAFETY: the handler only touches an atomic.
    unsafe {
        libc::signal(
            libc::SIGINT,
            on_interrupt as extern "C" fn(libc::c_int) as libc::sighandler_t,
        );
    }
}

pub fn interrupts() -> usize {
    INTERRUPTS.load(Ordering::Relaxed)
}

/// Columns and rows, or the classic 80 by 24 when stdout is not a terminal.
pub fn size() -> (usize, usize) {
    terminal::size().map_or((80, 24), |(columns, rows)| {
        (usize::from(columns.max(20)), usize::from(rows.max(5)))
    })
}

/// Lines on stdout that are rewritten where they stand, so a result fills in as it arrives and
/// stays in the scrollback when it is done.
pub struct LiveRegion {
    drawn: usize,
}

impl LiveRegion {
    pub fn start() -> Self {
        print!("\x1b[?25l");
        Self { drawn: 0 }
    }

    /// Redraws the region. While a run is live it is clipped to the terminal's height, because
    /// lines that have scrolled away can't be rewritten.
    pub fn draw(&mut self, lines: &[String], clip: bool) {
        let (_, rows) = size();
        let visible: &[String] = if clip && lines.len() > rows.saturating_sub(1) {
            &lines[lines.len() - rows.saturating_sub(1)..]
        } else {
            lines
        };
        let mut out = String::new();
        if self.drawn > 0 {
            out.push_str(&format!("\x1b[{}A", self.drawn));
        }
        out.push('\r');
        for line in visible {
            out.push_str(line);
            out.push_str("\x1b[K\n");
        }
        out.push_str("\x1b[J");
        self.drawn = visible.len();
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }

    /// Writes lines that stay, above the region. The region is drawn again by the next `draw`.
    pub fn print_above(&mut self, lines: &[String]) {
        if lines.is_empty() {
            return;
        }
        let mut out = String::new();
        if self.drawn > 0 {
            out.push_str(&format!("\x1b[{}A", self.drawn));
        }
        out.push('\r');
        for line in lines {
            out.push_str(line);
            out.push_str("\x1b[K\n");
        }
        out.push_str("\x1b[J");
        self.drawn = 0;
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
    }

    /// Leaves the last drawing in place and gives the cursor back.
    pub fn finish(self) {}
}

impl Drop for LiveRegion {
    fn drop(&mut self) {
        print!("\x1b[?25h");
        let _ = std::io::stdout().flush();
    }
}
