# rvrs CLI

See what the network is doing, from the terminal. `rvrs` is one binary that replaces traceroute, ping, mtr and dig with the same flags, and adds a BGP lookup.

```bash
rvrs traceroute 1.1.1.1
```

```
// traceroute one.one.one.one  1.1.1.1 · IPv4 · this device

      AS64500 Example Net · Frankfurt, DE
  1 │ 192.168.1.1                                       3.2    4.1    3.0 ms
  2 │ lo0.edge1.fra.example.net 198.51.100.1            8.2   16.1   10.1 ms
  3 │ ae1.core2.fra.example.net 198.51.100.9           10.9   10.1   13.6 ms
      AS13335 Cloudflare, Inc. · Frankfurt, DE
  4 │ 203.0.113.9                                      16.2   24.5   10.6 ms
  5 ● one.one.one.one 1.1.1.1                          10.4   10.3   10.2 ms

Reached 1.1.1.1 · 5 hops · 10.3 ms · 2 networks · 0.2 s
```

## Install

```bash
curl -fsSL https://rvrs.dev/install.sh | sh
```

rvrs runs on macOS and Linux. The installer picks the build for your system and puts `rvrs` in `~/.local/bin`. `RVRS_INSTALL_DIR` sets another folder.

### Update

```bash
rvrs update
```

`rvrs update` updates rvrs the way it was installed:

- A binary from the installer replaces itself with the newest release, in place. Links named `traceroute`, `ping`, `mtr` and `dig` keep working. The new binary has to run on this device before it takes the old one's place.
- A build from a git checkout pulls the new commits, lists them, and builds them. A checkout with uncommitted changes, or with commits of its own, is left alone and rvrs says why.
- A Homebrew install prints the command that updates it, `brew upgrade rvrs`.

`rvrs update --check` says whether a newer version is out and changes nothing. Once a day rvrs also looks on its own, in the background, so no run waits for it. When it finds a newer version, a result ends with one line, such as `rvrs 0.3.1 is out. Install it with rvrs update.`, and `rvrs status` shows the same line. `RVRS_UPDATE_CHECK=0` turns the daily look off.

### Use rvrs as traceroute, ping, mtr and dig

Called as `traceroute`, `ping`, `mtr` or `dig`, rvrs prints what those tools print, so scripts that parse their output keep working. As `mtr` it prints mtr's report.

The installer links all four when you add `--link`:

```bash
curl -fsSL https://rvrs.dev/install.sh | sh -s -- --link
```

It puts `traceroute`, `ping`, `mtr` and `dig` links next to `rvrs` and leaves any real file there alone. The links take effect when `~/.local/bin` comes before `/usr/bin` and `/sbin` on your `PATH`, and the installer tells you when it doesn't.

To link them by hand:

```bash
for tool in traceroute ping mtr dig; do ln -s "$(command -v rvrs)" ~/.local/bin/$tool; done
```

To go back to the system tools, remove the links:

```bash
rm ~/.local/bin/traceroute ~/.local/bin/ping ~/.local/bin/mtr ~/.local/bin/dig
```

## How it differs

- **Speed.** All probes go out at once, so a path that answers is traced in under a second. A silent hop costs one wait, where traceroute waits three times.
- **No root.** rvrs uses unprivileged ICMP sockets. `rvrs status` checks whether this device allows them.
- **Same flags, same output in a pipe.** `rvrs traceroute`, `rvrs ping`, `rvrs mtr` and `rvrs dig` take the flags of the tools they replace. In a pipe, with `--classic` or through a symlink named `traceroute`, `ping`, `mtr` or `dig`, they print what the originals print.
- **DNS.** `rvrs dig example.com` asks this device's resolver, three public resolvers and the zone's own servers, and says where they differ and why that is usually normal.
- **BGP.** `rvrs bgp example.com` shows the route the address falls in, its origin AS, the RPKI result, the AS paths route collectors see, 90 days of history and the network behind it. Your own looking glasses are asked beside the collectors.
- **History.** Every run is kept. `rvrs last`, `rvrs history` and `rvrs show <id>` bring one back, and `--json` prints it as a report. `rvrs save` keeps a run after history is cleared, and cases group saved runs around one question.

## Commands

