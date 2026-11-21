# The Console

`rvrs` on its own opens the Console: the target field on top, Recent below it, and the result once a run starts.

```
 Origin:  this device                     ● traceroute   <↑↓>    Hop
 Source:  192.168.1.20                    ○ mtr          <↵>     Details
 Target:  github.com                      ○ ping         <c>     Copy Address
 Address: 140.82.121.3 · IPv4             ○ dig          <R>     Run Again
 Network: AS36459 GitHub, Inc.            ○ bgp          <r>     Run Tool
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
  ⇧1 Console   ⇧2 Favorites   ⇧3 Cases   ⇧4 Account  ›  traceroute
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
| ⇧1 ⇧2 ⇧3 ⇧4 | Console, Favorites, Cases, Account |
| ? | Shows every key and the latency colors |

⇧4 Account shows who this install is signed in as, also in the header. `s` signs in: the Console shows a code and a link, opens the link, and waits while you confirm the code in the browser, as `rvrs login` does. `o` opens the link again, `c` copies it, esc cancels. Signed in, `q` signs out and revokes this install's credential. With `RVRS_TOKEN` set, the token is the sign-in and the section only says so.

A ping in the Console sends 20 packets a second apart, as the app does. An MTR runs until `s` stops it. Each hop shows its figures and a strip of its newest rounds, and the charts above the table show the destination's latency, jitter and loss round by round. Jitter is how far each reply moved from the one before, and loss is counted over the last 10 rounds. A hop's details chart the same for that hop. Starting another run, or quitting, stops a live run and keeps it in history. On a terminal under 22 rows the header hides and the keys move to the bottom line. The Console starts with the tool chosen last.

A DNS lookup has the app's four tabs. Overview holds the verdict, the key records, the resolution path and what changed since the last lookup of the name. Records lists one source's answers by group. Compare lays the sources side by side with the reason for each difference. Trace shows the delegation path and the DNSSEC state. A record's details open beside the records with one sentence on what the record is, the tools to run against the host it names, Copy Value and Copy as Zone Line.

A BGP lookup has the app's tabs. Overview holds the verdict, the route with its origin AS and RPKI result, the networks it is reached through, your newest traceroute to the same target laid against the collectors' paths, recent changes and related prefixes. Paths lists every distinct AS path with the share of collector peers that see it, then what each of your looking glasses holds. Prefixes lists what the origin network announces, with a filter. History draws who announced the prefix and how many peers saw it over 7, 30 or 90 days. Sources says where each part came from and when it was observed. An AS number or a prefix typed under any tool runs as a BGP lookup. Return on a network or a prefix opens its own lookup, and a path opens beside the list network by network.
