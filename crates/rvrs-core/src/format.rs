//! Numbers and times as rvrs writes them: latency in ms with one decimal, loss with one
//! decimal, ages in the shortest round unit.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn ms_value(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// "32.4 ms"
pub fn ms(duration: Duration) -> String {
    format!("{:.1} ms", ms_value(duration))
}

/// "32.4", for a column whose header carries the unit.
pub fn ms_bare(duration: Duration) -> String {
    format!("{:.1}", ms_value(duration))
}

/// "0.0%" from a fraction.
pub fn percent(fraction: f64) -> String {
    format!("{:.1}%", fraction * 100.0)
}

/// "5m", "2h", "3d": a TTL as the shortest round unit.
pub fn ttl(seconds: u32) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        3600..=86_399 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Seconds since 1970.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// "Just now", "5m ago", "3h ago", "5d ago".
pub fn relative(then: u64, now: u64) -> String {
    let age = now.saturating_sub(then);
    match age {
        0..=59 => "Just now".to_string(),
        60..=3599 => format!("{}m ago", age / 60),
        3600..=86_399 => format!("{}h ago", age / 3600),
        86_400..=604_799 => format!("{}d ago", age / 86_400),
        _ => format!("{}w ago", age / 604_800),
    }
}

/// "2026-11-15T09:30:00Z"
pub fn iso8601(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_has_one_decimal() {
        assert_eq!(ms(Duration::from_micros(12_449)), "12.4 ms");
        assert_eq!(ms_bare(Duration::from_micros(800)), "0.8");
        assert_eq!(percent(0.125), "12.5%");
    }

    #[test]
    fn ages_use_the_shortest_round_unit() {
        assert_eq!(relative(100, 130), "Just now");
        assert_eq!(relative(0, 5 * 60), "5m ago");
        assert_eq!(relative(0, 3 * 3600 + 5), "3h ago");
        assert_eq!(relative(0, 5 * 86_400), "5d ago");
        assert_eq!(relative(0, 21 * 86_400), "3w ago");
    }

    #[test]
    fn ttls_use_the_shortest_round_unit() {
        assert_eq!(ttl(45), "45s");
        assert_eq!(ttl(300), "5m");
        assert_eq!(ttl(7200), "2h");
        assert_eq!(ttl(86_400 * 3), "3d");
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(iso8601(1_794_735_000), "2026-11-15T09:30:00Z");
    }
}
