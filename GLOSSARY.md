# rvrs glossary

One word per thing, in every repo, document, screen, command and API field. The industry's own terms (hop, ASN, RTT, NXDOMAIN) and their casing are in the voice skill (`.claude/skills/rvrs-voice/SKILL.md`); this file names rvrs's own things. Identical copies live in the app (`../ios-app`), API (`../app`), CLI (`../cli`) and site (`../site`) repos. Change them together.

## Running things

| Term | Meaning | Not |
| --- | --- | --- |
| **tool** | One of traceroute, MTR, ping, iperf3, DNS and BGP. On the command line a tool is a subcommand named after the original (`rvrs traceroute`, `rvrs dig`). | feature, test type, check |
| **target** | What the person typed: a name or an address, with a port where the tool takes one. | destination (before it resolves), endpoint, URL |
| **host** | A name or address rvrs knows something about: a hop, a favorite, a resolver. A target is the host a run aims at. | node, server (unless it is one) |
| **destination** | The address a target resolved to, and the last hop of a path that reached it. | target (after it resolves) |
| **run** | One tool, against one target, from one origin, once. Everything rvrs saves, lists, shares and uploads is a run. A traceroute run may be called a trace, a DNS run a lookup and an iperf3 run a test where that reads better. The general word is always run. | job, session, measurement, check, scan |
| **result** | What a run measured, as a person reads it: the hop table, the chart, the records. | output (except for a command's text), response |
| **report** | A run as data (`RunReport`): `measured`, `hops`, `records` and `outcome` are observations, `interpretation` holds the inferences, `sentence` is one plain line. It is what `--json` prints, what Shortcuts receive, what the API stores and what MCP will return. | payload, export, result (for the data format) |
| **outcome** | How a run ended: reached, exhausted, stopped or failed. A run a node was asked for and would not make ended refused. | status, result |
| **state** | Where a run stands: requested, running, ended or expired. A run made on this device is ended as soon as it is kept. A run asked of a node waits (requested), is being made (running) and ends, or expires when the node never made it. | status, phase, progress |
| **status line** | The line of facts under a result's title, while the run is live and after it has ended: "Running · 4 hops so far", "Reached 1.1.1.1 · 6 hops · 10.3 ms". | headline, subtitle |
| **summary** | A run in one line of facts: "6 hops · 12.4 ms". | headline, title |
| **interpretation** | What a result likely means. Always worded as an inference and kept apart from what was measured. | insight (as a noun for output), diagnosis, analysis |
| **verdict** | An interpretation with a title: a few words that state the finding, then one or two sentences of evidence ("Stable", "Resolvers disagree"). | score, grade, rating |
| **continuous mode** | MTR or ping repeating until stopped. | live mode, repeat, monitoring |

## Inside a run

| Term | Meaning | Not |
| --- | --- | --- |
| **probe** | One packet a probing tool (traceroute, MTR, ping) sends and waits on. Ping and MTR count them as packets ("10 pkt"), as the original tools do. A probe is never a place. | a node, a measurement point |
| **round** | One probe to every hop, sent together. A traceroute sends a few rounds back to back, MTR one a second. | pass, cycle, run |
| **reply** | What comes back for a probe. No reply before the wait runs out is a loss. | response, answer (answers are DNS) |
| **wait** | How long a probe may take to answer before it counts as a loss. | timeout |
| **sample** | The RTT of one reply. | data point, measurement |
| **hop** | One router on the path, numbered by TTL. | node, step |
| **path** | The hops from the origin to the destination, in order. | route (a route is what BGP announces), trace |
| **network** | The autonomous system a host belongs to: its ASN and name ("AS13335 Cloudflare"). | provider, ISP (unless it is one), owner |
| **lookup** | Asking a directory about a name or address. A DNS run is a lookup. Reverse DNS and RIPEstat lookups add names and networks to hops and can be turned off. | enrichment, resolution |
| **source** | One resolver that answered in a DNS run: the device's, a public one or the authoritative one. | server, origin |
| **answer** | The records a source returned. | reply, response |

## Routing

The BGP tool's words. They are the industry's own; the table fixes which one rvrs uses where two exist.

| Term | Meaning | Not |
| --- | --- | --- |
| **route** | A prefix as one origin AS announces it. RPKI results belong to a route, never to a network or a company. | path (the hops of a run), announcement (the act) |
| **prefix** | A block of addresses in CIDR form, `203.0.113.0/24`. | subnet, range, network |
| **network** | An AS: who holds it, what it originates, who it is seen next to. Written `AS64500`. | ASN (that is the number), provider, company |
| **origin AS** | The network that announces a prefix. Always "origin AS" or "origin network"; a bare "origin" is where a run was made from. | origin, owner, source |
| **holder** | Who an AS number or prefix is registered to. It can differ from the brand that runs the network. | owner, operator |
| **collector** | A route collector (RIPE RIS): it records the routes its peers send. | probe, monitor, node |
| **collector peer** | A router that sends its routes to a collector. Visibility is counted in collector peers. | vantage point, probe |
| **looking glass** | A service that shows the routes its own routers hold. In rvrs, one the user added: it is asked for the prefix on every BGP lookup. Four types are read today: Alice-LG (an exchange's route servers), lg.cgi, hyperglass and bird-lg-go (an operator's own routers). | collector (those are RIPE RIS), LG (in copy), mirror |
| **route server** | A router at an internet exchange that passes members' routes on. A looking glass lists what its route servers hold; a route there is accepted or filtered, and one is the best. | collector, peer, server |
| **AS path** | The networks a route crossed from a collector peer to the origin AS. An observation from that peer, never this device's path. | route, trace |
| **origin validation** | The RPKI check of one route against the ROAs that cover it. Five results, in these words: Origin authorized, Wrong origin ASN, Prefix too specific, No ROA found, Unable to check. Badges say Valid, Invalid, Not Found, Unknown. | RPKI status of a network, secure, verified, safe |
| **ROA** | A signed statement that an AS may originate a prefix up to a maximum length. | certificate, authorization record |
| **upstream, downstream** | A network seen before or after another in AS paths. An observed position, worded as such. | provider, customer, peer (those are contracts) |

## Where a run happens

| Term | Meaning | Not |
| --- | --- | --- |
| **this device** | The phone, tablet or computer rvrs is running on, in the app and on the command line alike. The default origin. | local, localhost, this machine, client |
| **node** | A machine somewhere else that runs rvrs tools on request: a server, a Raspberry Pi, a router. It runs the same binary as the CLI (`rvrs node`), and `--node <name>` or the app's "Run from" picks one. It signs in with a token, and its runs belong to the workspace. | probe, agent, remote node, sensor, vantage point |
| **token** | A node's credential: one per node, made in the web app or received when the node enrolls, and revoked without touching another node. People sign in with a login. | API key, secret, password |
| **enroll** | A node joining a workspace by itself: it trades an enrollment token for a token of its own. | register, onboard, adopt, provision, activate |
| **enrollment token** | What a node enrolls with. It enrolls a set number of nodes before it expires and does nothing else. No image carries one. | join token, provisioning key, bootstrap token, claim code |
| **build feature** | A part of rvrs a build can leave out: a tool, the TCP probe, the path MTU check, the Console. A node says which ones it has. Every build has all of them unless it was made without some, and an update keeps what was left out. | capability, module, plugin, feature (for a tool) |
| **origin** | Where a run was made from: this device or a node, with its interface, source address, public address and network. | source (sources are DNS resolvers), vantage point |
| **fleet** | Every node of one workspace. | cluster, pool |
| **schedule** | A run a node repeats at an interval. The runs it makes are scheduled runs. | check, monitor, job |
| **alert** | The message sent when a scheduled run's result changes. | notification (those are the system's), alarm |

