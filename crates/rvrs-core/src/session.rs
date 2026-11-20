//! One run from start to end: resolve the target, drive the engine, fold its events into the
//! [`Run`], and look up each host's name and network as it first answers. The CLI, the Console
//! and later the node all use this, so a run behaves the same wherever it is shown.

use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::asn::{self, Cache, Lookup};
use crate::bgp::{self, BgpEvent};
use crate::dns::{self, DnsEvent};
use crate::engine::{self, Event, MtrOptions, Options, PingOptions};
use crate::model::{Network, Run, Tool};
use crate::resolve;
use crate::FamilyChoice;

/// Which lookups a run makes for the hosts that answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lookups {
    /// Reverse DNS through the system resolver.
    pub names: bool,
    /// Networks through RIPEstat. Sends each public hop address to stat.ripe.net.
    pub networks: bool,
}

impl Lookups {
    pub const ALL: Lookups = Lookups {
        names: true,
        networks: true,
    };
    pub const NONE: Lookups = Lookups {
        names: false,
        networks: false,
    };
}

enum Message {
    Resolved(IpAddr, Option<IpAddr>),
    Engine(Event),
    Name(IpAddr, Option<String>),
    Network(IpAddr, Option<Network>, bool),
    /// What the internet sees this device as. Counted as pending until it arrives.
    Public(Option<IpAddr>),
    /// A DNS lookup has no destination to resolve first. This is the address its questions
    /// leave from.
    Origin(Option<IpAddr>),
    Dns(DnsEvent),
    Bgp(BgpEvent),
    /// The same network again with its location, once that has arrived. Not counted as pending.
    Located(IpAddr, Network),
}

pub struct Session {
    pub run: Run,
    receiver: Receiver<Message>,
    sender: Sender<Message>,
    /// Addresses waiting for their network. A few workers take them in turn, each over one kept
    /// connection, so a path of many hops never opens a burst of requests.
    network_queue: Option<Sender<IpAddr>>,
    stop: Arc<AtomicBool>,
    lookups: Lookups,
    cache: Arc<Cache>,
    pending_lookups: usize,
}

impl Session {
    /// Starts a traceroute.
    pub fn start(target: &str, family: FamilyChoice, options: Options, lookups: Lookups) -> Self {
        let options = options.clamped();
        let run = Run::new(
            Tool::Traceroute,
            target,
            family,
            options.queries,
            options.max_ttl,
        );
        Self::launch(run, lookups, move |destination, events, stop| {
            engine::run(destination, options, events, stop)
        })
    }

    /// Starts a ping.
    pub fn start_ping(
        target: &str,
        family: FamilyChoice,
        options: PingOptions,
        lookups: Lookups,
    ) -> Self {
        let run = Run::new(Tool::Ping, target, family, 1, options.ttl);
        Self::launch(run, lookups, move |destination, events, stop| {
            engine::ping(destination, options, events, stop)
        })
    }

    /// Starts an MTR run.
    pub fn start_mtr(
        target: &str,
        family: FamilyChoice,
        options: MtrOptions,
        lookups: Lookups,
    ) -> Self {
        let run = Run::new(Tool::Mtr, target, family, 0, options.max_ttl);
        Self::launch(run, lookups, move |destination, events, stop| {
            engine::mtr(destination, options, events, stop)
        })
    }

    /// Starts a DNS lookup. There is no destination to resolve first: every server is asked
    /// by its address.
    pub fn start_dns(target: &str, options: dns::Options, lookups: Lookups) -> Self {
        let name = dns::lookup::normalize(target);
        let mut run = Run::new(Tool::Dns, &name, FamilyChoice::Auto, 0, 0);
        run.dns = Some(dns::DnsResult::new(&name));
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let lookup_sender = sender.clone();
        let lookup_stop = Arc::clone(&stop);
        thread::spawn(move || {
            // The questions leave toward the device's resolver, or a public one without it.
            let toward = options
                .device_resolvers
                .clone()
                .unwrap_or_else(dns::lookup::device_resolvers)
                .first()
                .copied()
                .or_else(|| {
                    options
                        .public_resolvers
                        .first()
                        .map(|resolver| resolver.address)
                });
            let _ = lookup_sender.send(Message::Origin(toward.and_then(resolve::source_toward)));
            let (events, lookup_events) = mpsc::channel();
            let forward = lookup_sender.clone();
            let forwarder = thread::spawn(move || {
                for event in lookup_events {
                    if forward.send(Message::Dns(event)).is_err() {
                        break;
                    }
                }
            });
            dns::lookup::run(&name, &options, &events, &lookup_stop);
            drop(events);
            let _ = forwarder.join();
        });

        Self {
            run,
            receiver,
            sender,
            network_queue: None,
            stop,
            lookups,
            cache: Arc::new(Cache::open()),
            pending_lookups: 0,
        }
    }

