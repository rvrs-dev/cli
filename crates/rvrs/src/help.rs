//! Help text. Every line follows the rvrs voice: what the thing does, in the industry's words.

use crate::style::Style;

/// The version and commit `build.rs` derives from git: every commit is a new version.
pub const VERSION: &str = env!("RVRS_VERSION");
pub const COMMIT: &str = env!("RVRS_COMMIT");

/// `rvrs 0.14.2 (4ea5d4e)`, with what the build leaves out after it: `without console`.
pub fn version() -> String {
    match crate::features::version_note() {
        Some(note) => format!("rvrs {VERSION} ({COMMIT}) {note}"),
        None => format!("rvrs {VERSION} ({COMMIT})"),
    }
}

fn section(style: Style, title: &str, rows: &[(&str, &str)]) -> String {
    let marked: Vec<(&str, &str, bool)> = rows.iter().map(|(n, d)| (*n, *d, false)).collect();
    section_marked(style, title, &marked)
}

/// What a row carries after its description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    None,
    /// Planned, named here so people know what is coming, and refused with a sentence when typed.
    Soon,
    /// In rvrs, and left out of this build. Typed, it says how to add it.
    LeftOut,
}

/// `Tag::LeftOut` when this build leaves the build feature out.
fn tag_of(feature: &str) -> Tag {
    if crate::features::has(feature) {
        Tag::None
    } else {
        Tag::LeftOut
    }
}

/// Rows tagged Left out where the named build feature isn't in this build: `(row, feature)`.
fn tag_rows<'a>(
    rows: &[(&'a str, &'a str)],
    features: &[(&str, &str)],
) -> Vec<(&'a str, &'a str, Tag)> {
    rows.iter()
        .map(|(name, description)| {
            let tag = features
                .iter()
                .find(|(row, _)| row == name)
                .map_or(Tag::None, |(_, feature)| tag_of(feature));
            (*name, *description, tag)
        })
        .collect()
}

/// A section whose rows may be marked Soon.
fn section_marked(style: Style, title: &str, rows: &[(&str, &str, bool)]) -> String {
    let tagged: Vec<(&str, &str, Tag)> = rows
        .iter()
        .map(|(name, description, soon)| {
            (
                *name,
                *description,
                if *soon { Tag::Soon } else { Tag::None },
            )
        })
        .collect();
    section_tagged(style, title, &tagged)
}

fn section_tagged(style: Style, title: &str, rows: &[(&str, &str, Tag)]) -> String {
    let widest = rows
        .iter()
        .map(|(name, _, _)| name.len())
        .max()
        .unwrap_or(0);
    let mut out = format!("{}\n", style.bold(title));
    for (name, description, tag) in rows {
        let tag = match tag {
            Tag::None => String::new(),
            Tag::Soon => format!("  {}", style.yellow("Soon")),
            Tag::LeftOut => format!("  {}", style.yellow("Left out")),
        };
        if description.is_empty() {
            out.push_str(&format!("  {name}{tag}\n"));
        } else {
            out.push_str(&format!(
                "  {name:<widest$}   {}{tag}\n",
                style.dim(description)
            ));
        }
    }
    out
}

/// The --node line every tool's help carries.
const NODE_FLAG: (&str, &str, bool) = (
    "--node <name>",
    "Run from a node instead of this device",
    false,
);

