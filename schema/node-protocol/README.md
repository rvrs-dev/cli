# The node protocol

How a node talks to the rvrs API, and how a person's install asks a node for a run. `openapi.json` in this folder names every route and field, and `fixtures/` holds one example of each body. This page says what the routes are for, who moves a run from one state to the next, and what each side does when something gets lost.

Status: version 1, at revision 2. The rvrs API serves it, except enrolling, and `rvrs node` speaks it since rvrs 0.15. Revision 2 added schedules, which `rvrs node` keeps since rvrs 0.17. Anyone may build a controller or a node that speaks it.

## Who speaks

| Who | Signs in with | May |
| --- | --- | --- |
| A node (`rvrs node`) | Its own token | Send its runs, claim the runs asked of it, read its own schedules and say what it is. It reads no history, no library and no other node |
| Whatever sets a node up | An enrollment token | Enroll nodes, and nothing else |
| A person's install (the CLI, the app) | A login | Ask a node of its workspace for a run, read where that run stands, and make and change schedules. It never holds the event stream and never claims |

A credential goes in `Authorization: Bearer <token>`. The node opens every connection, over HTTPS, so a node behind NAT needs no open port and no address of its own.

## The routes

| Route | From | Does | A retry |
| --- | --- | --- | --- |
| `POST /v1/nodes` | An enrollment token | Enrolls a node and answers with its token | Safe. The same `key` gets the same node and the same token |
| `POST /v1/nodes/me/heartbeats` | A node | Says what the node is: version, build features, limits, outbox | Safe. Each one replaces the last |
| `GET /v1/nodes/me/events` | A node | The event stream, which wakes the node when there is work | Safe. A new stream replaces the old one |
| `POST /v1/nodes/me/claims` | A node | Takes the runs that wait for the node and keeps the ones it holds | Safe with its `key`. The same claim sent again gets the same runs as the same attempts |
| `POST /v1/runs` | A node | Sends a finished run, with `claim` when the run was asked for | Safe. A run is told apart by the node and the report's `id` |
| `GET /v1/nodes` | A person's install | Lists the nodes of its workspace, with whether each is online | Safe |
| `POST /v1/run-requests` | A person's install | Asks a node for a run and answers with the run's id | Safe with a `key`. Without one a retry asks twice |
| `GET /v1/run-requests/{id}` | A person's install | Where the run stands, and its report once it ended | Safe |
| `GET /v1/nodes/me/schedules` | A node | The node's copy of its schedules, with when each one starts | Safe |
| `GET /v1/schedules` | A person's install | Lists the schedules the person made | Safe |
| `POST /v1/schedules` | A person's install | Makes a schedule | Safe. A schedule that says the same as one the person has is that one |
| `GET /v1/schedules/{id}` | A person's install | One schedule | Safe |
| `PATCH /v1/schedules/{id}` | A person's install | Changes a schedule, pauses it or resumes it | Safe. The same change twice leaves the schedule as the first did |
| `DELETE /v1/schedules/{id}` | A person's install | Deletes a schedule and keeps its runs | Safe. The second answer is 404 |

`POST /v1/runs` is the route every install already sends its runs through. A node uses the same one.

## Rules for both sides

- Fields are only added. Each side ignores a field, an event and an error code it doesn't know. A change that would break a reader gets a new path, `/v2`, and the API serves both while nodes update.
- Words that describe something grow too: a run's `state`, a `reason`, a report's `outcome`, a build feature. A reader takes a word it doesn't know, keeps it as it is and assumes nothing from it. An install that reads a state it doesn't know shows the word and keeps reading until the report is there or `expiresAt` has passed.
- Words that tell a node what to do are another matter. The API sends a node only the ones its `protocol` revision has, and a node never guesses at one. A verdict it doesn't know stops the run. A tool or an option it doesn't know makes it refuse the run, because running without it would measure something else than what was asked.
- Times the node acts on are lengths in seconds, never moments. A router's clock can be wrong before it has synchronized, and the protocol doesn't depend on it.
- Every answer that is not 2xx has the one error shape, `{ "error": { "code", "message" } }`. A reader branches on `code`.
- An answer without that shape didn't come from the API. A load balancer or a proxy on the way answers with a page of its own, such as a 404 when no server behind it has room. A node treats an answer without the API's error, whatever its status, as it treats a 5xx. A 2xx is read the same way: every answer the API gives a node is a JSON object, and a page with a good status, as a captive portal sends, is no answer.
- No heartbeat, event or log line carries a credential, a target or anything from a report.

## Tokens and enrolling

A node's token is its own: one per node, stored on the node, revoked alone. It comes from one of two places. A person makes a node in the web app and gives the node its token (`RVRS_TOKEN`). Or the node enrolls.

An enrollment token enrolls a set number of nodes before it expires and does nothing else. No image carries one. Whatever sets the device up writes it at first boot: cloud-init, an installer, an ISP's auto-configuration server. So a router imaged today can enroll weeks later, and an image found in the wild enrolls nothing.

To enroll, the node makes a random `key` of 128 bits or more from the system's random source, keeps it, and sends it with the name it proposes:

```
POST /v1/nodes
Authorization: Bearer <enrollment token>

{ "key": "5tq0yM3VfJm0b6mVq1mW4w", "name": "cpe-4471", "version": "0.14.2" }
```

The answer carries the node, its workspace and the node's token. The node stores the token next to the key before it uses it, and forgets the enrollment token where it can.

- Enrolling answers the same every time. Under one enrollment token, the same `key` is the same node and gets the same token: 201 the first time, 200 after that, with the same body. No enroll revokes or replaces a token. So answers that come late, twice or out of order all say the same thing, and the node can keep whichever it reads.
- Two enrolls with one `key` at the same moment make one node. The enrollment token counts it once.
- That holds for as long as the enrollment token is neither expired nor revoked. A token that has enrolled every node it may still answers the keys it knows.
- A `key` belongs to the enrollment token it was first sent with. The same `key` under another enrollment token is another node.
- A node with a new `key`, after a factory reset, is a new node.
- No route of this protocol changes a node's token, and enrolling never rotates one. A person can replace a node's token where the node is managed, in the web app. The node stays the node it was, with its id, its name, its tags and its runs. The token it had answers 401 from that moment, and the node is given the new one the way it was given the first.
- A node that was revoked is gone for good. Its name is free at once, so a new node can take it, with a new `key` when it enrolls and an id of its own.
- A node's name is unique in its workspace: 1 to 63 lowercase letters, digits, `.`, `-` and `_`. It is what `--node` takes. The API makes it from what the node proposes, and a person can change it later. The node's `id` never changes.
- 410 `ENROLLMENT_CLOSED` says the enrollment token expired, was revoked, or has enrolled every node it may and doesn't know this `key`. 403 `NODE_REVOKED` says this `key` belongs to a node that was revoked. The node stops enrolling after either.
- A node without a token and without an enrollment token makes no call at all. The API's default address alone never enrolls a node and never receives a run.