    /// Starts a BGP lookup. Nothing is probed, so there is no destination to resolve first.
    /// With lookups on, the network this device is in is asked for too, so the result can
    /// mark it among the paths.
    pub fn start_bgp(
        target: &bgp::Target,
        glasses: Vec<bgp::LookingGlass>,
        lookups: Lookups,
    ) -> Self {
        let mut run = Run::new(Tool::Bgp, &target.canonical, FamilyChoice::Auto, 0, 0);
        run.bgp = Some(bgp::BgpResult::new(target.kind, &target.canonical));
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let lookup_sender = sender.clone();
        let lookup_stop = Arc::clone(&stop);
        let target = target.clone();
        thread::spawn(move || {
            let (events, lookup_events) = mpsc::channel();
            let forward = lookup_sender.clone();
            let watched = Arc::clone(&lookup_stop);
            // A data call can take many seconds to give up. A stop ends the run at once, and
            // whatever still answers after that has nobody to tell.
            let forwarder = thread::spawn(move || loop {
                match lookup_events.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => {
                        if forward.send(Message::Bgp(event)).is_err() {
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if watched.load(Ordering::Relaxed) {
                            let _ = forward.send(Message::Bgp(BgpEvent::Stopped));
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            });
            bgp::lookup::run(&target, &glasses, &events, &lookup_stop);
            drop(events);
            let _ = forwarder.join();
        });

        let mut pending_lookups = 0;
        if lookups.networks {
            pending_lookups = 2;
            let sender = sender.clone();
            thread::spawn(move || {
                let agent = asn::agent();
                let address = asn::public_address(&agent);
                let _ = sender.send(Message::Public(address));
                let found = match address.map(|address| (address, asn::announced(&agent, address)))
                {
                    Some((address, Lookup::Found(network))) => {
                        Message::Network(address, Some(network), true)
                    }
                    Some((address, _)) => Message::Network(address, None, false),
                    None => Message::Network(IpAddr::from([0, 0, 0, 0]), None, false),
                };
                let _ = sender.send(found);
            });
        }

        Self {
            run,
            receiver,
            sender,
            network_queue: None,
            stop,
            lookups,
            cache: Arc::new(Cache::open()),
            pending_lookups,
        }
    }

    /// Resolves the target on a thread of its own, then hands the destination to the engine
    /// and forwards what it reports.
    fn launch<Engine>(run: Run, lookups: Lookups, engine: Engine) -> Self
    where
        Engine: FnOnce(IpAddr, &Sender<Event>, &Arc<AtomicBool>) + Send + 'static,
    {
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        let engine_sender = sender.clone();
        let engine_stop = Arc::clone(&stop);
        let host = run.target.clone();
        let family = run.family;
        thread::spawn(move || {
            let destination = match resolve::resolve(&host, family) {
                Ok(address) => address,
                Err(error) => {
                    let _ = engine_sender.send(Message::Engine(Event::Failed(error.to_string())));
                    return;
                }
            };
            let _ = engine_sender.send(Message::Resolved(
                destination,
                resolve::source_toward(destination),
            ));
            let (events, engine_events) = mpsc::channel();
            let forward = engine_sender.clone();
            let forwarder = thread::spawn(move || {
                for event in engine_events {
                    if forward.send(Message::Engine(event)).is_err() {
                        break;
                    }
                }
            });
            engine(destination, &events, &engine_stop);
            drop(events);
            let _ = forwarder.join();
        });

        Self {
            run,
            receiver,
            sender,
            stop,
            lookups,
            cache: Arc::new(Cache::open()),
            pending_lookups: 0,
            network_queue: None,
        }
    }

    /// Asks the run to stop. It ends with the outcome "stopped" on a following `pump`.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// True while probes are out or lookups are still due.
    pub fn is_settling(&self) -> bool {
        self.run.is_running() || self.pending_lookups > 0
    }

    pub fn pending_lookups(&self) -> usize {
        self.pending_lookups
    }

    /// Waits up to `timeout` for news and folds everything that is ready into the run.
    /// Returns true when the run changed.
    pub fn pump(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut changed = false;
        loop {
            let message = if changed {
                match self.receiver.try_recv() {
                    Ok(message) => message,
                    Err(_) => return true,
                }
            } else {
                match self
                    .receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                {
                    Ok(message) => message,
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                        return false
                    }
                }
            };
            changed = true;
            self.handle(message);
        }
    }

    /// After the run has ended: gives outstanding lookups up to `budget` to land, then saves
    /// what was learned to the lookup cache. A result never waits on lookups longer than this.
    pub fn finish_lookups(&mut self, budget: Duration) {
        let deadline = Instant::now() + budget;
        while self.pending_lookups > 0 && Instant::now() < deadline {
            self.pump(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(50)),
            );
        }
        self.cache.flush();
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Resolved(destination, source) => {
                self.run.destination = Some(destination);
                self.run.source = source;
                // The public side of the origin, asked once the run is under way. It is a
                // lookup like the others, so the same switch turns it off.
                if self.lookups.networks {
                    self.pending_lookups += 1;
                    let sender = self.sender.clone();
                    thread::spawn(move || {
                        let _ = sender.send(Message::Public(asn::public_address(&asn::agent())));
                    });
                }
            }
            Message::Public(address) => {
                self.pending_lookups = self.pending_lookups.saturating_sub(1);
                self.run.public_source = address;
            }
            Message::Origin(source) => {
                self.run.source = source;
                if self.lookups.networks {
                    self.pending_lookups += 1;
                    let sender = self.sender.clone();
                    thread::spawn(move || {
                        let _ = sender.send(Message::Public(asn::public_address(&asn::agent())));
                    });
                }
            }
            Message::Dns(event) => self.run.apply_dns(&event),
            Message::Bgp(event) => self.run.apply_bgp(&event),
            Message::Engine(event) => {
                if let Some(address) = self.run.apply(&event) {
                    self.look_up(address);
                }
            }
            Message::Name(address, name) => {
                self.pending_lookups = self.pending_lookups.saturating_sub(1);
                if let Some(name) = name {
                    self.run.names.insert(address, name);
                }
            }
            Message::Network(address, network, answered) => {
                self.pending_lookups = self.pending_lookups.saturating_sub(1);
                if answered {
                    // A network waits for its location before it is cached, so the cache
                    // never keeps half an answer for a week.
                    if network.is_none() {
                        self.cache.store(address, None);
                    }
                    self.run.networks.insert(address, network);
                }
            }
            Message::Located(address, network) => {
                self.cache.store(address, Some(network.clone()));
                self.run.networks.insert(address, Some(network));
            }
        }
    }