| Command | Does |
| --- | --- |
| `rvrs` | Opens the Console |
| `rvrs <target>` | Opens the Console and traces the target |
| `rvrs traceroute [flags] <target>` | Traces and prints the result |
| `rvrs ping [flags] <target>` | Pings until Ctrl-C, or `-c` packets |
| `rvrs mtr [flags] <target>` | Measures loss and latency at every hop, round after round, until Ctrl-C or `-c` rounds |
| `rvrs dig <name>` | Looks the name up at every source and compares the answers. `rvrs dns` is the same command |
| `rvrs dig [@server] <name> [type] [+options]` | Asks one question and prints what dig prints |
| `rvrs bgp <host\|address\|prefix\|ASN>` | Looks up the route, its origin validation, AS paths, history and network |
| `rvrs bgp check <prefix> <ASN>` | Says whether that route would pass RPKI origin validation |
| `rvrs bgp glass` | Lists your looking glasses. `glass add`, `glass remove` and `glass test` manage them |
| `rvrs last` | Shows the newest run |
| `rvrs show <id>` | Shows a run from history. The first characters of the id are enough |
| `rvrs history` | Lists the runs on this device. `rvrs history clear` deletes them |
| `rvrs save [id] [--case <name>]` | Saves the newest run, or the one named, into the case used last or the one named |
| `rvrs saved` | Lists saved runs. `rvrs saved remove <id>` takes one out |
| `rvrs case` | Lists the cases |
| `rvrs case <name>` | Lists the runs in a case |
| `rvrs case new\|rename\|delete <name>` | Starts, renames or deletes a case. Deleting a case keeps its runs saved |
| `rvrs case add\|remove <name> [id]` | Puts a run in a case or takes it out |
| `rvrs favorite <host>` | Adds a host to Favorites |
| `rvrs favorites` | Lists favorite hosts with their newest run |
| `rvrs status` | Checks that this device can run the tools |
| `rvrs completions install` | Sets up tab completion for zsh, bash or fish |
| `rvrs update` | Brings rvrs up to date, however it was installed |

Flags on every run: `--json` prints the report, `--classic` prints the original tool's output, `--color auto|always|never` sets color, and `--case <name>` saves the run into a case. `NO_COLOR` is honored.

### Soon

`rvrs help` lists these with a Soon label. Until they exist, each one says so and sends nothing.

| Command | Will do |
| --- | --- |
| `rvrs iperf3` | Download and upload throughput against an iperf3 server, as the rvrs app measures it |
| `rvrs node` | Runs this machine as a node in your fleet |
| `rvrs nodes` | Lists the nodes in your fleet. `nodes show`, `nodes rename` and `nodes remove` manage one |
| `rvrs schedule <tool> <target> --node <name> --every <time>` | Has a node repeat a run |
| `rvrs schedules`, `rvrs alerts` | List the fleet's schedules and the alerts they sent |
| `rvrs login`, `rvrs logout`, `rvrs share` | Sign in to your workspace, sign out, print a run's share link |
| `--node <name>` | Runs any tool from a node instead of this device |
| `--fleet` | Runs it from every node in the fleet |

## The Console

`rvrs` on its own opens the Console: the target field, Recent below it, and the result once a run starts.

```
 Origin:  this device                     ● traceroute   <↑↓>    Hop              __ __
 Source:  192.168.1.20                    ○ mtr          <↵>     Details         / // /  _____   _______  _____
 Target:  github.com                      ○ ping         <c>     Copy Address   / // /  / ___/ | / / ___/ / ___/
 Address: 140.82.121.3 · IPv4             ○ dig          <R>     Run Again     / // /  / /   | |/ / /    (__  )
 Network: AS36459 GitHub, Inc.            ○ bgp          <r>     Run Tool     /_//_/  /_/    |___/_/    /____/
 Status:  Reached 140.82.121.3 · 7 hops · 20.4 ms · 2 networks
┌──────────────────────────────── Traceroute(github.com)[7] ─────────────────────────────────┐
│ HOP↑ HOST                            ADDRESS        NETWORK               LOSS    AVG  ...  │
│ 1    192.168.1.1                     192.168.1.1    AS64500 Example Net     0%    6.0       │
│ 2    lo0.edge1.fra.example.net       198.51.100.1   ┆                       0%   10.7       │
│●7    lb-140-82-121-3-fra.github.com  140.82.121.3   AS36459 GitHub, Inc.    0%   20.4       │
│ LATENCY BY HOP                                                                              │
│    20.4 ┤        ███ ███         ███                                                        │
│    0 ms ┤▇▇▇ ███ ███ ███ ××× ××× ███                                                        │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
  ⇧1 Console   ⇧2 Favorites   ⇧3 Cases  ›  traceroute
```