## The heartbeat

A heartbeat says what the node is. The API learns from it what to show a person and what the node can run.

| Field | Holds |
| --- | --- |
| `version` | The node's rvrs version |
| `protocol` | The revision of this protocol the node knows, `2` today. A verdict or an event the node has to act on raises it, and the API sends a node none its revision lacks. Revision 2 is a node that keeps schedules |
| `system` | `os` and `arch`, as in `linux` and `aarch64` |
| `boot` | A random word the node makes each time it starts, so the API sees a restart |
| `features` | The build features the node has: `traceroute`, `ping`, `mtr`, `dns`, `bgp`, `iperf3`, `tcp` (the TCP probe), `mtu` (the path MTU check), `console` |
| `limits` | What the node allows itself, whatever it is asked: `runsAtOnce`, `runsPerMinute`, `packetsPerRun` (probes, which a DNS or a BGP lookup sends none of), `secondsPerRun` (every tool), `reportBytes` |
| `outbox` | `waiting` (runs not sent yet), `dropped` (runs given up since this start) and `keeps` (`disk` or `memory`) |
| `tags`, `location` | What the node declares about itself, both optional. See Tags and location |

The answer names the node and its workspace as the API knows them, so a node that was renamed on the web learns its name, and every node learns its `id`.

The answer may also name `events`, the address of the event stream. An API can serve its streams apart from its other routes, so that releasing the API doesn't end every stream, and moving them needs no update to any node. A node opens its stream at `events` when an answer names it, and at `/v1/nodes/me/events` on the API's address when none has. It keeps the address until a later answer names another, and sends its token there as it does to the API. An address whose scheme differs from the API's is ignored.