    fn look_up(&mut self, address: IpAddr) {
        if self.lookups.names {
            self.pending_lookups += 1;
            let sender = self.sender.clone();
            thread::spawn(move || {
                let _ = sender.send(Message::Name(address, resolve::reverse(address)));
            });
        }
        if self.lookups.networks && !crate::is_private(address) {
            if let Some(cached) = self.cache.get(address) {
                self.run.networks.insert(address, cached);
                return;
            }
            self.pending_lookups += 1;
            let queue = self.network_queue.get_or_insert_with(|| {
                let (queue, addresses) = mpsc::channel::<IpAddr>();
                let addresses = Arc::new(std::sync::Mutex::new(addresses));
                for _ in 0..4 {
                    let addresses = Arc::clone(&addresses);
                    let sender = self.sender.clone();
                    thread::spawn(move || {
                        let agent = asn::agent();
                        loop {
                            let next = match addresses.lock() {
                                Ok(addresses) => addresses.recv(),
                                Err(_) => return,
                            };
                            let Ok(address) = next else { return };
                            let found = asn::announced(&agent, address);
                            let message = match &found {
                                Lookup::Found(network) => {
                                    Message::Network(address, Some(network.clone()), true)
                                }
                                Lookup::Unknown => Message::Network(address, None, true),
                                Lookup::Failed => Message::Network(address, None, false),
                            };
                            if sender.send(message).is_err() {
                                return;
                            }
                            if let Lookup::Found(mut network) = found {
                                asn::locate(&agent, address, &mut network);
                                if sender.send(Message::Located(address, network)).is_err() {
                                    return;
                                }
                            }
                        }
                    });
                }
                queue
            });
            let _ = queue.send(address);
        }
    }
}
