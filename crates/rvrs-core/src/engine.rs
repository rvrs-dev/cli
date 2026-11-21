//! Parallel ICMP probing. Each round sends one probe to every hop; rounds go out back to back.
//! Replies are matched by sequence number, so wire order doesn't matter.
//!
//! The original traceroute waits for each probe before sending the next, which makes a silent
//! hop cost three full waits. Here every probe is on the wire within a fraction of a second, a
//! silent hop costs one wait in total, and once the destination has answered the remaining
//! probes are given a wait scaled to the path (ten times the slowest reply, at least half a
//! second), the way Linux traceroute's adaptive `-w` does.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::packet::{self, Parsed};
use crate::socket::IcmpSocket;
use crate::Family;

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// First TTL probed (`-f`).
    pub first_ttl: u8,
    /// Highest TTL probed (`-m`).
    pub max_ttl: u8,
    /// Probes per hop (`-q`), which is also the number of rounds.
    pub queries: u8,
    /// Longest a probe may take to answer (`-w`).
    pub wait: Duration,
    /// Pause between probes (`-z`), so routers that rate-limit their replies keep answering.
    pub spacing: Duration,
    /// Echo payload in bytes.
    pub payload: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            first_ttl: 1,
            max_ttl: 30,
            queries: 3,
            wait: Duration::from_secs(5),
            spacing: Duration::from_millis(5),
            payload: 32,
        }
    }
}

