//! How many runs a node makes at once and starts in a minute.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::refusal::Refusal;
use super::Limits;

const MINUTE: Duration = Duration::from_secs(60);

/// Counts the runs a node is making and the ones it started in the last minute. The clock is
/// one that only counts forward, so a wrong wall clock changes nothing.
#[derive(Debug, Clone)]
pub struct Gate {
    at_once: u32,
    per_minute: u32,
    running: u32,
    started: VecDeque<Instant>,
}

impl Gate {
    pub fn new(limits: &Limits) -> Gate {
        Gate {
            at_once: limits.runs_at_once,
            per_minute: limits.runs_per_minute,
            running: 0,
            started: VecDeque::new(),
        }
    }

    fn forget(&mut self, now: Instant) {
        while self
            .started
            .front()
            .is_some_and(|then| now.duration_since(*then) >= MINUTE)
        {
            self.started.pop_front();
        }
    }

    /// How many more runs the node starts now. It is the `room` of a claim, so the API hands out
    /// no more than the node will take.
    pub fn room(&mut self, now: Instant) -> u32 {
        self.forget(now);
        let free = self.at_once.saturating_sub(self.running);
        let left = self.per_minute.saturating_sub(self.started.len() as u32);
        free.min(left)
    }

    /// Counts a run as started, or refuses it when the node is at a limit. A claim that asks for
    /// `room` runs never gets here with more, so a refusal means the API handed out too many.
    pub fn start(&mut self, now: Instant) -> Result<(), Refusal> {
        self.forget(now);
        if self.running >= self.at_once {
            return Err(Refusal::AtOnce {
                limit: self.at_once,
            });
        }
        if self.started.len() as u32 >= self.per_minute {
            return Err(Refusal::PerMinute {
                limit: self.per_minute,
            });
        }
        self.running += 1;
        self.started.push_back(now);
        Ok(())
    }

    /// A run ended, however it ended.
    pub fn end(&mut self) {
        self.running = self.running.saturating_sub(1);
    }

    pub fn running(&self) -> u32 {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate(at_once: u32, per_minute: u32) -> Gate {
        Gate::new(&Limits {
            runs_at_once: at_once,
            runs_per_minute: per_minute,
            ..Limits::default()
        })
    }

    #[test]
    fn runs_at_once() {
        let now = Instant::now();
        let mut gate = gate(2, 12);
        assert_eq!(gate.room(now), 2);
        assert_eq!(gate.start(now), Ok(()));
        assert_eq!(gate.room(now), 1);
        assert_eq!(gate.start(now), Ok(()));
        assert_eq!(gate.room(now), 0);
        assert_eq!(gate.start(now), Err(Refusal::AtOnce { limit: 2 }));
        // The refused run was never counted.
        assert_eq!(gate.running(), 2);
        gate.end();
        assert_eq!(gate.room(now), 1);
        assert_eq!(gate.start(now), Ok(()));
    }

    #[test]
    fn runs_per_minute() {
        let start = Instant::now();
        let mut gate = gate(2, 3);
        for second in 0..3 {
            let now = start + Duration::from_secs(second);
            assert_eq!(gate.start(now), Ok(()));
            gate.end();
        }
        let now = start + Duration::from_secs(3);
        assert_eq!(gate.room(now), 0);
        assert_eq!(gate.start(now), Err(Refusal::PerMinute { limit: 3 }));
        // A minute after the first start, one more fits.
        let later = start + Duration::from_secs(60);
        assert_eq!(gate.room(later), 1);
        assert_eq!(gate.start(later), Ok(()));
        assert_eq!(gate.start(later), Err(Refusal::PerMinute { limit: 3 }));
    }

    #[test]
    fn a_node_that_allows_no_runs_starts_none() {
        let now = Instant::now();
        let mut gate = gate(0, 12);
        assert_eq!(gate.room(now), 0);
        assert_eq!(gate.start(now), Err(Refusal::AtOnce { limit: 0 }));
    }

    #[test]
    fn ending_more_runs_than_started_counts_none() {
        let mut gate = gate(1, 12);
        gate.end();
        assert_eq!(gate.running(), 0);
        assert_eq!(gate.room(Instant::now()), 1);
    }
}