pub fn main(style: Style) -> String {
    let mut out = format!(
        "{} rvrs {VERSION}\nSee what the network is doing.\n\n",
        style.bold("//")
    );
    // Usage shows what this build runs. Tools lists every tool and marks the ones it leaves out.
    let console = crate::features::has("console");
    let traces = crate::features::has("traceroute");
    let usage: Vec<(&str, &str)> = [
        (console, "rvrs", "Open the Console"),
        (
            console && traces,
            "rvrs <target>",
            "Open the Console and trace the target",
        ),
        (
            !console && traces,
            "rvrs <target>",
            "Trace the target and print the result",
        ),
        (
            traces,
            "rvrs traceroute [flags] <target>",
            "Trace and print the result",
        ),
        (
            crate::features::has("ping"),
            "rvrs ping [flags] <target>",
            "Ping until Ctrl-C, or -c packets",
        ),
        (
            crate::features::has("dns"),
            "rvrs dig <name>",
            "Look the name up at every source and compare the answers",
        ),
        (
            crate::features::has("bgp"),
            "rvrs bgp <host|address|prefix|ASN>",
            "Look up the route, its origin validation and the network",
        ),
    ]
    .into_iter()
    .filter(|(shown, _, _)| *shown)
    .map(|(_, name, description)| (name, description))
    .collect();
    out.push_str(&section(style, "Usage", &usage));
    out.push('\n');
    out.push_str(&section_tagged(
        style,
        "Tools",
        &[
            (
                "traceroute",
                "The path to a target, hop by hop, with each hop's network",
                tag_of("traceroute"),
            ),
            (
                "ping",
                "Latency and loss to a target, packet by packet",
                tag_of("ping"),
            ),
            (
                "mtr",
                "Loss and latency at every hop, round after round",
                tag_of("mtr"),
            ),
            (
                "dig",
                "One name asked of this device's resolver, public resolvers and the authoritative servers",
                tag_of("dns"),
            ),
            (
                "iperf3",
                "Download and upload throughput against an iperf3 server",
                Tag::Soon,
            ),
            (
                "bgp",
                "The route an address falls in: origin AS, RPKI, AS paths, history and the network behind it",
                tag_of("bgp"),
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Runs",
        &[
            ("last", "Show the newest run again"),
            ("show <id>", "Show a run from history"),
            (
                "history [tool] [target]",
                "List the runs kept on this device, all or those of a tool or a target",
            ),
            (
                "save [id] [--case <name>]",
                "Save the newest run, or the one named, into the case used last or the one named",
            ),
            (
                "saved",
                "List the saved runs, or remove one with saved remove <id>",
            ),
            ("case", "List the cases"),
            ("case <name>", "List the runs in a case"),
            (
                "case new|rename|delete <name>",
                "Start, rename or delete a case. Its runs stay saved",
            ),
            (
                "case add|remove <name> [id]",
                "Put a run in a case, or take it out",
            ),
            ("favorite <host>", "Keep a host in Favorites"),
            ("favorites", "List the favorite hosts with their newest run"),
        ],
    ));
    out.push('\n');
    out.push_str(&section_marked(
        style,
        "Nodes",
        &[
            ("node", "Run this machine as a node in your fleet", false),
            (
                "nodes",
                "List the nodes in your fleet and whether each is online",
                false,
            ),
            (
                "nodes show <name>",
                "Show a node: its origin, its schedules and its newest runs",
                true,
            ),
            (
                "nodes rename|remove <name>",
                "Rename a node, or take it out of the fleet",
                true,
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section_marked(
        style,
        "Fleet",
        &[
            (
                "schedule add <tool> <target>",
                "Have nodes repeat a run: --every <interval> and --node, --fleet or --all-nodes",
                false,
            ),
            ("schedules", "List the schedules you made", false),
            ("schedule show <id>", "Show one schedule", false),
            (
                "schedule pause|resume <id>",
                "Have no node run a schedule, or have its nodes run it again",
                false,
            ),
            (
                "schedule rm <id>",
                "Delete a schedule and keep the runs it made",
                false,
            ),
            (
                "alerts",
                "List the alerts sent when a scheduled run's result changed",
                true,
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section_marked(
        style,
        "Account",
        &[
            (
                "login",
                "Sign this install in to your workspace: a code, confirmed in the browser",
                false,
            ),
            (
                "logout",
                "Sign out and revoke this install's credential",
                false,
            ),
            (
                "sync",
                "Send this device's runs to the account and read what is new, now",
                false,
            ),
            (
                "share [id]",
                "Print the share link of the newest run, or the one named: a public page",
                false,
            ),
            (
                "share --public-address",
                "Show the public address on the page, and --install-name the install's name",
                false,
            ),
            (
                "share --no-private-networks",
                "Leave the private network out of the page",
                false,
            ),
            (
                "share --no-public-address",
                "Leave the public address out, and --no-install-name the install's name",
                false,
            ),
            ("share --stop [id]", "End a run's share link", false),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Setup",
        &[
            ("status", "Check that this device can run the tools"),
            ("completions", "Set up tab completion for zsh, bash or fish"),
            ("update", "Update rvrs the way it was installed"),
            (
                "update --with|--without <feature>",
                "Add a build feature, or leave one out",
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section_marked(
        style,
        "Flags",
        &[
            ("--json", "Print the run as a report", false),
            ("--classic", "Print what the original tool prints", false),
            ("--color <when>", "auto, always or never", false),
            (
                "--case <name>",
                "Save the run into a case once it is made",
                false,
            ),
            NODE_FLAG,
            (
                "--no-wait",
                "With --node: print the run's id and leave",
                false,
            ),
            (
                "--expires-in <seconds>",
                "With --node: how long the request waits for the node",
                false,
            ),
            ("--fleet", "Run from every node in the fleet", true),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Examples",
        &[
            ("rvrs traceroute 1.1.1.1", ""),
            ("rvrs mtr example.com --node fra-1", ""),
            ("rvrs ping -c 20 -i 0.2 example.com", ""),
            ("rvrs last --json", ""),
        ],
    ));
    out
}

pub fn node(style: Style) -> String {
    let mut out = format!(
        "{} rvrs node\nThis machine as a node. It makes the runs people in your workspace ask of it and repeats\nthe ones they schedule, within its own limits, and sends each result to the account of\nwhoever asked. It opens every connection itself, so it needs no open port.\n\n",
        style.bold("//")
    );
    out.push_str(&section(
        style,
        "Usage",
        &[(
            "RVRS_TOKEN=<token> rvrs node",
            "The token is made under Installs at app.rvrs.dev",
        )],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Flags",
        &[
            (
                "--state <folder>",
                "Where the node keeps its outbox, its schedules and its token",
            ),
            (
                "--tag <key>=<value>",
                "A tag the node declares. Repeat it for more",
            ),
            ("--location <lat>,<lon>", "Where the node is"),
            ("--place <text>", "The place by name, as in \"Esbjerg, DK\""),
            ("--location-precision <p>", "exact, area or city"),
            (
                "--config <file>",
                "The node's file, for its tags and location",
            ),
            (
                "--allow <range>",
                "A private range the node may probe, as in 192.168.1.0/24",
            ),
            (
                "--limit <name>=<value>",
                "runsAtOnce, runsPerMinute, packetsPerRun, secondsPerRun or reportBytes",
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Environment",
        &[
            ("RVRS_TOKEN", "The node's token"),
            ("RVRS_API", "Another API than api.rvrs.dev"),
            ("RVRS_NODE_STATE", "--state"),
            ("RVRS_NODE_TAGS", "--tag, separated by commas"),
            ("RVRS_NODE_LOCATION", "--location"),
            ("RVRS_NODE_ALLOW", "--allow, separated by commas"),
            ("RVRS_NODE_LIMITS", "--limit, separated by commas"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Examples",
        &[
            ("RVRS_TOKEN=rvrs_... rvrs node --tag isp=telia", ""),
            (
                "docker run -d --network host -e RVRS_TOKEN=rvrs_... -v rvrs-node:/var/lib/rvrs ghcr.io/rvrs-dev/rvrs",
                "",
            ),
        ],
    ));
    out
}

pub fn schedule(style: Style) -> String {
    let mut out = format!(
        "{} rvrs schedule\nA run your nodes repeat at an interval. Each node keeps the schedule and repeats the run by\nitself, also while the API is out of reach, and the runs are kept in your account.\n\n",
        style.bold("//")
    );
    out.push_str(&section(
        style,
        "Usage",
        &[
            ("rvrs schedules", "List the schedules you made"),
            (
                "rvrs schedule add <tool> <target> [flags]",
                "Make a schedule",
            ),
            ("rvrs schedule show <id>", "Show one schedule"),
            ("rvrs schedule pause <id>", "Have no node run it"),
            ("rvrs schedule resume <id>", "Have its nodes run it again"),
            (
                "rvrs schedule rm <id>",
                "Delete it and keep the runs it made",
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Flags of add",
        &[
            (
                "--every <interval>",
                "The time between two runs on one node: 300, 90s, 5m or 2h, from 60 seconds to 24 hours",
            ),
            ("--node <name>", "A node to run it on. Repeat it for more"),
            (
                "--fleet <selector>",
                "Every node whose tags match, as in isp=telia",
            ),
            ("--all-nodes", "Every node of the workspace"),
            ("--name <name>", "What people call the schedule"),
            ("--json", "Print the schedule as the API answers it"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "The tool's flags",
        &[
            (
                "traceroute",
                "-4, -6, -q, -w, -m, -f, -T, -p, --mtu, -n, --no-lookups",
            ),
            ("ping", "-4, -6, -c, -i, -W, -n, --no-lookups"),
            ("mtr", "-4, -6, -c, -i, -Z, -m, -f, -n, --no-lookups"),
            ("dig", "The record type and --resolver"),
            ("bgp", "None"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Examples",
        &[
            (
                "rvrs schedule add ping -c 10 example.com --every 1m --node fra-1 --node cpe-4471",
                "",
            ),
            (
                "rvrs schedule add mtr example.com --every 5m --fleet \"isp=telia,line in (ftth,cable)\"",
                "",
            ),
            ("rvrs schedule pause 7a1c9e52", ""),
        ],
    ));
    out
}

pub fn traceroute(style: Style) -> String {
    let mut out = format!(
        "{} rvrs traceroute\nThe path to a target, hop by hop. Takes the flags of the traceroute it replaces.\n\n",
        style.bold("//")
    );
    out.push_str(&section(
        style,
        "Usage",
        &[("rvrs traceroute [flags] <target> [packet length]", "")],
    ));
    out.push('\n');
    out.push_str(&section_tagged(
        style,
        "traceroute flags",
        &tag_rows(
            &[
                (
                    "-m, --max-hops <n>",
                    "Last hop to probe, 1 to 255 (default 30)",
                ),
                ("-q, --queries <n>", "Probes per hop, 1 to 10 (default 3)"),
                (
                    "-w, --wait <s>",
                    "Longest wait for a reply, in seconds (default 5)",
                ),
                ("-f, --first <n>", "First hop to probe (default 1)"),
                (
                    "-z, --sendwait <t>",
                    "Pause between probes: seconds up to 10, milliseconds beyond",
                ),
                ("-n", "Addresses only, no reverse DNS"),
                (
                    "-A, --as-path-lookups",
                    "Show each hop's AS number in the classic output",
                ),
                ("-4, -6", "Resolve the target to IPv4 or IPv6"),
                (
                    "-I, --icmp",
                    "ICMP echo probes, which rvrs sends unless -T is given",
                ),
                (
                    "-T, --tcp",
                    "TCP probes to a port: is it open, closed or filtered, and where a SYN stops",
                ),
                ("-p, --port <n>", "The port for -T (default 80)"),
                (
                    "--mtu",
                    "The path MTU: the largest packet that crosses unfragmented, and who limits it",
                ),
            ],
            &[
                ("-T, --tcp", "tcp"),
                ("-p, --port <n>", "tcp"),
                ("--mtu", "mtu"),
            ],
        ),
    ));
    out.push('\n');
    out.push_str(&section_marked(
        style,
        "rvrs flags",
        &[
            ("--json", "Print the run as a report", false),
            (
                "--classic",
                "Print what traceroute prints, which a pipe gets anyway",
                false,
            ),
            (
                "--no-lookups",
                "No reverse DNS and no network lookups",
                false,
            ),
            ("--color <when>", "auto, always or never", false),
            (
                "--case <name>",
                "Save the run into a case once it is made",
                false,
            ),
            NODE_FLAG,
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Not supported yet",
        &[("-U, -s, -i", "UDP probes, a source address or interface")],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Examples",
        &[
            ("rvrs traceroute 1.1.1.1", ""),
            ("rvrs traceroute -6 -q 5 example.com", ""),
            ("rvrs traceroute -T -p 443 example.com", ""),
            ("rvrs traceroute --mtu example.com", ""),
            ("rvrs traceroute -n example.com | awk '{print $2}'", ""),
            ("rvrs traceroute --json example.com | jq .hops", ""),
        ],
    ));
    out.push('\n');
    out.push_str(&style.dim(
        "Network lookups send each public hop address to stat.ripe.net. Use --no-lookups to leave them out.\n",
    ));
    out
}

pub fn ping(style: Style) -> String {
    let mut out = format!(
        "{} rvrs ping\nLatency and loss to a target, packet by packet. Takes the flags of the ping it replaces.\n\n",
        style.bold("//")
    );
    out.push_str(&section(
        style,
        "Usage",
        &[("rvrs ping [flags] <target>", "")],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "ping flags",
        &[
            ("-c <n>", "Stop after n packets (default: until Ctrl-C)"),
            ("-i <s>", "Seconds between packets (default 1)"),
            (
                "-W <s>",
                "Seconds a packet may take before it counts as a loss (default 2)",
            ),
            ("-w <s>", "Stop after this many seconds, whatever was sent"),
            ("-s <bytes>", "Payload size (default 56)"),
            ("-t <ttl>", "TTL of the packets sent (default 64)"),
            ("-n", "Addresses only, no reverse DNS"),
            ("-q", "Only the opening line and the statistics"),
            ("-4, -6", "Resolve the target to IPv4 or IPv6"),
        ],
    ));
    out.push('\n');
    out.push_str(&section_marked(
        style,
        "rvrs flags",
        &[
            ("--json", "Print the run as a report when it ends", false),
            (
                "--classic",
                "Print what ping prints, which a pipe gets anyway",
                false,
            ),
            (
                "--no-lookups",
                "No reverse DNS and no network lookup",
                false,
            ),
            ("--color <when>", "auto, always or never", false),
            (
                "--case <name>",
                "Save the run into a case once it is made",
                false,
            ),
            NODE_FLAG,
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Not supported yet",
        &[("-I, -f and the rest", "A source interface, flood mode")],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Examples",
        &[
            ("rvrs ping 1.1.1.1", ""),
            ("rvrs ping -c 20 -i 0.2 example.com", ""),
            ("rvrs ping -c 1 -W 1 example.com && echo up", ""),
            (
                "rvrs ping -c 10 --json example.com | jq .interpretation",
                "",
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&style.dim("Exit code 0 when the target answered, 1 when it didn't, 2 when the run couldn't be made.\n"));
    out
}

pub fn mtr(style: Style) -> String {
    let mut out = format!(
        "{} rvrs mtr\nLoss and latency at every hop, round after round. Takes the flags of the mtr it replaces.\n\n",
        style.bold("//")
    );
    out.push_str(&section(
        style,
        "Usage",
        &[("rvrs mtr [flags] <target>", "")],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "mtr flags",
        &[
            (
                "-c, --report-cycles <n>",
                "Stop after n rounds (default: until Ctrl-C, or 10 for a report)",
            ),
            (
                "-r, --report",
                "Print mtr's report when the rounds are done",
            ),
            (
                "-w, --report-wide",
                "A report whose host column fits the longest name",
            ),
            (
                "-i, --interval <s>",
                "Seconds between rounds, from 0.1 (default 1)",
            ),
            (
                "-m, --max-ttl <n>",
                "Last hop to probe, 1 to 255 (default 30)",
            ),
            ("-f, --first-ttl <n>", "First hop to probe (default 1)"),
            (
                "-s, --psize <bytes>",
                "Packet size, headers included (default 64)",
            ),
            (
                "-Z, --timeout <s>",
                "Seconds a probe may take before it counts as a loss (default 2)",
            ),
            (
                "-U, --max-unknown <n>",
                "Silent hops probed past the farthest that answered (default 5)",
            ),
            ("-n, --no-dns", "Addresses only, no reverse DNS"),
            ("-b, --show-ips", "Names and addresses in the report"),
            ("-z, --aslookup", "Each hop's AS number in the report"),
            ("-4, -6", "Resolve the target to IPv4 or IPv6"),
            ("-j", "Print the run as a report, as --json does"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "rvrs flags",
        &[
            ("--json", "Print the run as a report when it ends"),
            ("--classic", "Print mtr's report, which a pipe gets anyway"),
            ("--case <name>", "Save the run into this case"),
            ("--no-lookups", "No reverse DNS and no network lookups"),
            ("--color <when>", "auto, always or never"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Not supported yet",
        &[(
            "-u, -T, -a, -I, -C, -x",
            "UDP and TCP probes, a source address or interface, CSV and XML",
        )],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Examples",
        &[
            ("rvrs mtr 1.1.1.1", ""),
            ("rvrs mtr -c 30 -i 0.5 example.com", ""),
            ("rvrs mtr -rw example.com", ""),
            ("rvrs mtr -c 20 --json example.com | jq .interpretation", ""),
        ],
    ));
    out.push('\n');
    out.push_str(&style.dim(
        "Loss is red only when it reaches the destination. A middle hop that drops replies while later\nhops answer is likely rate-limiting them, and rvrs names it under the table.\n",
    ));
    out
}

pub fn bgp(style: Style) -> String {
    let mut out = format!(
        "{} rvrs bgp\nThe route an address falls in: who announces it, whether RPKI authorizes that, how route\ncollectors reach it, what changed, and the network behind it. Read from RIPEstat and from\nyour looking glasses. Nothing is probed.\n\n",
        style.bold("//")
    );
    out.push_str(&section(
        style,
        "Usage",
        &[
            (
                "rvrs bgp <host|address|prefix|ASN>",
                "The lookup: example.com, 192.0.2.1, 192.0.2.0/24, AS64500",
            ),
            (
                "rvrs bgp check <prefix> <ASN>",
                "Would this route pass RPKI origin validation?",
            ),
            ("rvrs bgp glass", "List your looking glasses"),
            (
                "rvrs bgp glass add <host>",
                "Add a looking glass. Every lookup of a route asks it for the prefix",
            ),
            ("rvrs bgp glass remove <host>", "Remove it"),
            (
                "rvrs bgp glass test <host> [prefix]",
                "Ask it for a prefix and print each step",
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "What prints",
        &[
            (
                "(none)",
                "Overview: the verdict, the route, who it is reached through, recent changes",
            ),
            (
                "--paths",
                "Every distinct AS path collectors see, and what your looking glasses hold",
            ),
            (
                "--prefixes",
                "The prefixes the network originates, with RPKI for the first 40",
            ),
            (
                "--neighbors",
                "For an AS: the networks seen upstream and downstream of it",
            ),
            (
                "--history",
                "Who announced the prefix, its visibility and BGP updates",
            ),
            (
                "--days <n>",
                "How far back History looks: 7, 30 or 90 (default 7)",
            ),
            (
                "--rpki",
                "The origin validation spelled out: the three checks and the covering ROAs",
            ),
            (
                "--sources",
                "Where each part came from and when it was observed",
            ),
            ("--all", "Everything above"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Looking glass flags",
        &[
            (
                "--type <type>",
                "alice-lg, lg-cgi, hyperglass or bird-lg-go (default: each is tried)",
            ),
            (
                "--name <name>",
                "What to call it (default: its own title, or the host)",
            ),
            (
                "--router <name>",
                "A router to ask, repeated or separated by commas. For test, the one to ask",
            ),
            ("--raw", "With test: print the router's own output"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "rvrs flags",
        &[
            ("--json", "Print the lookup as a report when it ends"),
            (
                "--classic",
                "Print plain text with its sources, which a pipe gets anyway",
            ),
            ("--no-glasses", "Don't ask your looking glasses"),
            ("--case <name>", "Save the lookup into this case"),
            ("--no-lookups", "No lookup of the network this device is in"),
            ("--color <when>", "auto, always or never"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Examples",
        &[
            ("rvrs bgp example.com", ""),
            ("rvrs bgp 192.0.2.0/24 --paths", ""),
            ("rvrs bgp AS64500 --neighbors --prefixes", ""),
            ("rvrs bgp example.com --history --days 90", ""),
            ("rvrs bgp check 192.0.2.0/24 AS64500", ""),
            ("rvrs bgp glass add lg.example.net", ""),
            ("rvrs bgp example.com --json | jq .interpretation", ""),
        ],
    ));
    out.push('\n');
    out.push_str(&style.dim(
        "An AS path is what a collector peer or a looking glass sees from its own network.\nNo ROA found means no ROA covers the route, so RPKI says nothing about it.\n",
    ));
    out
}

pub fn dig(style: Style) -> String {
    let mut out = format!(
        "{} rvrs dig\nOne name asked of this device's resolver, public resolvers and the authoritative servers.\nWith a server, a type or a +option it asks one question and prints what dig prints.\n\n",
        style.bold("//")
    );
    out.push_str(&section(
        style,
        "Usage",
        &[
            ("rvrs dig <name>", "The lookup, with every source compared"),
            (
                "rvrs dig [@server] <name> [type] [+options]",
                "One question, in dig's output",
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "dig flags",
        &[
            (
                "@<server>",
                "The server to ask, by address or name (default: this device's resolver)",
            ),
            (
                "<type>, -t <type>",
                "A, AAAA, MX, TXT, NS, SOA, CNAME, HTTPS, CAA, DS, DNSKEY, ANY, TYPE99 (default A)",
            ),
            (
                "-x <address>",
                "The name for an address, through its PTR record",
            ),
            (
                "-q <name>",
                "The name, where it could be read as something else",
            ),
            ("-p <port>", "The server's port (default 53)"),
            ("-4, -6", "Ask over IPv4 or IPv6"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "dig options",
        &[
            ("+short", "The answer's values and nothing else"),
            ("+trace", "From the root down, one server per zone"),
            ("+tcp, +vc", "Ask over TCP from the start"),
            ("+dnssec, +do", "Ask for DNSSEC records with the answer"),
            ("+norecurse", "Ask the server for what it holds itself"),
            ("+noedns", "No OPT record in the question"),
            (
                "+noall, +answer, +authority, +additional",
                "Which sections print",
            ),
            (
                "+question, +comments, +stats, +cmd",
                "Which lines print around them, each with a +no form",
            ),
            (
                "+time=<s>, +tries=<n>, +retry=<n>",
                "Seconds per try (default 5) and tries (default 3)",
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "rvrs flags",
        &[
            (
                "--resolver <address>",
                "A resolver of your own, asked beside the public ones",
            ),
            ("--json", "Print the lookup as a report when it ends"),
            ("--classic", "Print dig's output, which a pipe gets anyway"),
            ("--case <name>", "Save the lookup into this case"),
            ("--no-lookups", "No lookup of this device's public address"),
            ("--color <when>", "auto, always or never"),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Not supported yet",
        &[(
            "-b, -f, -k, -y, -c CH, +multiline, +cookie, +subnet",
            "A source address, batch files, TSIG keys, other classes and the rest of dig's options",
        )],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Examples",
        &[
            ("rvrs dig example.com", ""),
            ("rvrs dig example.com MX +short", ""),
            ("rvrs dig @1.1.1.1 example.com AAAA", ""),
            ("rvrs dig -x 1.1.1.1 +short", ""),
            ("rvrs dig example.com +trace", ""),
            ("rvrs dig example.com --json | jq .interpretation", ""),
        ],
    ));
    out.push('\n');
    out.push_str(&style.dim(
        "rvrs checks that DNSSEC records are present along the delegation. It doesn't validate signatures.\n",
    ));
    out
}
