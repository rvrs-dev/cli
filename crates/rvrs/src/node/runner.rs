//! One run a node was asked for, made with the same engines a run on this device uses.
//!
//! The request was checked before this is called (`rvrs_core::node::request::check`), and the
//! address it probes was resolved and checked too. So an engine is handed that address and never
//! the name, and the run ends on its own: at its deadline, whatever it has measured by then.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rvrs_core::bgp::Target;
use rvrs_core::dns;
use rvrs_core::model::{Outcome, Run, Tool};
use rvrs_core::node::request::{Admitted, Plan};
use rvrs_core::session::{Lookups, Session};
use rvrs_core::FamilyChoice;

/// How long names and networks may still arrive once a run has ended.
const LOOKUP_BUDGET: Duration = Duration::from_millis(1500);
/// How long an engine gets to end after it was told to stop.
const STOP_BUDGET: Duration = Duration::from_secs(5);

/// A run that ended before anything was sent, with the sentence that says why.
pub fn failed(tool: Tool, target: &str, why: &str) -> Run {
    let mut run = Run::new(tool, target, FamilyChoice::Auto, 0, 0);
    run.outcome = Some(Outcome::Failed(why.to_string()));
    run
}

fn lookups(wanted: bool) -> Lookups {
    if wanted {
        Lookups::ALL
    } else {
        Lookups::NONE
    }
}

/// Makes the run and returns it once it has ended. `destination` is the address the node's
/// guard resolved and checked, for a tool that probes. `stop` ends the run early, with what it
/// measured until then.
pub fn make(admitted: &Admitted, destination: Option<IpAddr>, stop: &Arc<AtomicBool>) -> Run {
    let started = Instant::now();
    let address = destination.map(|address| address.to_string());
    let host = address.as_deref().unwrap_or(&admitted.target);
    let mut wants_lookups = false;
    let mut wants_mtu = false;
    let mut session = match &admitted.plan {
        Plan::Traceroute {
            family,
            options,
            tcp_port,
            mtu,
            lookups: wanted,
        } if cfg!(feature = "traceroute") => {
            wants_lookups = *wanted;
            wants_mtu = *mtu && cfg!(feature = "mtu");
            match tcp_port.filter(|_| cfg!(feature = "tcp")) {
                Some(port) => {
                    Session::start_tcp(host, *family, options.clone(), port, lookups(*wanted))
                }
                None => Session::start(host, *family, options.clone(), lookups(*wanted)),
            }
        }
        Plan::Ping {
            family,
            options,
            lookups: wanted,
        } if cfg!(feature = "ping") => {
            wants_lookups = *wanted;
            Session::start_ping(host, *family, options.clone(), lookups(*wanted))
        }
        Plan::Mtr {
            family,
            options,
            lookups: wanted,
        } if cfg!(feature = "mtr") => {
            wants_lookups = *wanted;
            Session::start_mtr(host, *family, options.clone(), lookups(*wanted))
        }
        Plan::Dns {
            record_type,
            resolver,
        } if cfg!(feature = "dns") => {
            let mut options = dns::Options::default();
            if let Some(resolver) = resolver {
                options.own_resolvers = vec![dns::Resolver {
                    label: resolver.to_string(),
                    address: *resolver,
                }];
            }
            if let Some(code) = record_type.as_deref().and_then(dns::type_code) {
                options.types = vec![code];
            }
            Session::start_dns(&admitted.target, options, Lookups::NONE)
        }
        Plan::Bgp if cfg!(feature = "bgp") => match Target::parse(&admitted.target) {
            // A node asks RIPEstat and no looking glass: those are a person's own.
            Some(target) => Session::start_bgp(&target, Vec::new(), Lookups::NONE),
            None => {
                return failed(
                    admitted.tool,
                    &admitted.target,
                    "The target isn't a hostname, an address, a prefix or an AS number.",
                )
            }
        },
        // The check refuses a tool the build lacks, so this is never reached.
        _ => {
            return failed(
                admitted.tool,
                &admitted.target,
                "This build lacks the tool.",
            )
        }
    };

    let mut told_to_stop: Option<Instant> = None;
    loop {
        session.pump(Duration::from_millis(100));
        if !session.run.is_running() {
            break;
        }
        match told_to_stop {
            None if stop.load(Ordering::Relaxed) || started.elapsed() >= admitted.deadline => {
                session.stop();
                told_to_stop = Some(Instant::now());
            }
            Some(since) if since.elapsed() > STOP_BUDGET => {
                // The engine didn't end. The run is over all the same, with what it has.
                session.run.outcome = Some(Outcome::Stopped);
                break;
            }
            _ => {}
        }
    }
    session.finish_lookups(if wants_lookups {
        LOOKUP_BUDGET
    } else {
        Duration::ZERO
    });
    if wants_mtu && !stop.load(Ordering::Relaxed) {
        if let Some(destination) = session.run.destination {
            if let Ok(found) = rvrs_core::mtu::discover(destination, Duration::from_secs(1), stop) {
                session.run.mtu = Some(found);
            }
        }
    }
    session.run.clone()
}