Latency is green under 50 ms, yellow from 50 to 150 ms and red above, with the number always beside the color.

| Key | Does |
| --- | --- |
| Return | Runs the selected tool against the target. `ping example.com` picks the tool too |
| typing | Filters Recent by tool and host: `ping git` |
| tab | Switches between traceroute, MTR, ping, dig and bgp, or between the hops and a hop's details. In a DNS or BGP lookup it goes to the next tab |
| 1 2 3 4 | In a DNS lookup: Overview, Records, Compare, Trace |
| o, z | In a DNS lookup: shows the next source's records, copies the selected record as a zone line |
| 1 2 3 4 5 | In a BGP lookup: Overview, Paths, Prefixes, History, Sources. For a network: Overview, Neighbors, Prefixes, Sources |
| o, d, v | In a BGP lookup: the next side of Neighbors or filter of Prefixes, the next range of History, the RPKI result of the selected prefix |
| ↑ ↓ or j k | Moves through the list or the hops |
| PgUp PgDn | Moves a screen at a time through the list, the hops or a lookup's rows |
| → or Return | Opens a run, a hop's details, a record's details, a case or a favorite. In a BGP lookup it opens the selected network's or prefix's own lookup, or an AS path's details |
| ← or esc | Goes back one level. esc in the empty field quits |
| t, m, p, b, n, w | On a hop: Run Traceroute, Run MTR, Run Ping, Run BGP, Copy Hostname, Whois |
| d | In an MTR: switches between each hop's figures and a wide strip of its rounds |
| c | Copies the address of the selected hop or the ping target, the selected record's value, or the selected row of a BGP lookup |
| r | Opens the run picker with every tool that can take the selected run's target, hop, record, network or prefix. Return runs the selected tool, and its letter (t, m, p, d, b) runs it at once |
| R | Runs the open run again |
| s | Stops the run while it runs. Once it has ended, saves it or removes it from saved runs |
| a | Adds the run to cases |
| f | Adds the host to Favorites or removes it |
| x | Takes a run out of the open case |
| ⇧1 ⇧2 ⇧3 | Console, Favorites, Cases |
| ? | Shows every key and the latency colors |

A ping in the Console sends 20 packets a second apart, as the app does. An MTR runs until `s` stops it. Each hop shows its figures and a strip of its newest rounds, and the charts above the table show the destination's latency, jitter and loss round by round. Jitter is how far each reply moved from the one before, and loss is counted over the last 10 rounds. A hop's details chart the same for that hop. Starting another run, or quitting, stops a live run and keeps it in history. On a terminal under 22 rows the header hides and the keys move to the bottom line. The Console starts with the tool chosen last.

A DNS lookup has the app's four tabs. Overview holds the verdict, the key records, the resolution path and what changed since the last lookup of the name. Records lists one source's answers by group. Compare lays the sources side by side with the reason for each difference. Trace shows the delegation path and the DNSSEC state. A record's details open beside the records with one sentence on what the record is, the tools to run against the host it names, Copy Value and Copy as Zone Line.

A BGP lookup has the app's tabs. Overview holds the verdict, the route with its origin AS and RPKI result, the networks it is reached through, your newest traceroute to the same target laid against the collectors' paths, recent changes and related prefixes. Paths lists every distinct AS path with the share of collector peers that see it, then what each of your looking glasses holds. Prefixes lists what the origin network announces, with a filter. History draws who announced the prefix and how many peers saw it over 7, 30 or 90 days. Sources says where each part came from and when it was observed. An AS number or a prefix typed under any tool runs as a BGP lookup. Return on a network or a prefix opens its own lookup, and a path opens beside the list network by network.

## traceroute flags

rvrs follows the Linux traceroute.

