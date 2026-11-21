//! Help text. Every line follows the rvrs voice: what the thing does, in the industry's words.

use crate::style::Style;

/// The version and commit `build.rs` derives from git: every commit is a new version.
pub const VERSION: &str = env!("RVRS_VERSION");
pub const COMMIT: &str = env!("RVRS_COMMIT");

pub fn version() -> String {
    format!("rvrs {VERSION} ({COMMIT})")
}

fn section(style: Style, title: &str, rows: &[(&str, &str)]) -> String {
    let marked: Vec<(&str, &str, bool)> = rows.iter().map(|(n, d)| (*n, *d, false)).collect();
    section_marked(style, title, &marked)
}

/// A section whose rows may be marked Soon: planned, named here so people know what is coming,
/// and refused with a sentence when typed.
fn section_marked(style: Style, title: &str, rows: &[(&str, &str, bool)]) -> String {
    let widest = rows
        .iter()
        .map(|(name, _, _)| name.len())
        .max()
        .unwrap_or(0);
    let mut out = format!("{}\n", style.bold(title));
    for (name, description, soon) in rows {
        let tag = if *soon {
            format!("  {}", style.yellow("Soon"))
        } else {
            String::new()
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
    true,
);

pub fn main(style: Style) -> String {
    let mut out = format!(
        "{} rvrs {VERSION}\nSee what the network is doing.\n\n",
        style.bold("//")
    );
    out.push_str(&section(
        style,
        "Usage",
        &[
            ("rvrs", "Open the Console"),
            ("rvrs <target>", "Open the Console and trace the target"),
            (
                "rvrs traceroute [flags] <target>",
                "Trace and print the result",
            ),
            (
                "rvrs ping [flags] <target>",
                "Ping until Ctrl-C, or -c packets",
            ),
            (
                "rvrs dig <name>",
                "Look the name up at every source and compare the answers",
            ),
            (
                "rvrs bgp <host|address|prefix|ASN>",
                "Look up the route, its origin validation and the network",
            ),
        ],
    ));
    out.push('\n');
    out.push_str(&section_marked(
        style,
        "Tools",
        &[
            (
                "traceroute",
                "The path to a target, hop by hop, with each hop's network",
                false,
            ),
            ("ping", "Latency and loss to a target, packet by packet", false),
            ("mtr", "Loss and latency at every hop, round after round", false),
            (
                "dig",
                "One name asked of this device's resolver, public resolvers and the authoritative servers",
                false,
            ),
            (
                "iperf3",
                "Download and upload throughput against an iperf3 server",
                true,
            ),
            (
                "bgp",
                "The route an address falls in: origin AS, RPKI, AS paths, history and the network behind it",
                false,
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
            ("node", "Run this machine as a node in your fleet", true),
            (
                "nodes",
                "List the nodes in your fleet and whether they answer",
                true,
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
                "schedule <tool> <target>",
                "Have a node repeat a run: --node <name> --every <time>",
                true,
            ),
            ("schedules", "List the schedules across the fleet", true),
            ("schedules remove <id>", "Stop a schedule", true),
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
            ("share [id]", "Print the share link to a run page", true),
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
            ("--fleet", "Run from every node in the fleet", true),
        ],
    ));
    out.push('\n');
    out.push_str(&section(
        style,
        "Examples",
        &[
            ("rvrs traceroute 1.1.1.1", ""),
            ("rvrs ping -c 20 -i 0.2 example.com", ""),
            ("rvrs last --json", ""),
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
    out.push_str(&section(
        style,
        "traceroute flags",
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