The answer names `schedules` too, the version of the copy of its schedules the node should hold ([Schedules](#schedules)). An API without schedules leaves it out.

A node sends one when it starts, when one of these facts changes, and every `heartbeatSeconds`. Whether a node is online is not the heartbeat's to say. A node is online while it holds its event stream.

## Tags and location

Every install can carry tags, and a node can say where it is. A node declares both in its own configuration ([`../node-config.md`](../node-config.md) says how a person or a package writes them) and sends the declaration in the enroll and in every heartbeat. A node only declares. A person edits on top of it in the web app, and the API works out the tags and the location that count.

A tag is a key and a value, as in `isp=telia`. On the wire the tags are one JSON object from key to value, 20 at most. A key is a lowercase letter followed by lowercase letters, digits, `.`, `-` and `_`, 63 characters at most (`^[a-z][a-z0-9._-]{0,62}$`). A value is the same and may start with a digit (`^[a-z0-9][a-z0-9._-]{0,62}$`).

A location is `lat` and `lon` in decimal degrees, a `place` of 1 to 100 characters such as `Esbjerg, DK`, and a `precision`. `exact` keeps 4 decimals (about 10 m), `area` keeps 2 (about 1 km) and `city` keeps 1 (about 10 km). `city` is the default. The precision is a bounded string and never a closed list, since the list may grow. A reader that doesn't know a word treats it as `city`, the coarsest. The API rounds the coordinates to the precision before it stores them, so a node may send more digits than the precision keeps and never gets more back.

- A heartbeat or an enroll that leaves out `tags` or `location` changes nothing. An older node says nothing, and the API keeps what it had.
- `tags: {}` says the node declares no tags.
- The node sends its whole declaration every time it sends one, never a difference.
- An enroll sets the declaration only when it makes the node, which is the 201. The heartbeat speaks for the node from then on, so enrolls that arrive out of order can't undo each other.
- A tag set on the web is never undone by a declaration. The API keeps what the node declared apart from what a person edited and works out the tags that count: the declaration with the edits applied. A location works the same way. A node that restarts, reconnects or declares again leaves a person's edits where they are.
- A run's report carries a snapshot of its origin as it was when the run was made: `origin.tags` and `origin.location`, the second already rounded. Every change to an install's tags or location, from a declaration or from an edit on the web, makes a revision, a number that goes up by one, and the API keeps what each revision said. The enroll and heartbeat answers carry the current one as `node.metadataRevision`.
- A node writes the revision it held when the run started into `origin.metadataRevision` of every report it makes, and keeps it with the report in its outbox. A tag edited while a long MTR runs is no part of that MTR. A run that waits a day for the uplink is therefore made under the revision of the day it ran. A run the API handed out takes its revision when the API gives it out, which the node doesn't have to do.
- The API checks the revision and writes `tags` and `location` itself. The node never says what its tags are, only which revision it worked under: a revision that isn't the node's own, including another node's, or that nobody made, is ignored, and the run takes the revision that was current when it started (by `startedAgoMilliseconds`, which the API can trust, and for a node that sent neither, when the result arrived). Whatever tags and location the report carries are replaced.
- Who made the run is the token's alone. The API never takes the node's name or id from the report, whatever it says in `origin`.
- Changing a node's tags or location afterwards leaves every earlier run as it was. A report from before this has neither.

## The event stream

`GET /v1/nodes/me/events`, or the address the heartbeat's answer names, answers with Server-Sent Events and stays open.

| Event | Data | The node |
| --- | --- | --- |
| `hello` | `keepAliveSeconds`, `claimSeconds`, `heartbeatSeconds`, `reconnectSeconds` | Takes the numbers, then claims and sends what waits in its outbox |
| `work` | `{}` | Claims now |
| `schedules` | `{}` | Reads its copy of its schedules now |
| `: keep-alive` (a comment line) | | Knows the stream is alive |

- An event says there is work and nothing about it. It carries no run's parameters and nothing secret. What the work is comes from the claim.
- `work` is repeated. While a run waits for a node that holds a stream, the API writes `work` again every 15 seconds until the node has claimed the run. A wake-up that got lost is followed by the next one.
- `hello` is the first event of every stream. Its numbers are the API's to choose, so a fleet can be slowed down without updating a node.
- A stream that stays silent for twice `keepAliveSeconds` is dead. The node closes it and opens a new one. Closing means giving up the connection, so a stream that died without a word holds no socket on the node.
- When a stream ends or can't be opened, the node waits a random time up to `reconnectSeconds` and tries again. Every failure in a row doubles that window, up to 300 seconds, and a `hello` sets it back. So an API release doesn't bring every node back in the same second.
- A node holds one stream. A second one with the same token replaces the first.
- `schedules` says the node's schedules changed and nothing about them. The API writes it to nodes at revision 2 and up, and writes it again every 15 seconds until the node has read its copy.

## Claiming

A wake-up can get lost, so no run depends on a single one. A node claims:

- after every `hello`, which is every time it connects,
- after every `work`, which the API repeats while a run waits,
- every third of `leaseSeconds` while it holds a run,
- after an attempt to open the stream that failed, so a node whose network lets no stream through still gets its runs, later,
- and as the last net while it has nothing to do, at a random time between half of `claimSeconds` and all of it.

A claim says everything the node holds and how much more it will take:

```
POST /v1/nodes/me/claims

{ "key": "c7m2q9xk", "room": 1, "holding": [ { "run": "0e6d2f0a-8c0e-4f58-9a55-0b8f3f6f1c11", "attempt": 1 } ] }
```

`holding` lists every run the node claimed and the API hasn't answered a result for: the ones still probing and the ones whose result waits in the outbox. `room` is how many more runs the node will start now, within its own limits. `key` is a random word the node makes for this claim, 1 to 64 characters, and sends again with every retry of it.

The answer has three parts.

- `runs`: the runs the node now holds for the first time, `room` of them at most, oldest first. Each has its `run` id, its `attempt`, and the `tool`, `target` and `options` to run.
- `holding`: a verdict for each run the node listed. `keep` means the lease is renewed. `stop` means a person stopped the run: the node ends it now and sends what it measured, with the outcome `stopped`. `drop` means a newer attempt has the run or the run has its result: the node stops and sends nothing.
- `leaseSeconds`: how long each run in `runs` and each `keep` is the node's from now.

A verdict the node doesn't know is never read as `keep`. The node stops that run at once, counts its lease as over, and sends what it measured with the outcome `stopped` and an `error` that names the verdict. The API takes or refuses that result by the rule for any result. So a newer API that means to stop or restrict a run is obeyed by an older node in the part that matters: the probing ends.

A run the API believes the node holds, and the node didn't list, is free again at once. That is how a restart is noticed: a node that starts up claims with an empty `holding`, and the runs it lost come back in the same answer as a new attempt.

A lost answer looks the same from the API's side: the node claimed, the answer never reached it, and it asks again without listing the runs that answer held. The `key` tells the two apart. The API keeps the key of the claim that gave each run out. A claim that arrives with that key again is a retry, and its runs come back as the same attempts, with their leases started over. So a node on a line that drops answers spends no attempt on them. A node makes a new key for every new claim and after every start, which is why a restart still frees its runs. A claim without a key can't be told from a restart, and the runs it doesn't list come back as new attempts.

A node sends one claim at a time and waits for its answer before the next. Two claims in flight could leave out a run the first one was about to hand over, and the node would get it twice. Should a node ever hold two attempts of one run, the higher one is the run and the lower one is dropped.

A lease is a hold for a limited time. A node that goes silent loses it after `leaseSeconds`, and the run may be claimed again. Every claim of the same run is a new attempt: 1, 2, 3. The API gives a run out three times at most and never after the request expired.

Before it starts a run, the node checks it against what it is and what it allows: the tool and every option are ones its build knows, the run fits its limits, and the target is one it may probe. A run that fails the check is not started. The node sends a report with the outcome `refused` and the reason in `error`, through the same route as any result.

[What a node does and what it refuses](../../docs/node-safety.md) has the rules of that check: the limits and their ceilings, the address space a node sends nothing to, and the ranges its owner can allow. The node keeps them itself. No claim, event or answer of the API carries one, and `room` is how the node says how many runs it will take.

### What a run is

| Field | Holds |
| --- | --- |
| `tool` | `traceroute`, `ping`, `mtr`, `dns`, `bgp` or `iperf3`: the word a build names the tool by in `features` |
| `target` | What the person typed |
| `options` | What the person set. An option left out takes the tool's default |

| Option | Tools | The CLI's flag | Holds |
| --- | --- | --- | --- |
| `family` | traceroute, ping, mtr | `-4`, `-6` | `IPv4` or `IPv6` |
| `rounds` | traceroute, mtr | `-q`, `-c` | Rounds to send |
| `packets` | ping | `-c` | Packets to send |
| `intervalMilliseconds` | ping, mtr | `-i` | The time between packets or rounds |
| `waitMilliseconds` | traceroute, ping, mtr | `-w`, `-W`, `-Z` | The wait for a reply |
| `maxHops` | traceroute, mtr | `-m` | The last hop to probe |
| `firstHop` | traceroute, mtr | `-f` | The first hop to probe |
| `probe` | traceroute | `-T -p` | `{ "protocol": "tcp", "port": 443 }` probes with TCP |
| `mtu` | traceroute | `--mtu` | `true` finds the path MTU once the trace is done |
| `lookups` | traceroute, ping, mtr | `-n` | `false` leaves out reverse DNS and network lookups |
| `type` | dns | The type | The record type, `A` when left out |
| `resolver` | dns | `--resolver` | A resolver of the person's own, asked beside the others |

A BGP lookup takes no options. iperf3 gets its options when the CLI has the tool.

A run on a node always ends on its own. Ping and MTR, which go on until stopped on a device, send 10 packets or rounds when the request names no number.

### How a lost wake-up is recovered

| What was lost | Recovered by | Within |
| --- | --- | --- |
| A `work` event, or the notice inside the API behind it | The API writes `work` again | 15 s |
| The stream, without the node knowing | The node misses its keep-alives, reconnects and claims | 50 s plus its reconnect wait |
| Every stream, on a network that lets none through | The node claims after each failed attempt to open one | Its reconnect window, 300 s at most |
| Anything else | The node's idle claim | `claimSeconds`, 1,800 s |

A request waits 300 seconds for its node unless the asker says otherwise, which covers the first three. `expiresIn` can be as short as 30 seconds. Such a request outlasts a lost event and can expire when the stream was dead. `claimSeconds` may be longer than any request waits, and that is on purpose: the idle claim is the net under the other three, and no run is promised through it.

## Sending a result

A finished run goes to `POST /v1/runs` as its report. A run that was asked for carries the claim it was made under:

```
POST /v1/runs

{ "report": { ... }, "claim": { "run": "0e6d2f0a-8c0e-4f58-9a55-0b8f3f6f1c11", "attempt": 1 }, "startedAgoMilliseconds": 14412 }
```

- The report's `origin.kind` is `node`. `origin.nodeId` is the node's id, which never changes. `origin.node` is the node's name. The API writes all three from the token ([Who made a run](#who-made-a-run)). The report's `id` is the node's own and never repeats on that node, for as long as the API keeps its runs. `rvrs node` writes the run's id and the attempt, as in `0e6d2f0a-8c0e-4f58-9a55-0b8f3f6f1c11-1`. A node that makes its ids at random needs 128 bits of them.
- A node sends the report alone, without the `detail` a CLI adds for its other installs.
- The API takes a result when its attempt is the run's newest and the run has no result yet. Whether the lease ran out in between doesn't matter: a node that lost its uplink during an MTR still delivers that MTR, which is the run someone wanted.
- 409 `CLAIM_LOST` says a newer attempt has the run, or the run already has its result. The node drops the result. So a node that hung can't overwrite a newer result, and a run made twice is stored and counted once.
- 201 says the API stored the result, and its answer names the run: `id` is the run's id. A node keeps a result in its outbox until an answer names its run, and sends it again after any other 2xx.
- 200 says the API had this report already, for this run and this attempt. Nothing changed.
- 422 says the report is not the run that was asked for: its `tool` is another tool than the request named, without regard to case, or its `target` is another target. A node writes both as it was given them, also in a report with the outcome `refused`.
- 422 also says the report's `id` is one the node used for another run. An id names one run on its node, so the API never answers for a run with another run's report.
- A report without `claim` is a run the node made by itself. A scheduled run names its schedule beside the report, as `schedule` ([Schedules](#schedules)).
- A node with many results waiting sends them between its claims, never in place of one. A node that comes back after a day without its uplink still claims every third of the lease, so the runs it is making keep theirs.

### Who made a run

The API knows a node by its token and by nothing in a body. It stores a run under the node it authenticated, answers 422 when `origin.nodeId` names another node, and never looks a node up by a name it read in a report.

Who made a run is the API's to say, so it writes that part of the report it stores: `origin.kind` is `node`, `origin.nodeId` is the authenticated node's id and `origin.node` is that node's name as the API has it when the result arrives. Whatever the report said in those three is replaced. A node that left them out, named another node or called itself a device is stored, listed and compared as the node it is. The rest of the origin is the node's own word and is kept.

A node's name can change and its id can't. A run keeps both: the id, so runs of one node compare across a rename, and the name the node had when the API took the run. A run that waited in an outbox across a rename carries the new name. Renaming a node changes no stored report.

### Whose clock

| Time | Whose clock | Holds |
| --- | --- | --- |
| `startedAt` in a report | The node's | When the run started, as the node believed. A router's clock can be years off before it has synchronized |
| `requestedAt`, `expiresAt`, `claimedAt`, `receivedAt`, `lastSeenAt` | The API's | When the API saw each thing happen |
| `observedStartedAt` | The API's, worked out | `receivedAt` minus `startedAgoMilliseconds`. `null` when the node didn't say |

`startedAgoMilliseconds` is how long before this request the run started, by a clock on the node that only counts forward. It needs no synchronized clock, and it stays right when a run waited in the outbox for hours. A node that restarted since the run leaves it out, since that clock started over.

Beyond who made it, the report is never rewritten. `startedAt` stays the node's word. A reader that orders runs, charts them or compares them across origins goes by the best of three, in this order:

1. `observedStartedAt`, where there is one.
2. `startedAt`, where it is believable: no more than 8 days before `receivedAt`, which is as long as an outbox keeps a run, and not after it. A node that restarted with runs in its outbox can say no more than this.
3. `receivedAt`, where `startedAt` is not believable. The run started before it arrived, so this is its latest possible moment. A reader says so where it shows the time.

So a run from a router whose clock says 1970 is placed in the hour it arrived in and is never lost to 1970. For a run that was asked for, `claimedAt` and `receivedAt` bound it either way: it started after the first and ended before the second.

A router that booted without a synchronized clock may report `startedAt` as `1970-01-01T00:04:10Z`. With `startedAgoMilliseconds` of 14,412 and a result received at `2026-11-22T20:15:16.412Z`, the run's `observedStartedAt` is `2026-11-22T20:15:02.000Z`.

## Schedules

A schedule is a run a node repeats at an interval: a tool, a target, options, an interval and the nodes it runs on. A person makes it, and each node keeps the schedules it runs and repeats them by itself. So a node goes on measuring while the API is out of reach, and the API starts no run of a schedule.

### Making one

```
POST /v1/schedules

{ "name": "Evening jitter", "tool": "ping", "target": "example.com", "options": { "packets": 10 }, "intervalSeconds": 60, "nodes": ["fra-1", "cpe-4471"] }
```

- `intervalSeconds` is 60 to 86,400.
- A schedule says where it runs in one of three ways: `nodes` names up to 50 nodes by name or id, `selector` takes every node whose tags match, and `fleet: true` takes every node of the workspace. A body with none of them, or with two, is answered 422, and so is an empty `nodes`. The field `fleet` keeps the name it shipped with: in the glossary a fleet is the nodes a selector matches.
- A selector is terms joined by commas, which mean and: `isp=telia`, `isp!=telia`, `isp` (has the tag), `!isp` (lacks it), `line in (ftth,cable)` and `line notin (dsl,lte)`. A node without the tag meets `!=` and `notin`. `GET /v1/nodes` and `GET /v1/runs` read `tag` the same way.
- A selector, and every node, are read again whenever nodes or their tags change. A node that gains the tag starts the schedule, and one that loses it stops.
- A schedule's tool and target never change. Another target is another schedule, so a schedule's runs are one series.
- `PATCH /v1/schedules/{id}` changes `name`, `options`, `intervalSeconds`, where it runs and `paused`. A paused schedule runs on no node and keeps its runs. A run of it that is going on a node is stopped when the node reads its copy.
- Making a schedule that says the same as one the person has, in tool, target, options, interval and where it runs, answers 200 with that one. Two of them would send every run twice.
- A schedule belongs to the person who made it, and so do its runs. Anyone else gets 404 `SCHEDULE_NOT_FOUND`.
- 404 `NODE_NOT_FOUND` says `nodes` names a node the workspace doesn't have, and `details.node` is the name it didn't find. 403 `SCHEDULE_LIMIT` says the workspace has as many schedules as it may.
- Deleting a schedule ends it on every node and keeps the runs it made.

The answer is the schedule as `GET /v1/schedules/{id}` gives it. `nodes` lists the nodes it names, with the name each has now. `nodeCount` is how many nodes it runs on at the moment. `everySeconds` is the interval the nodes keep, which is `intervalSeconds` unless the cap below made it longer. `lastRunAt` is when the API last stored one of its runs.

### Spread in time

A schedule on a thousand nodes must never send a thousand runs in the same second. The API gives every node a place of its own in the interval.

- For each schedule and node the API works out an offset into the interval, from the two ids. It is the same every time, and the offsets of many nodes fill the interval evenly.
- A node gets no moment, since its clock can be wrong. It gets `startsInMilliseconds`: how long after this answer its next run of the schedule starts. From there it repeats every `everySeconds`, by a clock that only counts forward.
- One target gets about 10 scheduled run starts a second from one workspace, across every schedule that names it. When the schedules for a target would start more, the API makes each of them longer by the same factor and hands out the longer interval as `everySeconds`. 3,000 nodes that ping one target every 60 seconds would start 50 runs a second, so each node runs every 300 seconds. `intervalSeconds` stays what the person asked for.
- That number is a budget for the average and no ceiling for a single second. A hash places the nodes, so some seconds hold more than their share: of 6,000 nodes on 600 seconds, the busiest second holds about 20. A node that is cut off keeps its place by its own clock and drifts until it reads its copy again. A controller that has to promise a ceiling hands out the places itself.
- The API works `everySeconds` out again when nodes join or leave a selector, and the nodes whose copy changed read it again.

### The node's copy

```
GET /v1/nodes/me/schedules

{ "version": "5d0c2a9e41b7f630", "keepSeconds": 604800, "schedules": [ { "schedule": "7a1c9e52-3b4d-4f6a-8c0e-2d5f7b9a1c3e", "tool": "ping", "target": "example.com", "options": { "packets": 10 }, "everySeconds": 60, "startsInMilliseconds": 41250 } ] }
```

The answer is the node's whole copy: every schedule that runs on this node and isn't paused, 50 at most, each with what to run as a claim gives it. `version` changes when the tool, target, options or `everySeconds` of any entry does, and when an entry comes or goes. `startsInMilliseconds` is different in every answer and is no part of it.

A node reads its copy:

- after every `hello`, which is every time it connects,
- after every `schedules` event,
- when a heartbeat's answer names a `schedules` version other than the one it holds,
- and once when its copy is older than it is good for, or the hour it waits after a 403 is over, so a node whose network lets no stream through still gets a copy.

What it does with it:

- The answer replaces the copy the node held, whole. A schedule that is no longer in it ends at once: no further run of it starts, and a run of it that is going is stopped. What that run measured is sent with the outcome `stopped`, as a run a person stopped is. So pausing or deleting a schedule ends its packets on every node that hears of it, and a schedule that only changed its interval or options keeps the run it is making.
- The node keeps the copy where it keeps its outbox. A node with a disk starts from it after a restart.
- Every answer sets each schedule's next start anew, to `startsInMilliseconds` from the answer. The API works out the same place in the interval each time, so a schedule that didn't change keeps its rhythm and a node's drifting clock is put right. A start that would come less than half of `everySeconds` after the schedule's last one is left out.
- A node that starts from its kept copy without an answer starts each schedule at a random time within its `everySeconds`.
- A run is due once. When the node is at one of its limits, or the schedule's last run is still going, the run is left out. A node never makes up the runs it missed, after a pause, a restart or a day without power.
- A scheduled run counts against the node's limits like any other run, and is checked like one: the tool, every option, the limits and the target. A schedule that fails the check is refused once: the node sends one report with the outcome `refused` and starts no run of it until a copy says something else about that schedule or the node restarts.
- A copy is good for `keepSeconds` after its answer. After that the node starts no scheduled run until it has read the copy again. The time is the outbox's 7 days: a run older than that would never be sent.
- A restart never makes a copy fresh. The node keeps the copy's age with it: how long it has run with this copy, written down every few minutes, and when the answer came by its own clock. After a restart the age is the longer of the two: the time it ran, with a few minutes more for what a crash could have lost, and the time its clock says has passed, where the clock moved forward. A clock that reads earlier than the answer adds nothing. A kept copy without its age is no copy, and the node starts without schedules.
- 401 ends the node's schedules and its copy. After 403 `FLEET_STOPPED` the node keeps the copy and starts no run of it until it has read the copy again.
- An API from before schedules answers 404. The node holds no schedules then, and asks again after its next `hello`.

### A scheduled run

A scheduled run is sent like any run the node made by itself, with the schedule's id beside the report:

```
POST /v1/runs

{ "report": { ... }, "schedule": "7a1c9e52-3b4d-4f6a-8c0e-2d5f7b9a1c3e", "startedAgoMilliseconds": 3412 }
```

- It waits in the outbox like any result, so a node cut off from the API for an hour sends that hour's runs when it is back, oldest first.
- The API stores it as a run of that schedule, in the account of the person who made the schedule. It is taken while the schedule is paused and after it was deleted, since the node made it before it heard.
- Only a node the API handed the schedule to sends its runs. The API keeps which node read a copy with which schedule, and takes a run that started while the node could hold it: before the node read a copy without the schedule, and within the time a copy is good for after the last one that held it. A run that arrives later than an outbox could have kept it is not taken. A node that only knows a schedule's id writes nothing into its series.
- 422 says `schedule` is no schedule of the node's workspace, the node was not handed it or its hold on it is over for this run, or the report's `tool` or `target` is not the schedule's. The result leaves the outbox.
- A run with `claim` carries no `schedule`.
- `GET /v1/runs` leaves scheduled runs out, so a device's history holds what a person ran. `schedule=<id>` reads the runs of one schedule.

## The outbox

A finished run waits on the node until the API has answered for it.

- The outbox has a cap in runs, in bytes and in age: 500 runs, 64 MiB and 7 days on a node with a disk. A build for routers picks smaller ones.
- It sits on disk where there is one. On a router it sits in memory to spare the flash, so a reboot loses what was unsent. `outbox.keeps` in the heartbeat says which, and the router package says so too.
- When it is full, the oldest unsent run goes first, and the next heartbeat's `dropped` counts it.
- The node sends the oldest first, one at a time, and waits when the API answers 429.
- A run sent twice counts once.

## Stopping a node, and all nodes

A person can take a node's work away in two ways, and both reach the node as the answer to its next call.

- Revoking a node is for good. Its stream ends, and every call with its token answers 401. The node enrolls again, with a new `key`, to come back.
- Stopping all nodes is for every node of a workspace at once, and it can be undone. Their streams end, and every call with one of their tokens answers 403 `FLEET_STOPPED` until they are resumed. The code keeps the name it shipped with. No token is revoked, so a resumed node needs nothing new. It asks again after an hour.
- A node that reads either answer ends the runs it is making at once and sends no further probe. A node that holds a run claims every third of `leaseSeconds`, so that is within 30 seconds.
- After `FLEET_STOPPED` the results in the outbox wait for the nodes to be resumed, within the outbox's caps. After 401 they have nowhere to go.

## Listing the nodes

A person's install lists the nodes it may ask with `GET /v1/nodes`, ordered by name.

| Field | Holds |
| --- | --- |
| `id`, `name` | As the heartbeat's answer has them |
| `online` | `true` while the node holds its event stream |
| `lastSeenAt` | When the API last heard from the node, `null` for a node that never called |
| `version`, `features` | From the node's last heartbeat, so an install offers only the tools the node has. `null` and `[]` before its first |
| `tags` | The tags that count: the node's declaration with a person's edits applied |

- `limit` takes 1 to 200 nodes, 50 when left out. `next` in the answer is the cursor for the next page, and `null` on the last one.
- `tag` narrows the list with a selector, as `GET /v1/runs` takes it: `tag=isp=telia,line!=dsl`. [Schedules](#schedules) has every term a selector takes.
- A node's location is left out of the list. Who in a workspace may see where a home node is hasn't been decided.
- A node's token gets 403 `FORBIDDEN`. A node reads no other node.

## Asking for a run

A person's install asks, gets the run's id at once and reads the run until it ends.

```
POST /v1/run-requests

{ "node": "fra-1", "tool": "mtr", "target": "example.com", "options": { "rounds": 12 }, "expiresIn": 300, "key": "k7p2xd" }
```

- `node` is the node's name or its id. The API checks that the node belongs to the asker's workspace.
- `expiresIn` is how long the request may wait for the node, 30 to 86,400 seconds, 300 when left out.
- `key` is a word the install makes for this request. The same `key` from the same install answers 200 with the run it made the first time, and 422 when the rest of the body differs.

The answer is the run as `GET /v1/run-requests/{id}` gives it: `id`, `state`, `reason`, the `node` with `online` and `lastSeenAt`, what was asked, `requestedAt`, `expiresAt`, `attempt`, `claimedAt`, `receivedAt`, `observedStartedAt` and `report`. `report` is `null` until the run ended.

`GET /v1/run-requests/{id}?wait=25` answers when the state changes or after 25 seconds, whichever comes first, 30 at most. So `rvrs mtr example.com --node fra-1` shows requested, then running, then the result, with a handful of requests.

Only the account that asked reads a run this way. Anyone else gets 404 `RUN_NOT_FOUND`.

Once a run ended it is a run like any other: it is in `GET /v1/runs`, in history on every device of the account, with the origin's `kind` `node`.

## A run's states

| From | To | Moved by | When |
| --- | --- | --- | --- |
| | `requested` | A person's install | It asks |
| `requested` | `running` | The node | It claims the run. `attempt` goes up by one and the lease starts |
| `running` | `ended` | The node | Its result for the newest attempt arrives. The report's `outcome` says how the run ended |
| `running` | `requested` | The API | The lease ran out, or the node's claim no longer lists the run, and the run may still be given out |
| `requested` | `expired` | The API | `expiresAt` passed with no node holding the run. `reason` is `unclaimed` when no node ever took it and `abandoned` when one did and sent no result |
| `running` | `expired` | The API | The lease ran out after `expiresAt`, or it was the third attempt. `reason` is `abandoned` |
| `expired` | `ended` | The node | The result of the newest attempt arrives late |

A run a device makes itself is stored `ended`. `expiresAt` only limits how long the API gives the run out. A node that holds the run and keeps its lease may finish after it.

## What a node does with each answer

The table holds for answers with the API's error. Any other answer is read as a 5xx, whatever its status.

| Answer | The node |
| --- | --- |
| 200, 201 | Goes on |
| 401 | Its token is unknown or revoked. It ends the runs it is making, closes the stream and makes no further call until it is given a token |
| 403 | It may not do this now. It asks again after an hour, and never after `NODE_REVOKED`. After `FLEET_STOPPED` it also ends the runs it is making and closes the stream |
| 404, 410, 413, 422 | The call is wrong and stays wrong. It is not repeated. A result answered this way leaves the outbox and counts as dropped |
| 409 | The claim is lost. The result leaves the outbox |
| 429 | Waits as long as `Retry-After` says |
| 5xx, an answer without the API's error, or no answer | Tries again, with the same doubling window as a reconnect |

## The numbers

Every number the node acts on reaches it from the API, so each can change without an update to any node.

| What | Value | From |
| --- | --- | --- |
| A lease | 90 s | `leaseSeconds` in a claim's answer |
| Renewing a lease | Every third of it, 30 s | |
| One attempt, at most | 3,600 s | The API stops renewing |
| Attempts per run | 3 | The API |
| A request waits | 300 s, or the asker's 30 to 86,400 | `expiresIn` |
| `work`, while a run waits | Every 15 s | The API |
| A claim while idle | At random between 900 and 1,800 s | `claimSeconds` in `hello` |
| A keep-alive | Every 25 s | `keepAliveSeconds` in `hello` |
| A heartbeat | Every 3,600 s | `heartbeatSeconds` in `hello` |
| The reconnect window | 10 s, doubling to 300 s | `reconnectSeconds` in `hello` |
| A read that waits | 30 s at most | `wait` |
| A result's body | 2 MiB | |
| A schedule's interval | 60 to 86,400 s | `intervalSeconds`, the person's |
| Scheduled run starts at one target | About 10 a second from one workspace, on average | The API, as `everySeconds` |
| A node's copy of its schedules | 50 schedules, good for 604,800 s | `keepSeconds` in the copy |
| `schedules`, while a node's copy is behind | Every 15 s | The API |

### What an idle node costs

A node that holds its stream and has nothing to do is woken by events and asks for nothing in between, except this:

| What | How often | A node a day | 50,000 nodes |
| --- | --- | --- | --- |
| Keep-alive | Every 25 s | No request. A line on the open stream | No query |
| Idle claim | Every 1,350 s on average | 64 requests | 3.2 million a day, 37 a second |
| Heartbeat | Every 3,600 s | 24 requests | 1.2 million a day, 14 a second |

A node reads its schedules when it connects and when they change, and asks for nothing in between. An idle claim is one lookup in an index that holds only the runs that wait or are held, and it finds nothing. An unchanged heartbeat reads one row. The API can raise both numbers in `hello` for a large fleet. Recovering a lost wake-up depends on neither.

## Eight cases

### A node behind NAT

1. Whatever set the router up wrote an enrollment token. The node sends `POST /v1/nodes` and gets its token.
2. It sends a heartbeat and opens the event stream. Both are connections the node made. Nothing listens on the node.
3. A person runs `rvrs mtr example.com --node cpe-4471`. Their CLI sends `POST /v1/run-requests` and gets the run, `requested`.
4. The API writes `work` to the node's stream. The node claims and gets the run as attempt 1 with a lease of 90 seconds. The run is `running`.
5. The node runs the MTR within its limits and claims every 30 seconds to keep the lease.
6. The node sends the report with its claim. The run is `ended`, and the CLI's waiting read returns it with the report.

### A node offline for a day

1. The stream ends. The API shows the node as offline, with the time it last saw it.
2. A request made while it is away stays `requested` and turns `expired` with the reason `unclaimed` when `expiresIn` is over. The asker saw `online: false` in the first answer.
3. The node tries to reconnect. Its window grows to 300 seconds, so it tries about every two and a half minutes on average.
4. Runs it makes by itself wait in the outbox, up to its caps. When the outbox is full, the oldest goes.
5. The uplink is back. A stream opens, `hello` arrives, the node sends a heartbeat with its `dropped` count, sends the outbox oldest first and claims. Each result says how long ago its run started, so the API places it in time whatever the node's clock did in between.
6. Its token is the same as before. Nothing has to be set up again.

### A node one version behind the API

1. The API's answers carry fields the node doesn't know. The node ignores them. The same goes for an event with a new name.
2. Someone asks the node for a tool its build lacks, or sets an option it doesn't know. The node claims the run, refuses it and sends a report with the outcome `refused` and an `error` that names the tool or the option. The person sees why within a second or two.
3. The API has a verdict the node's `protocol` revision lacks. It doesn't send it to this node. If one reaches the node anyway, the node stops that run and says which verdict it didn't know.
4. The node's heartbeat lacks a field the newer API reads. The API treats the fact as unknown.
5. A node newer than the API sends fields the API doesn't know. The API drops them.
6. A change that none of this covers gets `/v2`, and `/v1` stays until the nodes have updated.

### A wake-up that never arrives

1. A request is stored and the `work` event for it is lost.
2. The stream is fine, so 15 seconds later the API writes `work` again. The node claims, and the run is in the answer.
3. Had the stream been dead without the node knowing, the node would have missed its keep-alives, reconnected within 50 seconds plus its wait, got `hello` and claimed.
4. Both happen well inside the 300 seconds a request waits by default. The idle claim, up to 1,800 seconds away, is not what recovers it.

### A lease that runs out while the node is still probing

1. The node claimed a run as attempt 1 and its uplink drops in the middle of the MTR.
2. The node can't renew. 90 seconds after its last claim the lease is over, and the run is `requested` again. Only this node can claim it, since it was asked of this node.
3. The node finishes the MTR. The report, with the lost rounds in it, waits in the outbox, and the node still lists the run as held.
4. The uplink is back. The node sends the result. Attempt 1 is still the newest, so the API takes it and the run is `ended`. Had the node claimed first, its `holding` would have listed the run and the verdict would have been `keep`.
5. Now the other way: the node hung and a watchdog restarted it. It claims with an empty `holding`. The API frees the run and gives it out again in the same answer, as attempt 2. If attempt 1's result shows up later, the answer is 409 `CLAIM_LOST` and the node drops it. One result is stored.

### A router that reboots with runs unsent

1. The router keeps its outbox in memory, and three results were waiting. The reboot loses them.
2. Its token and its `key` are on flash, written once. The node starts, sends a heartbeat with a new `boot`, opens the stream and claims with an empty `holding`.
3. Runs that were asked of it and whose results were lost are free again. Those still inside `expiresIn` come back as the next attempt and are made again. The others turn `expired` with the reason `abandoned`.
4. Runs the node made by itself are gone. It doesn't make them up, and its `dropped` count starts at zero, since it no longer knows what it lost.
5. A node with a disk loses nothing in a reboot. Its outbox is still there and goes out after `hello`, without `startedAgoMilliseconds`, since the clock that counted it started over.

### An enroll answered twice, out of order

1. A router on a slow uplink sends enroll A. No answer comes within its wait, so it sends enroll B with the same `key`. A is still on its way.
2. The API handles B first. The `key` is new under this enrollment token, so it makes the node and answers 201 with the node's token.
3. The API handles A. It knows the `key`, so it answers 200 with the same node and the same token. Had both arrived in the same instant, one would have made the node and the other would have found it.
4. B's answer reaches the node, then A's, or A's, then B's. Both carry the same token. Whichever the node stores, it holds a token that works.
5. The enrollment token counted one node. Nothing was revoked, so there is nothing a late answer could undo.

### A schedule on two nodes, one cut off for an hour

1. A person makes a schedule: ping `example.com` every 60 seconds from `fra-1` and `cpe-4471`. The API stores it and writes `schedules` to both nodes' streams.
2. Each node reads its copy. `fra-1` is told its next run starts in 41,250 ms and `cpe-4471` in 9,800 ms, so the two never ping in the same second. Each runs every 60 seconds from there and sends each report with the schedule's id.
3. The uplink of `cpe-4471` drops. The node goes on pinging every 60 seconds by its own clock. Each report says the packets were lost and waits in the outbox.
4. An hour later the uplink is back. A stream opens, `hello` arrives, the node reads its copy, which is the same version, and sends the 60 runs from its outbox, oldest first, between its claims. Each says how long ago it started, so the API places it in the hour it was made.
5. The schedule's chart shows `fra-1` without a gap, and for `cpe-4471` an hour of lost packets where the uplink was gone.
6. Had the node lost power for that hour, there would be a gap. It makes up no run it missed.
7. Had it restarted during that hour with the runs in its outbox, they would arrive without how long ago they started. The API places each by the node's clock where that is believable, and by its arrival otherwise.

## How the rvrs API keeps its side

Nothing here binds another controller. It is how the rvrs API meets the rules above.

- The event stream is a service of its own, named in the heartbeat's answer. Releasing the API leaves the streams open. An instance that holds streams and stops closes them over a minute, so its nodes come back spread out.
- Postgres is the queue. A request for a run is stored and announced with `NOTIFY` in one transaction.
- Each instance of the stream service holds one `LISTEN` connection and writes `work` to the nodes whose streams it holds. The rest of the API only sends `NOTIFY`.
- Every 15 seconds each instance of the stream service asks, in one query for all its streams, which of their nodes have a run waiting, and writes `work` to those. That is the repeat. Its cost grows with the runs that wait and not with the nodes that have nothing to do.
- The stream service starts listening before it looks for waiting runs, so a request can't fall between the two.
- A claim takes its runs in one short transaction (`FOR UPDATE SKIP LOCKED`), so two claims never get the same attempt. The index it reads holds only the runs that wait or are held.
- A node's token is derived from its `key` with a secret only the API holds. So the API answers the same token again without having kept it, and stores its hash alone. A unique index on the enrollment token and the `key` makes two enrolls at once one node.
- The API checks the workspace and the node on every enroll, claim and result. A node can't claim or answer another node's run.
- Revoking a node ends its stream at once, and every later call of its token answers 401. Replacing its token does the same for the token it had.
- A node counts as online by what the stream service wrote last. A stream that opens or closes is written within a second. A stream service that dies without closing its streams takes its nodes offline 90 seconds later. A line that drops without closing the connection can show its node as online until that connection times out.
- A node's newest stream is the one that counts. On the same instance of the stream service the older one ends at once, and on another instance within 15 seconds.
- Stopping every node of a workspace is one row. Every call with a node's token reads it, and the stream service ends the streams of that workspace's nodes.
- A node's offset into a schedule's interval is a hash of the schedule's id and the node's id, in milliseconds. Nothing is stored per node and schedule.
- A change to a schedule that goes by a selector or runs on every node raises a number on its workspace, and a node's copy is behind while the number it last read is another. A schedule that names its nodes marks those nodes as behind and no other, and so does a change to a node's tags. The stream service writes `schedules` to the nodes that are behind, 500 a second at most, so a fleet reads its copies over minutes and never in one second.
- How many nodes a schedule runs on, and the interval the budget leaves it, are counted when a schedule changes and, within 15 seconds, when nodes or their tags did. Every such change raises a number on the workspace, and a count writes the number it began at when it is done, so a change that lands while a count runs is counted by the next pass. A fleet that enrolls is counted once per pass and never once per node.
- Which node was handed which schedule is a row per node and schedule, written when the node reads its copy and at most once an hour after that.

## Left out on purpose

- Each hop as it arrives. In version 1 the asker sees the state change and then the whole result. A later version can let a node send a report while the run is still going, as an added field.
- A route for a person to stop a run on a node. The `stop` verdict is the node's half of it.
- Taking a declared location back. A node that stops declaring a location leaves `location` out, which changes nothing, so a person clears it in the web app.
- Rotating a node's token. When it comes, it is a route the node calls with the token it holds, and the old token works until the node has used the new one.
- A key pair per node. A node's token is a random credential today. A public key can be added to the enroll body later without a change to the rest.
- Alerts on a schedule's runs.
- Asking many nodes at once.
- What a plan allows. An API that refuses a node or a request for its plan answers 403 with a code of its own.

## Where it is checked

`cargo test` reads `openapi.json` and checks every file in `fixtures/` against the schema of the body it is an example of, that every body has an example, that a second enroll shows the same answer as the first, that the page reads an answer without the API's error as a 5xx, that a declaration breaking a rule of tags or location is caught, and that the report inside each result is one the CLI reads. The node's own checks are `rvrs_core::node`, whose tests hold its options against the table on this page, its heartbeat against the heartbeat's example, and the refusal it writes against the refused result's example. It checks that the two nodes of the schedule's example get different starts, and that the copy's entries say what the schedule says. The rvrs API keeps a copy of the fixtures and reads them against its own copy of the shapes. `rvrs node` is tested against an API inside the test that answers with these fixtures (`crates/rvrs/src/node/service_tests.rs`).
