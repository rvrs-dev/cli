# The Console

`rvrs` on its own opens the Console: the target field on top, Recent below it, and the result once a run starts. Keys come first, as in k9s: every key is a command until a tool's key opens the field.

```
t example.com ↵   trace the path to example.com
/ 64500 ↵         keep the hops with 64500 in an address, a name or a network
j                 the next of them
d                 its details beside the hops
```

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

Latency is green under 50 ms, yellow from 50 to 150 ms and red above, with the number always beside the color. In an MTR's charts jitter is magenta and loss is red. Loss in a table is red only where it reaches the destination. A middle hop that drops replies while later hops are clean is likely rate-limiting them.

| Key | Does |
| --- | --- |
| t, m, p, d, b | On a list: picks traceroute, MTR, ping, dig or BGP and opens the target field. In a result they act on the selected hop or record instead |
| : | Opens the target field for the tool chosen last, from a list or a result |
| Return | In the field: runs the tool against the target. `ping example.com` picks the tool too. With nothing typed, the field closes |
| typing | In the field: the target. Under the field it suggests hosts that hold what is typed: favorites, then hosts from history, the ones that start with it first. ↑ ↓ pick one, → completes the field with it, Return runs it. The list below stays as it is |
| / | Searches the list on screen: Recent, Favorites, Cases, a trace's or an MTR's hops (address, name or network), a DNS lookup's records, a BGP lookup's rows. The selection lands on the first hit, Return keeps the search, esc drops it |
| tab | In the field: the next tool. In a result: between the hops and a hop's details. In a DNS or BGP lookup: the next tab |
| 1 2 3 4 | In a DNS lookup: Overview, Records, Compare, Trace |
| o, z | In a DNS lookup: shows the next source's records, copies the selected record as a zone line |
| 1 2 3 4 5 | In a BGP lookup: Overview, Paths, Prefixes, History, Sources. For a network: Overview, Neighbors, Prefixes, Sources |
| o, v | In a BGP lookup: the next side of Neighbors, filter of Prefixes or range of History, the RPKI result of the selected prefix |
| ↑ ↓ or j k | Moves through the list, the hops a search left or a lookup's rows |
| g G | The first row, the last row |
| PgUp PgDn | Moves a screen at a time through the list, the hops or a lookup's rows |
| → or Return | Opens a run, a hop's details, a record's details, a case or a favorite. In a BGP lookup it opens the selected network's or prefix's own lookup, or an AS path's details |
| d | In a result: shows the selected hop's, record's, AS path's or RPKI result's details, and hides them again |
| ← or esc | Goes back one level. esc drops a search first, and on a list with nothing to take back it quits |
| t, m, p, b, n, w | On a hop: Run Traceroute, Run MTR, Run Ping, Run BGP, Copy Hostname, Whois |
| v | In an MTR: switches between each hop's figures and a wide strip of its rounds |
| c | Copies the address of the selected hop or the ping target, the selected record's value, or the selected row of a BGP lookup |
| r | Opens the run picker with every tool that can take the selected run's target, hop, record, network or prefix. Return runs the selected tool, and its letter (t, m, p, d, b) runs it at once |
| R | Runs the open run again |
| s | Stops the run while it runs. Once it has ended, saves it or removes it from saved runs |
| a | Adds the run to cases |
| S | Shares the run, signed in: the share sheet says what the link does and has Private Networks, Public Address and Device Name to switch, starting at the workspace's defaults, and Return makes the link. Then `o` opens it, `c` copies it and `x` stops sharing |
| f | Adds the host to Favorites or removes it |
| x | Takes a run out of the open case |
| ⇧1 ⇧2 ⇧3 ⇧4 | Console, Favorites, Cases, Account |
| ? | Opens the help on the page for what is on screen. A tool's page has the keys its result takes and the commands `rvrs <tool> --help` lists. tab turns the page, a tool's letter opens its page, `?` the Console's keys, esc closes it |

⇧4 Account shows who this install is signed in as, also in the header. `s` signs in: the Console shows a code and a link, opens the link, and waits while you confirm the code in the browser, as `rvrs login` does. `o` opens the link again, `c` copies it, esc cancels. Signed in, `q` signs out and revokes this install's credential. With `RVRS_TOKEN` set, the token is the sign-in and the section only says so.

`S` on a run, in a list or open, opens the share sheet. It first reads the run's link and the workspace's rules for links, and nothing is made until Return. A run without a link: the sheet says that the link makes a public page of the run that anyone with the link can open without signing in, and, in a workspace whose links expire, after how many days the link stops working. It lists what the run has for each switch, such as the router's address beside Private Networks. The switches start at the workspace's defaults, which are Private Networks on and Public Address and Device Name off until an owner or an admin changes them. Return generates the link and copies it. A shared run opens on its link, with the day it stops working when it expires and the switches where the link has them: `o` opens it, `c` copies it, Return after a switch moved changes what it shows, and `x` asks once, then stops sharing, and the link stops working for everyone. In a workspace where only owners and admins share, a member reads "Only owners and admins of this workspace share runs." in place of the switches, and can still open, copy and stop a link they made before.

A ping in the Console sends 20 packets a second apart, as the app does. An MTR runs until `s` stops it. Each hop shows its figures and a strip of its newest rounds, and the charts above the table show the destination's latency, jitter and loss round by round. Jitter is how far each reply moved from the one before, and loss is counted over the last 10 rounds. A hop's details chart the same for that hop. Starting another run, or quitting, stops a live run and keeps it in history. On a terminal under 22 rows the header hides and the keys move to the bottom line. The Console starts with the tool chosen last.

A DNS lookup has the app's four tabs. Overview holds the verdict, the key records, the resolution path and what changed since the last lookup of the name. Records lists one source's answers by group. Compare lays the sources side by side with the reason for each difference. Trace shows the delegation path and the DNSSEC state. A record's details open beside the records with one sentence on what the record is, the tools to run against the host it names, Copy Value and Copy as Zone Line.

A BGP lookup has the app's tabs. Overview holds the verdict, the route with its origin AS and RPKI result, the networks it is reached through, your newest traceroute to the same target laid against the collectors' paths, recent changes and related prefixes. Paths lists every distinct AS path with the share of collector peers that see it, then what each of your looking glasses holds. Prefixes lists what the origin network announces, with a filter. History draws who announced the prefix and how many peers saw it over 7, 30 or 90 days. Sources says where each part came from and when it was observed. An AS number or a prefix typed under any tool runs as a BGP lookup. Return on a network or a prefix opens its own lookup, and a path opens beside the list network by network.