| Flag | Meaning | Default |
| --- | --- | --- |
| `-m`, `--max-hops <n>` | Last hop to probe, 1 to 255 | 30 |
| `-q`, `--queries <n>` | Probes per hop, 1 to 10 | 3 |
| `-w`, `--wait <s>` | Longest wait for a reply, in seconds | 5 |
| `-f`, `--first <n>` | First hop to probe | 1 |
| `-z`, `--sendwait <t>` | Pause between probes: seconds up to 10, milliseconds above | 5 ms |
| `-n` | Addresses only, no reverse DNS | |
| `-A`, `--as-path-lookups` | Each hop's AS number in the classic output | |
| `-4`, `-6` | Resolve the target to IPv4 or IPv6 | the system's order |
| `-I`, `--icmp` | ICMP echo probes, which rvrs always sends | |
| `<packet length>` | Total packet size, after the target | 60 |

Where it differs from Linux traceroute:

- Probes are ICMP echo only. Linux defaults to UDP. `-T`, `-U`, `-p`, `-s` and `-i` exit with an error that says so.
- All probes go out in parallel, so `-N` is accepted and does nothing.
- Once the destination answers, a probe still out waits ten times the slowest reply, at least 0.5 s and at most `-w`.
- A router that rate-limits ICMP replies may answer only the first probe of its hop. `-z 0.2` spaces the probes out.
- The classic output has traceroute's format (`host (address)  1.234 ms`, `* * *`, `!H`), but not byte for byte. Scripts should use `--json`.

Exit codes: 0 when the run was made, whether or not the target answered. 1 when it couldn't be made, such as a name that didn't resolve. 2 for invalid arguments. 130 after a second Ctrl-C.

## ping flags

rvrs follows the Linux ping (iputils).

```
$ rvrs ping -i 0.2 github.com
// ping github.com  140.82.121.4 · IPv4 · this device

    1    21.9 ms  ttl 58
    2    23.6 ms  ttl 58
    3    21.4 ms  ttl 58

  ▇▇▇▇█▇▇▇▇▆▇▇█▇▇▇

16 pkt · 0.0% loss · 22.7 ms avg
min 20.2 · max 26.2 · jitter 2.0 · p50 22.0 · p95 26.2 ms
Stable. No loss over 16 packets. p95 26.2 ms, jitter 2.0 ms.
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `-c <n>` | Stop after n packets | until Ctrl-C |
| `-i <s>` | Seconds between packets, from 0.01 | 1 |
| `-W <s>` | Seconds before a packet counts as lost | 2 |
| `-w <s>` | Stop after this many seconds | |
| `-s <bytes>` | Payload size | 56 |
| `-t <ttl>` | TTL of the packets sent | 64 |
| `-n` | Addresses only, no reverse DNS | |
| `-q` | Only the first line and the statistics | |
| `-4`, `-6` | Resolve the target to IPv4 or IPv6 | the system's order |

In a pipe each line is ping's own (`64 bytes from one.one.one.one (1.1.1.1): icmp_seq=1 ttl=58 time=11.8 ms`), followed by the statistics with `rtt min/avg/max/mdev`.

Where it differs from Linux ping:

- A packet counts as lost after `-W` seconds, 2 by default. Linux ping waits up to 10 seconds.
- The verdict (Stable, Jittery, Occasional loss, Packet loss) appears after 10 packets and uses the app's thresholds. The report keeps it under `interpretation`.
- `-f`, `-I` and the rarer flags exit with an error.
- macOS ping spells some flags differently: there `-t` is a timeout and `-m` the TTL.

Exit codes are ping's: 0 when the target answered, 1 when it didn't, 2 when the run couldn't be made. `rvrs ping -c 1 -W 1 host && echo up` works.

## mtr flags

rvrs follows mtr.

```
$ rvrs mtr example.com
// mtr example.com  93.184.216.34 · IPv4 · this device

                                             Loss  Snt   Last    Avg   Best  Worst Jitter
      AS64500 Example Net · Frankfurt, DE
  1 │ 192.168.1.1                           16.7%   12    0.5    0.6    0.4    0.9    0.2  ▂▂▃▂▂▃▂▃××▂▂
  2 │ lo0.edge1.fra.example.net 198.51.100.1 0.0%   16    8.6   10.1    6.7   16.5    3.0  ▄▃▄▅▃▄▄▄▃▃▃▄▃▃▄▃
      AS64501 Example Edge
  3 ● 93.184.216.34                          0.0%   16   23.3   21.4   19.8   24.9    1.7  ▆▆▆▆▇▆▆▇▆▆▆▆▆▆▆▇