## Between a node and the API

The words of the node protocol, which is written down in the CLI repo (`schema/node-protocol`). People rarely read them on a screen. Routes and fields use them.

| Term | Meaning | Not |
| --- | --- | --- |
| **event stream** | The one connection a node holds open to the API. A node is online while it holds it. | socket, channel, tunnel, subscription |
| **wake-up** | The event that tells a node there is work. It says nothing about the work. | push, notification, signal |
| **claim** | A node taking the runs that wait for it. To claim. | pull, poll, fetch, pickup |
| **lease** | A node's hold on a run it claimed, for a limited time and renewed while it works. | lock, reservation |
| **attempt** | One claim of a run. A run whose lease ran out can be claimed again, as the next attempt. | retry, try |
| **heartbeat** | What a node tells the API about itself: its version, its build features and its limits. | ping (ping is a tool), keepalive, check-in |
| **outbox** | Where a finished run waits on a device until the API has it. | queue, buffer, spool |

## Keeping runs

| Term | Meaning | Not |
| --- | --- | --- |
| **history** | Every run kept, newest first: on the device, and for a person who is signed in also in their account, where it holds the runs of all their installs. The Console's Recent section shows the newest of it. | log, recents (as the name of the store), archive |
| **Recent** | The Console section that lists the newest runs in history. A section title, not a second store. | history (for the section) |
| **saved run** | A run kept on purpose, in a case or on its own. Saved runs do not expire with history. To save. | bookmark, pin, favorite |
| **case** | A named set of saved runs, from any tool and any origin, collected to look into one question: read together, stepped through, compared. A saved run can sit in several cases or in none, and a case holds any number of runs. | bookmark, group, folder, collection, investigation, incident, ticket, tag (a tag labels any run) |
| **favorite** | A host kept on purpose. A favorite is a host and never a run. | saved target, saved run |
| **library** | What a person keeps on purpose, as their account holds it: favorites, saved runs and cases. The same on every device they sign in on. A word of the API and the docs; a screen says Favorites, Saved or Cases. | collection, bookmarks, sync data |
| **tag** | A short label a person puts on a run to find it again or group it ("before-migration"). | label, category, folder |
| **comparison** | Two or more runs of the same tool side by side, often from one case: the same target from different origins, or the same origin at different times. | diff (a diff is what a comparison shows), benchmark |
| **account** | A person's rvrs identity. Accounts are optional and never needed to run a tool on this device. | profile, user (in copy) |
| **workspace** | What a team shares: its accounts, installs, nodes, runs, tags and schedules. A person alone has a workspace of one. | organization, org, team, tenant |
| **sign in** | Prose and buttons say "sign in" and "sign out". The command is `rvrs login`, as `gh` and `docker` spell theirs. | log in, authenticate (in copy) |
| **run page** | A run on rvrs.dev, rendered the way the app's result screen shows it. Only a run saved to an account has one. | permalink, report page |
| **share link** | The link to a run page. | URL, permalink |

