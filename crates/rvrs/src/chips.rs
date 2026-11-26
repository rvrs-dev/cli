//! The four figures above a latency chart, as every rvrs surface shows them (PARITY.md, pattern
//! 8): p50, p95, p99 and loss, in that order, each after a dot in its figure's color from
//! COLOR.md. Plain output and the Console draw them from [`chips`], so both say the same.

use rvrs_core::format;
use rvrs_core::model::HopStats;

use crate::style::Style;

/// A figure's terminal color, by COLOR.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hue {
    Green,
    Blue,
    Magenta,
    Red,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub name: &'static str,
    /// Milliseconds with one decimal and no unit, loss as a percent, a dash while unknown.
    pub value: String,
    pub hue: Hue,
}

/// What a chip shows for a value that isn't known yet.
pub const UNKNOWN: &str = "\u{2013}";

/// The chips of one hop's figures: p50 green, p95 blue, p99 magenta, loss red.
pub fn chips(stats: &HopStats) -> [Chip; 4] {
    let ms = |value: Option<std::time::Duration>| {
        value.map_or_else(|| UNKNOWN.to_string(), format::ms_bare)
    };
    let loss = if stats.sent == 0 && stats.reported_loss.is_none() {
        UNKNOWN.to_string()
    } else {
        format::percent(stats.loss_fraction())
    };
    [
        Chip {
            name: "p50",
            value: ms(stats.p50),
            hue: Hue::Green,
        },
        Chip {
            name: "p95",
            value: ms(stats.p95),
            hue: Hue::Blue,
        },
        Chip {
            name: "p99",
            value: ms(stats.p99),
            hue: Hue::Magenta,
        },
        Chip {
            name: "loss",
            value: loss,
            hue: Hue::Red,
        },
    ]
}

/// The chips as one line of plain output: "● p50 10.0  ● p95 10.4  ● p99 10.4  ● loss 0.0%",
/// each dot in its figure's color and the name dim. Without color the dots mean nothing, so
/// they are left out: "p50 10.0  p95 10.4  p99 10.4  loss 0.0%".
pub fn line(stats: &HopStats, style: Style) -> String {
    chips(stats)
        .iter()
        .map(|chip| {
            let words = format!("{} {}", style.dim(chip.name), chip.value);
            if style.color {
                let dot = match chip.hue {
                    Hue::Green => style.green("\u{25CF}"),
                    Hue::Blue => style.blue("\u{25CF}"),
                    Hue::Magenta => style.magenta("\u{25CF}"),
                    Hue::Red => style.red("\u{25CF}"),
                };
                format!("{dot} {words}")
            } else {
                words
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

#[cfg(test)]
mod tests {
    use rvrs_core::model::{HopStats, Slot};

    use super::*;

    fn stats(times: &[Option<f64>]) -> HopStats {
        let probes: Vec<Slot> = times
            .iter()
            .map(|time| match time {
                Some(ms) => Slot::Reply {
                    from: "1.1.1.1".parse().unwrap(),
                    micros: (ms * 1000.0).round() as u64,
                    unreachable: None,
                    ttl: None,
                },
                None => Slot::Lost,
            })
            .collect();
        HopStats::of(&probes)
    }

    #[test]
    fn four_chips_in_the_apps_order_and_colors() {
        // 100 replies from 1 to 100 ms: nearest rank puts p50 at 50, p95 at 95 and p99 at 99.
        let times: Vec<Option<f64>> = (1..=100).map(|ms| Some(f64::from(ms))).collect();
        let chips = chips(&stats(&times));
        let names: Vec<&str> = chips.iter().map(|chip| chip.name).collect();
        assert_eq!(names, ["p50", "p95", "p99", "loss"]);
        let values: Vec<&str> = chips.iter().map(|chip| chip.value.as_str()).collect();
        assert_eq!(values, ["50.0", "95.0", "99.0", "0.0%"]);
        let hues: Vec<Hue> = chips.iter().map(|chip| chip.hue).collect();
        assert_eq!(hues, [Hue::Green, Hue::Blue, Hue::Magenta, Hue::Red]);
    }

    #[test]
    fn an_unknown_value_is_a_dash_and_its_chip_stays() {
        assert_eq!(
            line(&stats(&[]), Style::PLAIN),
            "p50 \u{2013}  p95 \u{2013}  p99 \u{2013}  loss \u{2013}"
        );
        // A silent target has no percentiles, and all its loss.
        assert_eq!(
            line(&stats(&[None, None]), Style::PLAIN),
            "p50 \u{2013}  p95 \u{2013}  p99 \u{2013}  loss 100.0%"
        );
    }

    #[test]
    fn dots_only_with_color() {
        let figures = stats(&[Some(10.0), Some(10.4), None, Some(10.0)]);
        assert_eq!(
            line(&figures, Style::PLAIN),
            "p50 10.0  p95 10.4  p99 10.4  loss 25.0%"
        );
        assert_eq!(
            line(&figures, Style { color: true }),
            "\x1b[32m\u{25CF}\x1b[0m \x1b[2mp50\x1b[0m 10.0  \
             \x1b[34m\u{25CF}\x1b[0m \x1b[2mp95\x1b[0m 10.4  \
             \x1b[35m\u{25CF}\x1b[0m \x1b[2mp99\x1b[0m 10.4  \
             \x1b[31m\u{25CF}\x1b[0m \x1b[2mloss\x1b[0m 25.0%"
        );
    }
}