16 pkt · 0.0% loss · 21.4 ms avg · 3 hops · 2 networks · 16 s
min 19.8 · max 24.9 · jitter 1.7 · p50 20.7 · p95 24.9 ms
Stable. No loss over 16 packets. p95 24.9 ms, jitter 1.7 ms.
Hop 1 answers fewer probes than the hops after it. Routers often rate-limit these replies, so the
path itself is likely fine.
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `-c`, `--report-cycles <n>` | Stop after n rounds | until Ctrl-C, or 10 for a report |
| `-r`, `--report` | Print mtr's report when the rounds are done | |
| `-w`, `--report-wide` | A report whose host column fits the longest name | |
| `-i`, `--interval <s>` | Seconds between rounds, from 0.1 | 1 |
| `-m`, `--max-ttl <n>` | Last hop to probe, 1 to 255 | 30 |
| `-f`, `--first-ttl <n>` | First hop to probe | 1 |
| `-s`, `--psize <bytes>` | Packet size, headers included | 64 |
| `-Z`, `--timeout <s>` | Seconds a probe may take before it counts as a loss | 2 |
| `-U`, `--max-unknown <n>` | Silent hops probed past the farthest that answered | 5 |
| `-n`, `--no-dns` | Addresses only, no reverse DNS | |
| `-b`, `--show-ips` | Names and addresses in the report | |
| `-z`, `--aslookup` | Each hop's AS number in the report | |
| `-4`, `-6` | Resolve the target to IPv4 or IPv6 | the system's order |
| `-j` | Prints the run as a report, as `--json` does | |

On a terminal the table updates in place and stays in the scrollback when the run ends. In a pipe, with `-r`, or called as `mtr`, rvrs prints mtr's report: a `Start:` line, the `HOST:` line with `Loss%   Snt   Last   Avg  Best  Wrst StDev`, and a line per hop.

What rvrs adds to what was measured:

- **Loss is red only when it reaches the end of the path.** A middle hop that drops replies while later hops are clean is likely rate-limiting them. rvrs shows that loss in gray and names the hop under the table. It needs two losses and 10% at a hop before it says so.
- **A verdict on the destination** after 10 packets, with the thresholds the app uses.
- **A strip of each hop's newest rounds** on a wide terminal: the height is the latency, a cross is a loss.

Differences from the original:

- rvrs sends ICMP echo probes only. `-u`, `-T`, `-a` and `-I` return an error.
- `rvrs mtr` never opens a full-screen view, so `-t` and `-g` return an error. `rvrs` on its own opens the Console, and `mtr example.com` typed there runs an MTR.
- `-j` and `--json` print the rvrs report. mtr's own JSON, CSV and XML are not written.
- A long run keeps its newest 1200 rounds per hop.
- The first round probes every hop up to `-m` to find the path at once. After that rvrs stops `-U` hops past the farthest one that answered until the destination is known.

## dig flags

`rvrs dig <name>` runs the lookup the rvrs app runs. It asks this device's resolver, Cloudflare, Google, Quad9 and the zone's authoritative servers for nine record types, walks the delegation from the root, and compares the answers.

```
$ rvrs dig example.com

// dig example.com  this device

All sources agree
Resolves to 2 addresses, zone served by example-dns.net. DNSSEC records are present from the root down.
A / AAAA 1 / 1 · TTL 5m · DNSSEC Signed

Key records
  A      192.0.2.10                                        5m
  AAAA   2001:db8::10                                      5m
  MX     10 mail.example.com                               1h
  NS     ns1.example-dns.net                               1d

Compare
  Source                Server           A                 TTL      Time  Status
  Router                192.168.1.1      192.0.2.10        2m       4 ms  Match
  Cloudflare            1.1.1.1          192.0.2.10        5m      12 ms  Match
  Google                8.8.8.8          192.0.2.10        5m      27 ms  Match
  Quad9                 9.9.9.9          192.0.2.10        4m      27 ms  Match
  ns1.example-dns.net   198.51.100.53    192.0.2.10        5m      21 ms  Match

Delegation path
  Root           .            a.root-servers.net · 27 ms · delegates com to 13 servers · DS · RRSIG
  TLD            com          a.gtld-servers.net · 23 ms · delegates example.com to 2 servers · DS · RRSIG
  Authoritative  example.com  ns1.example-dns.net · 21 ms · answers for the name · RRSIG
  com publishes a DS for example.com, the zone serves its DNSKEY records and its answers are signed.
  A validating resolver can check every step.

Recent changes
  Same addresses, names and nameservers as the lookup 2h ago.
```