## Surfaces

| Term | Meaning | Not |
| --- | --- | --- |
| **app** | rvrs for iPhone and iPad. | client, mobile app |
| **CLI** | The `rvrs` binary on macOS and Linux. | client, command-line tool (after first mention) |
| **Console** | Where a run starts: the first tab of the app, and the screen `rvrs` opens when run without a command. | home, dashboard, TUI (in copy) |
| **skill** | The agent skill `rvrs-network-diagnostics`: instructions that teach an AI agent with a shell to diagnose a network with the CLI and its history. It lives in the CLI repo (`plugins/rvrs`) and is also packaged as a plugin. | integration, extension, MCP (that is the server) |
| **web app** | rvrs.dev for people: history, favorites, run pages. | dashboard, portal |
| **site** | The public page about rvrs: what it is and where to get it. One page, with no accounts and no runs. | web app, landing page, homepage, website |
| **try shell** | The shell at `ssh cli.rvrs.dev`: the rvrs CLI on an rvrs server, open to anyone without an account. Its runs start from that server, and their history ends with the session. | demo, sandbox, playground |
| **admin app** | The operator's view in the API repo: pulses and the waitlist. | dashboard, backend |
| **API** | api.rvrs.dev. | backend, server (in copy) |

## Operating rvrs

| Term | Meaning | Not |
| --- | --- | --- |
| **install** | One copy of one product (the app, the CLI, a node) on one device, known only by a random ID made at first start. Every install is its own entry. Signing in attaches an install to an account, and the account's workspace groups them. The app and the CLI sign in with a login and act for a person: they send runs, read history and ask nodes for runs. A node signs in with a token and acts for its workspace: it sends runs and takes requests. | user, device (in counts), client |
| **small build** | rvrs without the Console, built for size, for servers and for routers with little flash. | node build, lite, minimal |
| **product** | Which rvrs an install is: `ios`, `cli` or `node`. Pulses and reports name it. | platform, client, surface |
| **pulse** | The anonymous daily signal that an install was used: product, version, system, opens, runs per tool. Never a target, address or hostname. | telemetry event, analytics, heartbeat, ping |
| **waitlist** | The list of email addresses waiting for accounts and nodes. | newsletter, signup |

## Rules

- A word in the "Not" column is not wrong English. It means something else here, or nothing. "Probe" is the sharpest case: RIPE Atlas calls its devices probes, rvrs calls them nodes, and a probe is always a packet.
- "Agent" means an AI agent that uses rvrs through the skill and the CLI, or later through MCP. A node is not an agent.
- "Monitoring" describes other products. rvrs has schedules and alerts.
- Type names in code may differ from the term for historical reasons (`TraceRun` holds a run of any tool, `RecentStore` holds history). Rename them when the code is touched for another reason. Prose, UI copy, flags, API routes and JSON fields use the glossary term from the start.
- A new concept gets a row here before it gets a screen, a flag or a route.