impl Options {
    pub fn clamped(mut self) -> Self {
        self.max_ttl = self.max_ttl.max(1);
        self.first_ttl = self.first_ttl.clamp(1, self.max_ttl);
        self.queries = self.queries.clamp(1, 10);
        self.payload = self.payload.min(1400);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKind {
    /// A router on the path: time exceeded.
    Intermediate,
    /// The destination itself: echo reply.
    Destination,
    /// A router reporting that the destination can't be reached, with the ICMP code.
    Unreachable(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The destination answered at this TTL.
    Reached(u8),
    /// Every probe was sent and the destination never answered.
    Exhausted,
    /// Stopped by the person.
    Stopped,
}

#[derive(Debug, Clone)]
pub enum Event {
    Sent {
        ttl: u8,
        round: u8,
    },
    Reply {
        ttl: u8,
        round: u8,
        from: IpAddr,
        rtt: Duration,
        kind: ReplyKind,
    },
    Lost {
        ttl: u8,
        round: u8,
    },
    Finished(Ending),
    Failed(String),
    /// TCP: what the destination did with the first connect, and how long its answer took.
    Port {
        state: crate::tcp::PortState,
        connect: Option<Duration>,
        /// The maximum segment size the handshake settled on.
        mss: Option<u16>,
    },
    /// Ping: packet number `seq` (from 0) is on the wire.
    PingSent {
        seq: u32,
    },
    PingReply {
        seq: u32,
        from: IpAddr,
        rtt: Duration,
        /// The TTL the reply arrived with.
        ttl: Option<u8>,
    },
    /// A router answered instead of the target: time exceeded, or unreachable with its code.
    PingRefused {
        seq: u32,
        from: IpAddr,
        time_exceeded: bool,
        code: u8,
    },
    PingLost {
        seq: u32,
    },
    /// MTR: a probe of `round` (from 0) to this hop is on the wire. Rounds have no upper bound.
    MtrSent {
        ttl: u8,
        round: u32,
    },
    MtrReply {
        ttl: u8,
        round: u32,
        from: IpAddr,
        rtt: Duration,
        kind: ReplyKind,
    },
    MtrLost {
        ttl: u8,
        round: u32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MtrOptions {
    /// First TTL probed (`-f`).
    pub first_ttl: u8,
    /// Highest TTL probed (`-m`).
    pub max_ttl: u8,
    /// Rounds to send (`-c`). None runs until stopped.
    pub count: Option<u32>,
    /// Pause between rounds (`-i`). Routers rate-limit their replies at anything much faster.
    pub interval: Duration,
    /// How long a probe may take to answer before it counts as a loss (`-Z`).
    pub wait: Duration,
    /// Pause between the probes of one round.
    pub spacing: Duration,
    /// Echo payload in bytes (`-s` minus the headers).
    pub payload: usize,
    /// How many silent hops in a row are probed past the farthest one that answered (`-U`).
    pub max_unknown: u8,
}

impl Default for MtrOptions {
    fn default() -> Self {
        Self {
            first_ttl: 1,
            max_ttl: 30,
            count: None,
            interval: Duration::from_secs(1),
            wait: Duration::from_secs(2),
            spacing: Duration::from_millis(5),
            payload: 36,
            max_unknown: 5,
        }
    }
}

/// Runs MTR against `destination`: one probe to every hop per round, a round per interval,
/// until the count is reached or `stop` is set. Blocks; run it on its own thread.
pub fn mtr(
    destination: IpAddr,
    options: MtrOptions,
    events: &Sender<Event>,
    stop: &Arc<AtomicBool>,
) {
    if let Err(message) = execute_mtr(destination, &options, events, stop) {
        let _ = events.send(Event::Failed(message));
    }
}

fn execute_mtr(
    destination: IpAddr,
    options: &MtrOptions,
    events: &Sender<Event>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    let family = Family::of(destination);
    let socket = IcmpSocket::open(family).map_err(|error| socket_error(&error))?;
    let identifier = random_identifier();
    let payload = vec![0x72u8; options.payload.min(1400)];
    let max_ttl = options.max_ttl.max(1);
    let first_ttl = options.first_ttl.clamp(1, max_ttl);

    // Outstanding probes by echo sequence. The sequence wraps on a long run, and by then the
    // probe that used a number before has long settled.
    let mut open: HashMap<u16, (u8, u32, Instant)> = HashMap::new();
    let mut next_sequence: u16 = 1;
    let mut destination_ttl: Option<u8> = None;
    // The farthest hop that ever answered. While the destination is unknown, later rounds stop
    // a few hops past it, so a silent target isn't sprayed with thirty probes a second.
    let mut farthest: u8 = 0;

    let mut round: u32 = 0;
    let mut ttl = first_ttl;
    let mut in_round = true;
    let mut next_round = Instant::now();
    let mut next_send = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = events.send(Event::Finished(Ending::Stopped));
            return Ok(());
        }
        let now = Instant::now();
        let more_rounds = options.count.is_none_or(|count| round < count);

        if !in_round && more_rounds && now >= next_round {
            in_round = true;
            ttl = first_ttl;
            next_send = now;
        }
        if in_round && more_rounds && now >= next_send {
            // The first round goes all the way, to find the path at once.
            let limit = match destination_ttl {
                Some(limit) => limit,
                None if round == 0 => max_ttl,
                None => max_ttl.min(farthest.saturating_add(options.max_unknown).max(first_ttl)),
            };
            if ttl > limit {
                in_round = false;
                round += 1;
                next_round += options.interval;
                if next_round < now {
                    next_round = now + options.interval;
                }
            } else {
                let sequence = next_sequence;
                next_sequence = next_sequence.wrapping_add(1).max(1);
                let packet = packet::echo_request(family, identifier, sequence, &payload);
                open.insert(sequence, (ttl, round, Instant::now()));
                socket
                    .send(&packet, destination, ttl)
                    .map_err(|error| format!("Couldn't send a probe ({error})."))?;
                let _ = events.send(Event::MtrSent { ttl, round });
                next_send = Instant::now() + options.spacing;
                if ttl == u8::MAX {
                    in_round = false;
                    round += 1;
                    next_round += options.interval;
                } else {
                    ttl += 1;
                }
            }
            continue;
        }

        open.retain(|_, (probe_ttl, probe_round, sent_at)| {
            if destination_ttl.is_some_and(|limit| *probe_ttl > limit) {
                return false;
            }
            let expired = now >= *sent_at + options.wait;
            if expired {
                let _ = events.send(Event::MtrLost {
                    ttl: *probe_ttl,
                    round: *probe_round,
                });
            }
            !expired
        });
        if !more_rounds && open.is_empty() {
            let ending = destination_ttl.map_or(Ending::Exhausted, Ending::Reached);
            let _ = events.send(Event::Finished(ending));
            return Ok(());
        }

        let until = if in_round && more_rounds {
            next_send.saturating_duration_since(now)
        } else if more_rounds {
            next_round.saturating_duration_since(now)
        } else {
            Duration::from_millis(20)
        };
        let pause = until.clamp(Duration::from_millis(1), Duration::from_millis(20));
        let Some(datagram) = socket
            .receive(pause)
            .map_err(|error| format!("Couldn't read from the ICMP socket ({error})."))?
        else {
            continue;
        };
        let (reply_identifier, sequence, kind) = match datagram.parsed {
            Parsed::EchoReply {
                identifier,
                sequence,
            } => (identifier, sequence, ReplyKind::Destination),
            Parsed::TimeExceeded {
                identifier,
                sequence,
            } => (identifier, sequence, ReplyKind::Intermediate),
            Parsed::Unreachable {
                code,
                identifier,
                sequence,
            } => (identifier, sequence, ReplyKind::Unreachable(code)),
            Parsed::Other { .. } => continue,
        };
        if !socket.rewrites_identifier() && reply_identifier != identifier {
            continue;
        }
        let Some((probe_ttl, probe_round, sent_at)) = open.remove(&sequence) else {
            continue;
        };
        if kind != ReplyKind::Intermediate {
            destination_ttl = Some(destination_ttl.map_or(probe_ttl, |limit| limit.min(probe_ttl)));
        }
        if destination_ttl.is_some_and(|limit| probe_ttl > limit) {
            continue;
        }
        farthest = farthest.max(probe_ttl);
        let _ = events.send(Event::MtrReply {
            ttl: probe_ttl,
            round: probe_round,
            from: datagram.source,
            rtt: datagram.received_at.saturating_duration_since(sent_at),
            kind,
        });
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PingOptions {
    /// Packets to send (`-c`). None runs until stopped.
    pub count: Option<u32>,
    /// Pause between packets (`-i`).
    pub interval: Duration,
    /// How long a packet may take to answer before it counts as a loss (`-W`).
    pub wait: Duration,
    /// Echo payload in bytes (`-s`).
    pub payload: usize,
    /// TTL of the packets sent (`-t`).
    pub ttl: u8,
    /// Stop after this long, whatever was sent (`-w`).
    pub deadline: Option<Duration>,
}

impl Default for PingOptions {
    fn default() -> Self {
        Self {
            count: None,
            interval: Duration::from_secs(1),
            wait: Duration::from_secs(2),
            payload: 56,
            ttl: 64,
            deadline: None,
        }
    }
}

/// Pings `destination` and reports through `events` until the count is reached, the deadline
/// passes or `stop` is set. Blocks; run it on its own thread.
pub fn ping(
    destination: IpAddr,
    options: PingOptions,
    events: &Sender<Event>,
    stop: &Arc<AtomicBool>,
) {
    if let Err(message) = execute_ping(destination, &options, events, stop) {
        let _ = events.send(Event::Failed(message));
    }
}

fn execute_ping(
    destination: IpAddr,
    options: &PingOptions,
    events: &Sender<Event>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    let family = Family::of(destination);
    let socket = IcmpSocket::open(family).map_err(|error| socket_error(&error))?;
    let identifier = random_identifier();
    let payload = vec![0x72u8; options.payload.min(65_000)];
    let started = Instant::now();

    // Outstanding packets by echo sequence. The 16-bit sequence wraps on a long run, and by
    // then the packet that used a number before has long settled.
    let mut open: HashMap<u16, (u32, Instant)> = HashMap::new();
    let mut seq: u32 = 0;
    let mut replies: u32 = 0;
    let mut next_send = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = events.send(Event::Finished(Ending::Stopped));
            return Ok(());
        }
        let now = Instant::now();
        let past_deadline = options
            .deadline
            .is_some_and(|deadline| now >= started + deadline);
        let sending = !past_deadline && options.count.is_none_or(|count| seq < count);

        if sending && now >= next_send {
            let sequence = (seq % 65_536) as u16;
            let packet = packet::echo_request(family, identifier, sequence, &payload);
            open.insert(sequence, (seq, Instant::now()));
            socket
                .send(&packet, destination, options.ttl)
                .map_err(|error| format!("Couldn't send a packet ({error})."))?;
            let _ = events.send(Event::PingSent { seq });
            seq += 1;
            next_send += options.interval;
            if next_send < now {
                next_send = now + options.interval;
            }
            continue;
        }

        open.retain(|_, (number, sent_at)| {
            let expired = now >= *sent_at + options.wait;
            if expired {
                let _ = events.send(Event::PingLost { seq: *number });
            }
            !expired
        });
        if !sending && (open.is_empty() || past_deadline) {
            let ending = if replies > 0 {
                Ending::Reached(options.ttl)
            } else {
                Ending::Exhausted
            };
            let _ = events.send(Event::Finished(ending));
            return Ok(());
        }

        let until_send = if sending {
            next_send.saturating_duration_since(now)
        } else {
            Duration::from_millis(20)
        };
        let pause = until_send.clamp(Duration::from_millis(1), Duration::from_millis(20));
        let Some(datagram) = socket
            .receive(pause)
            .map_err(|error| format!("Couldn't read from the ICMP socket ({error})."))?
        else {
            continue;
        };
        let (reply_identifier, sequence) = match datagram.parsed {
            Parsed::EchoReply {
                identifier,
                sequence,
            }
            | Parsed::TimeExceeded {
                identifier,
                sequence,
            }
            | Parsed::Unreachable {
                identifier,
                sequence,
                ..
            } => (identifier, sequence),
            Parsed::Other { .. } => continue,
        };
        if !socket.rewrites_identifier() && reply_identifier != identifier {
            continue;
        }
        let Some((number, sent_at)) = open.remove(&sequence) else {
            continue;
        };
        let from = datagram.source;
        let event = match datagram.parsed {
            Parsed::EchoReply { .. } => {
                replies += 1;
                Event::PingReply {
                    seq: number,
                    from,
                    rtt: datagram.received_at.saturating_duration_since(sent_at),
                    ttl: datagram.ttl,
                }
            }
            Parsed::TimeExceeded { .. } => Event::PingRefused {
                seq: number,
                from,
                time_exceeded: true,
                code: 0,
            },
            Parsed::Unreachable { code, .. } => Event::PingRefused {
                seq: number,
                from,
                time_exceeded: false,
                code,
            },
            Parsed::Other { .. } => continue,
        };
        let _ = events.send(event);
    }
}

struct Probe {
    ttl: u8,
    round: u8,
    sent_at: Instant,
    settled: bool,
}

/// Probes `destination` and reports through `events` until the run ends. Blocks; run it on its
/// own thread. Setting `stop` ends the run with [`Ending::Stopped`].
pub fn run(destination: IpAddr, options: Options, events: &Sender<Event>, stop: &Arc<AtomicBool>) {
    if let Err(message) = execute(destination, options.clamped(), events, stop) {
        let _ = events.send(Event::Failed(message));
    }
}

fn execute(
    destination: IpAddr,
    options: Options,
    events: &Sender<Event>,
    stop: &Arc<AtomicBool>,
) -> Result<(), String> {
    let family = Family::of(destination);
    let socket = IcmpSocket::open(family).map_err(|error| socket_error(&error))?;
    let identifier = random_identifier();
    let payload = vec![0x72u8; options.payload];

    let mut probes: HashMap<u16, Probe> = HashMap::new();
    let mut next_sequence: u16 = 1;
    let mut destination_ttl: Option<u8> = None;
    let mut slowest = Duration::ZERO;

    let mut round: u8 = 0;
    let mut ttl = options.first_ttl;
    let mut sending = true;
    let mut next_send = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = events.send(Event::Finished(Ending::Stopped));
            return Ok(());
        }
        let now = Instant::now();

        if sending && now >= next_send {
            let past_destination = destination_ttl.is_some_and(|limit| ttl > limit);
            if past_destination || ttl > options.max_ttl {
                round += 1;
                ttl = options.first_ttl;
                if round >= options.queries {
                    sending = false;
                }
            } else {
                let sequence = next_sequence;
                next_sequence = next_sequence.wrapping_add(1).max(1);
                let packet = packet::echo_request(family, identifier, sequence, &payload);
                probes.insert(
                    sequence,
                    Probe {
                        ttl,
                        round,
                        sent_at: Instant::now(),
                        settled: false,
                    },
                );
                socket
                    .send(&packet, destination, ttl)
                    .map_err(|error| format!("Couldn't send a probe ({error})."))?;
                let _ = events.send(Event::Sent { ttl, round });
                next_send = Instant::now() + options.spacing;
                if ttl == options.max_ttl {
                    round += 1;
                    ttl = options.first_ttl;
                    if round >= options.queries {
                        sending = false;
                    }
                } else {
                    ttl += 1;
                }
            }
            continue;
        }

        // Probes whose wait has run out are losses.
        let wait = match destination_ttl {
            Some(_) => options
                .wait
                .min((slowest * 10).max(Duration::from_millis(500))),
            None => options.wait,
        };
        let mut open = 0usize;
        for probe in probes.values_mut().filter(|probe| !probe.settled) {
            if destination_ttl.is_some_and(|limit| probe.ttl > limit) {
                probe.settled = true;
            } else if now >= probe.sent_at + wait {
                probe.settled = true;
                let _ = events.send(Event::Lost {
                    ttl: probe.ttl,
                    round: probe.round,
                });
            } else {
                open += 1;
            }
        }
        if !sending && open == 0 {
            let ending = destination_ttl.map_or(Ending::Exhausted, Ending::Reached);
            let _ = events.send(Event::Finished(ending));
            return Ok(());
        }

        let until_send = if sending {
            next_send.saturating_duration_since(now)
        } else {
            Duration::from_millis(20)
        };
        let pause = until_send.clamp(Duration::from_millis(1), Duration::from_millis(20));
        let Some(datagram) = socket
            .receive(pause)
            .map_err(|error| format!("Couldn't read from the ICMP socket ({error})."))?
        else {
            continue;
        };
        let (reply_identifier, sequence, kind) = match datagram.parsed {
            Parsed::EchoReply {
                identifier,
                sequence,
            } => (identifier, sequence, ReplyKind::Destination),
            Parsed::TimeExceeded {
                identifier,
                sequence,
            } => (identifier, sequence, ReplyKind::Intermediate),
            Parsed::Unreachable {
                code,
                identifier,
                sequence,
            } => (identifier, sequence, ReplyKind::Unreachable(code)),
            Parsed::Other { .. } => continue,
        };
        if !socket.rewrites_identifier() && reply_identifier != identifier {
            continue;
        }
        let Some(probe) = probes.get_mut(&sequence) else {
            continue;
        };
        if probe.settled {
            continue;
        }
        probe.settled = true;
        if kind != ReplyKind::Intermediate {
            destination_ttl = Some(destination_ttl.map_or(probe.ttl, |limit| limit.min(probe.ttl)));
        }
        if destination_ttl.is_some_and(|limit| probe.ttl > limit) {
            continue;
        }
        let rtt = datagram
            .received_at
            .saturating_duration_since(probe.sent_at);
        slowest = slowest.max(rtt);
        let _ = events.send(Event::Reply {
            ttl: probe.ttl,
            round: probe.round,
            from: datagram.source,
            rtt,
            kind,
        });
    }
}

/// The error a person sees when the ICMP socket can't be opened. On Linux that is almost always
/// the kernel setting that decides who may open one, so the message says how to change it.
fn socket_error(error: &std::io::Error) -> String {
    if cfg!(target_os = "linux")
        && matches!(error.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM))
    {
        return "Couldn't open an ICMP socket. This Linux system doesn't allow it for your user. \
                To allow it: sudo sysctl -w net.ipv4.ping_group_range=\"0 2147483647\""
            .to_string();
    }
    format!("Couldn't open an ICMP socket ({error}).")
}

fn random_identifier() -> u16 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let value = RandomState::new().build_hasher().finish() as u16;
    value.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_clamp_to_what_the_engine_can_do() {
        let options = Options {
            first_ttl: 9,
            max_ttl: 4,
            queries: 40,
            ..Options::default()
        }
        .clamped();
        assert_eq!(options.first_ttl, 4);
        assert_eq!(options.queries, 10);
        let options = Options {
            first_ttl: 0,
            max_ttl: 0,
            queries: 0,
            ..Options::default()
        }
        .clamped();
        assert_eq!(
            (options.first_ttl, options.max_ttl, options.queries),
            (1, 1, 1)
        );
    }
}