When sources answer differently, a Differences section names each difference with its usual reason, such as a CDN that answers by where the resolver asks from. Recent changes compares the lookup with the one before it in history. `--resolver <address>` adds a resolver of your own to the sources, and can be given more than once.

rvrs checks that DNSSEC records are present along the delegation. It doesn't validate signatures.

With a server, a record type or a `+option`, rvrs asks one question and prints what dig prints. A pipe, `--classic` and a link named `dig` do the same for a name alone.

```bash
rvrs dig example.com MX +short
rvrs dig @1.1.1.1 example.com AAAA
rvrs dig -x 1.1.1.1 +short
rvrs dig example.com +trace
rvrs dig example.com --json | jq .interpretation
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `@<server>` | The server to ask, by address or name | This device's resolver |
| `<type>`, `-t <type>` | A, AAAA, CNAME, MX, TXT, NS, SOA, HTTPS, CAA, DS, DNSKEY, PTR, SRV, ANY or `TYPE99` | A |
| `-x <address>` | The name for an address, through its PTR record | |
| `-q <name>` | The name, where it could be read as a type | |
| `-p <port>` | The server's port | 53 |
| `-4`, `-6` | Ask over IPv4 or IPv6 | |
| `+short` | The answer's values and nothing else | |
| `+trace` | From the root down, one server per zone | |
| `+tcp`, `+vc` | Ask over TCP from the start | UDP, then TCP when the answer is truncated |
| `+dnssec`, `+do` | Ask for DNSSEC records with the answer | |
| `+norecurse` | Ask the server for what it holds itself | |
| `+noedns` | No OPT record in the question | |
| `+noall`, `+answer`, `+authority`, `+additional` | Which sections print | All |
| `+question`, `+comments`, `+stats`, `+cmd` | Which lines print around them, each with a `+no` form | All |
| `+time=<s>`, `+tries=<n>`, `+retry=<n>` | Seconds per try and the number of tries | 5 and 3 |
| `--resolver <address>` | A resolver of your own, asked beside the public ones in the lookup | |
| `--no-lookups` | No lookup of this device's public address | |

Differences from the original:

- The first line names rvrs and its version where dig names itself. Every other line is dig's, as dig 9.10 prints it.
- Class IN only. `-b`, `-f`, `-k`, `-y`, `+multiline`, `+cookie`, `+subnet` and any option not in the table return an error that names it.
- `+trace` asks this device's resolver for the root servers, then one server per zone. It adds DNSSEC records only with `+dnssec`.
- One question is printed and not kept. The lookup is a run: it goes to history, `rvrs last` shows it again, and `--case` saves it.
- A question exits with dig's codes: 0 for any answer, NXDOMAIN included, and 9 when no server answered. The lookup exits 0 when a source answered and 1 when none did.
- `--json` prints the lookup as a report and takes a name alone.

## bgp

`rvrs bgp` takes a hostname, an address, a prefix or an AS number and runs the lookup the rvrs app runs. Nothing is probed. The route, its visibility, AS paths, history and the network come from [RIPEstat](https://stat.ripe.net/docs/data-api/) (RIPE RIS route collectors, a Routinator validator and the registries' statistics), and your looking glasses add what their own routers hold.

```text
$ rvrs bgp example.com

// bgp example.com  route collectors

Routed and authorized
192.0.2.0/24 is announced by AS64500 Example Net, seen by 318 of 320 collector peers. A ROA
authorizes this origin AS.
Seen by 318/320 peers · Paths 42 · First seen 2017

Route
  Address           192.0.2.10
  Announced prefix  192.0.2.0/24
  Example Net       AS64500
  RPKI              Valid
  Route object      RADB
  Registry          RIPE NCC

Reached through
  AS64501       Transit One   ████████████   68.6%
  AS64502       Transit Two   █████▌         31.4%

Your path
  AS64510 › AS64501 › AS64500
  Your traceroute entered AS64500 Example Net from AS64501 Transit One. 68.6% of collector
  peers see the route through the same network.
  Traceroute · 5m ago

Network
  About AS64500
  Example Net · 12 IPv4 and 3 IPv6 prefixes · 15 of 15 checked have a ROA

Recent changes
  No origin AS changes in 7 days
  1,204 BGP updates seen by collectors in that time.

Sources
  RIPE RIS and RPKI · observed 3h ago
```

```bash
rvrs bgp 192.0.2.0/24 --paths
rvrs bgp AS64500 --neighbors --prefixes
rvrs bgp example.com --history --days 90
rvrs bgp check 192.0.2.0/24 AS64500
rvrs bgp example.com --json | jq .interpretation
```

| Flag | Prints | Default |
| --- | --- | --- |
| (none) | Overview: the verdict, the route, who it is reached through, recent changes | |
| `--paths` | Every distinct AS path collectors see, and what your looking glasses hold | |
| `--prefixes` | The prefixes the network originates, with RPKI for the first 40 | |
| `--neighbors` | For an AS: the networks seen upstream and downstream of it | |
| `--history` | Who announced the prefix, its visibility and 7 days of BGP updates | |
| `--days <n>` | How far back History looks: 7, 30 or 90 | 7 |
| `--rpki` | The origin validation spelled out: the three checks and the covering ROAs | |
| `--sources` | Where each part came from and when it was observed | |
| `--all` | Everything above | |
| `--no-glasses` | Leaves your looking glasses out of this lookup | |
| `--no-lookups` | No lookup of the network this device is in, which marks its own paths | |

How to read it:

- A hostname is resolved to all its addresses and each is placed in its route. The first one that has a route is looked up in full, and the others are listed.
- RPKI belongs to a route: a prefix and one origin AS. The results are Origin authorized, Wrong origin ASN, Prefix too specific, No ROA found and Unable to check. No ROA found means no ROA covers the route, so RPKI says nothing about it.
- An AS path is what a collector peer or a looking glass sees from its own network. Your path lays the newest `rvrs traceroute` or `rvrs mtr` to the same target against those paths.
- No route observed means no collector reported the prefix. Collectors see only what their peers send, so the address may still be reachable.
- One data call that fails leaves its part empty and is listed as Unavailable under Sources. The lookup fails only when nothing answers.
- In a pipe or with `--classic` the lookup prints plain text that ends in one `;;` line per source with the time its data describes.
- The lookup exits 0 when it found a route or a network, 1 when no route was observed and 2 when it failed. `rvrs bgp check` exits 0 for Valid, 1 for Invalid or Not Found and 2 when the validator gave no answer.

### Looking glasses

A looking glass shows the routes its own routers hold. Add one and every `rvrs bgp` lookup of a route asks it for the prefix, beside the collectors. They are kept in `looking-glasses.json` in the config folder.

```bash
rvrs bgp glass add lg.example.net
rvrs bgp glass add lg.example.org --router "Amsterdam,Frankfurt"
rvrs bgp glass
rvrs bgp glass test lg.example.net 192.0.2.0/24
rvrs bgp glass remove lg.example.net
```

`glass add` tries each type in turn, or the one `--type` names (`alice-lg`, `lg-cgi`, `hyperglass`, `bird-lg-go`). A looking glass in front of up to four routers is added with all of them. With more, `glass add` lists them and takes the ones to ask with `--router`, since every router is one more request on every lookup. `glass test` asks one router for a prefix (1.1.1.0/24 unless you name one), prints each step, and prints the router's own output with `--raw`.

| Type | Read through | Example | Status |
| --- | --- | --- | --- |
| Alice-LG, an exchange's route servers | Its JSON API: accepted and filtered routes, and the origin AS's sessions with the route servers | lg.de-cix.net | Supported |
| lg.cgi, the classic Perl looking glass | The HTML form at `/`, `/lg.cgi` or `/cgi-bin/lg.cgi`, one request per router | lg.ip.tdc.net | Supported |
| hyperglass 1 and 2 | Its JSON API, text or structured routes | lg.nforce.com, lg.solnet.ch | Supported |
| bird-lg-go | Its pages around BIRD's `show route all` | lg.burble.com | Supported |
| bird-lg (the Python original), bgplg and custom forms | | | Not supported |
| A looking glass behind a CAPTCHA | | lg.twelve99.net | Not supported: rvrs does not work around a CAPTCHA |

Router output is read as Junos, Cisco or FRR, or BIRD. A looking glass that doesn't answer is listed as No answer and never fails the lookup.

## The report

`--json` prints the same `RunReport` the iOS app returns to Shortcuts. `measured`, `hops`, `records` (a DNS lookup's reference answer) and `outcome` hold what was measured (for a BGP lookup: the collector peers that see the prefix, the distinct paths and the origin AS number), `interpretation` what it likely means, and `sentence` the run in one line.

```bash
rvrs traceroute --json example.com | jq '.hops[] | {ttl, hostname, asn, avgMilliseconds}'
rvrs last --json | jq .sentence
```

## What leaves this device

- Probes and ping packets, to the target and the routers on the way.
- Reverse DNS for each hop, through the system resolver. `-n` turns it off.
- For each public hop address, a lookup of its network and location at stat.ripe.net (RIPEstat), cached for a week. Private addresses are never sent.
- One request to stat.ripe.net for this device's public address, shown in history as PUBLIC. The classic output skips it.
- The questions of `rvrs dig`: to this device's resolver, to 1.1.1.1, 8.8.8.8 and 9.9.9.9, and to the root, TLD and authoritative servers of the name. One question in dig's output goes to the one server asked.
- The requests of `rvrs bgp`, to stat.ripe.net: the address, prefix or AS number that was looked up (for a hostname, up to eight of the addresses it resolves to), the origin AS with up to 40 of its prefixes, and the AS numbers in the paths, for their holders' names. The prefix and the origin AS number also go to each of your looking glasses. `--no-glasses` leaves those out.
- Once a day, a look for a newer rvrs: a request to api.github.com for the newest release, or `git fetch` in the checkout rvrs was built from. It sends nothing about you or your runs. `RVRS_UPDATE_CHECK=0` turns it off.

`--no-lookups` turns off everything but the probes, the DNS questions and the requests a BGP lookup is made of. The CLI has no account and sends no usage data.

## Files

| What | Where |
| --- | --- |
| History, 500 runs at most | `~/.local/share/rvrs/history.json` |
| Saved runs | `~/.local/share/rvrs/saved.json` |
| Cases | `~/.local/share/rvrs/cases.json` |
| Favorites | `~/.local/share/rvrs/favorites.json` |
| Your looking glasses | `~/.config/rvrs/looking-glasses.json` |
| The Console's last tool | `~/.config/rvrs/console.json` |
| Network lookups, kept a week | `~/.cache/rvrs/networks.json` |
| What the last look for an update found | `~/.cache/rvrs/update.json` |

`XDG_DATA_HOME`, `XDG_CONFIG_HOME` and `XDG_CACHE_HOME` are honored. `RVRS_HOME=<dir>` puts everything in one folder. `RVRS_HISTORY=0` keeps a run out of history.

## Build

```bash
cargo build --release      # target/release/rvrs
cargo test                 # unit tests, no network
cargo clippy --all-targets
```

Needs Rust 1.82 or newer. On Linux, unprivileged ICMP sockets must be allowed. Most distributions allow them, and where they don't, rvrs names the setting (`net.ipv4.ping_group_range`). Run as root, or in a unikernel without unprivileged ICMP such as Unikraft, rvrs uses a raw ICMP socket instead.

### Versions

`VERSION` holds major.minor, and the build adds the number of commits since `VERSION` last changed (`crates/rvrs/build.rs`). `rvrs --version` prints the result and the commit, as in `rvrs 0.5.3 (4ea5d4e)`, with `-dirty` for uncommitted changes.

Copy `target/release/rvrs` to a folder on your `PATH`, such as `~/.local/bin`, and link it as traceroute, ping, mtr and dig as described under Install.

## Layout

```
crates/rvrs-core   engine, run model, report, history, lookups; no terminal code
crates/rvrs        the binary: flags, output, the Console, commands
install.sh         the installer behind rvrs.dev/install.sh
GLOSSARY.md        rvrs's terms
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in rvrs by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
