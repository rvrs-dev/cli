# The node protocol

How a node talks to the rvrs API, and how a person's install asks a node for a run. `openapi.json` in this folder names every route and field, and `fixtures/` holds one example of each body. This page says what the routes are for, who moves a run from one state to the next, and what each side does when something gets lost.

Status: version 1, written down and not built yet. The rvrs API and `rvrs node` are built to it next. Anyone may build a controller or a node that speaks it.

## Who speaks

| Who | Signs in with | May |
| --- | --- | --- |
| A node (`rvrs node`) | Its own token | Send its runs, claim the runs asked of it and say what it is. It reads no history, no library and no other node |
| Whatever sets a node up | An enrollment token | Enroll nodes, and nothing else |
| A person's install (the CLI, the app) | A login | Ask a node of its workspace for a run and read where that run stands. It never holds the event stream and never claims |

A credential goes in `Authorization: Bearer <token>`. The node opens every connection, over HTTPS, so a node behind NAT needs no open port and no address of its own.

## The routes

| Route | From | Does | A retry |
| --- | --- | --- | --- |
| `POST /v1/nodes` | An enrollment token | Enrolls a node and answers with its token | Safe. The node's `key` tells the API it is the same node |
| `POST /v1/nodes/me/heartbeats` | A node | Says what the node is: version, build features, limits, outbox | Safe. Each one replaces the last |
| `GET /v1/nodes/me/events` | A node | The event stream, which wakes the node when there is work | Safe. A new stream replaces the old one |
| `POST /v1/nodes/me/claims` | A node | Takes the runs that wait for the node and keeps the ones it holds | Safe. A claim says everything the node holds, every time |
| `POST /v1/runs` | A node | Sends a finished run, with `claim` when the run was asked for | Safe. A run is told apart by the node and the report's `id` |
| `POST /v1/run-requests` | A person's install | Asks a node for a run and answers with the run's id | Safe with a `key`. Without one a retry asks twice |
| `GET /v1/run-requests/{id}` | A person's install | Where the run stands, and its report once it ended | Safe |

`POST /v1/runs` is the route every install already sends its runs through. A node uses the same one.

## Rules for both sides

- Fields are only added. Each side ignores a field, an event and an error code it doesn't know. A change that would break a reader gets a new path, `/v2`, and the API serves both while nodes update.
- Values that are words grow too: a run's `state`, a `reason`, a `verdict`, a report's `outcome`, a build feature. A reader keeps a word it doesn't know as it is.
- `options` is the one place where an unknown word matters. A node that doesn't know a tool or an option refuses the run, because running without it would measure something else than what was asked.
- Times the node acts on are lengths in seconds, never moments. A router's clock can be wrong before it has synchronized, and the protocol doesn't depend on it.
- Every answer that is not 2xx has the one error shape, `{ "error": { "code", "message" } }`. A reader branches on `code`.
- No heartbeat, event or log line carries a credential, a target or anything from a report.

## Tokens and enrolling

A node's token is its own: one per node, stored on the node, revoked alone. It comes from one of two places. A person makes a node in the web app and gives the node its token (`RVRS_TOKEN`). Or the node enrolls.

An enrollment token enrolls a set number of nodes before it expires and does nothing else. No image carries one. Whatever sets the device up writes it at first boot: cloud-init, an installer, an ISP's auto-configuration server. So a router imaged today can enroll weeks later, and an image found in the wild enrolls nothing.

To enroll, the node makes a random `key` of 128 bits or more, keeps it, and sends it with the name it proposes:

```
POST /v1/nodes
Authorization: Bearer <enrollment token>

{ "key": "5tq0yM3VfJm0b6mVq1mW4w", "name": "cpe-4471", "version": "0.14.2" }
```

The answer carries the node, its workspace and the node's token, once. The node stores the token next to the key and forgets the enrollment token where it can.

- A node that lost the answer enrolls again with the same `key`. The API answers 200 with the same node and a new token, the earlier token stops working, and the enrollment token's count goes up once.
- A node with a new `key`, after a factory reset, is a new node.
- A node's name is unique in its workspace: 1 to 63 lowercase letters, digits, `.`, `-` and `_`. It is what `--node` takes. The API makes it from what the node proposes, and a person can change it later.
- 410 `ENROLLMENT_CLOSED` says the enrollment token expired, was revoked or has enrolled every node it may. 403 `NODE_REVOKED` says this `key` belongs to a node that was revoked. The node stops enrolling after either.
- A node without a token and without an enrollment token makes no call at all. The API's default address alone never enrolls a node and never receives a run.

## The heartbeat

A heartbeat says what the node is. The API learns from it what to show a person and what the node can run.

| Field | Holds |
| --- | --- |
| `version` | The node's rvrs version |
| `system` | `os` and `arch`, as in `linux` and `aarch64` |
| `boot` | A random word the node makes each time it starts, so the API sees a restart |
| `features` | The build features the node has: `traceroute`, `ping`, `mtr`, `dns`, `bgp`, `iperf3`, `tcp` (the TCP probe), `mtu` (the path MTU check), `console` |
| `limits` | What the node allows itself, whatever it is asked: `runsAtOnce`, `runsPerMinute`, `packetsPerRun`, `secondsPerRun`, `reportBytes` |
| `outbox` | `waiting` (runs not sent yet), `dropped` (runs given up since this start) and `keeps` (`disk` or `memory`) |

The answer names the node and its workspace as the API knows them, so a node that was renamed on the web learns its name.

A node sends one when it starts, when one of these facts changes, and every `heartbeatSeconds`. Whether a node is online is not the heartbeat's to say. A node is online while it holds its event stream.

## The event stream

`GET /v1/nodes/me/events` answers with Server-Sent Events and stays open.

| Event | Data | The node |
| --- | --- | --- |
| `hello` | `keepAliveSeconds`, `claimSeconds`, `heartbeatSeconds`, `reconnectSeconds` | Takes the numbers, then claims and sends what waits in its outbox |
| `work` | `{}` | Claims now |
| `: keep-alive` (a comment line) | | Knows the stream is alive |

- An event says there is work and nothing about it. It carries no run's parameters and nothing secret. What the work is comes from the claim.
- `hello` is the first event of every stream. Its numbers are the API's to choose, so a fleet can be slowed down without updating a node.
- A stream that stays silent for twice `keepAliveSeconds` is dead. The node closes it and opens a new one.
- When a stream ends or can't be opened, the node waits a random time up to `reconnectSeconds` and tries again. Every failure in a row doubles that window, up to 300 seconds, and a `hello` sets it back. So an API release doesn't bring every node back in the same second.
- A node holds one stream. A second one with the same token replaces the first.
- `schedules` is a reserved event name. It is filled in when nodes repeat runs on a schedule.

## Claiming

A wake-up can get lost, so the claim never depends on one. A node claims:

- after every `hello`, which is every time it connects,
- after every `work`,
- every `claimSeconds` while it has nothing to do,
- every third of `leaseSeconds` while it holds a run,
- and after an attempt to open the stream that failed, so a node whose network lets no stream through still gets its runs, later.

A claim says everything the node holds and how much more it will take:

```
POST /v1/nodes/me/claims

{ "room": 1, "holding": [ { "run": "0e6d2f0a-8c0e-4f58-9a55-0b8f3f6f1c11", "attempt": 1 } ] }
```

`holding` lists every run the node claimed and the API hasn't answered a result for: the ones still probing and the ones whose result waits in the outbox. `room` is how many more runs the node will start now, within its own limits.

The answer has three parts.

- `runs`: the runs the node now holds for the first time, `room` of them at most, oldest first. Each has its `run` id, its `attempt`, and the `tool`, `target` and `options` to run.
- `holding`: a verdict for each run the node listed. `keep` means the lease is renewed. `stop` means a person stopped the run: the node ends it now and sends what it measured, with the outcome `stopped`. `drop` means a newer attempt has the run or the run has its result: the node stops and sends nothing. A node treats a verdict it doesn't know as `keep`.
- `leaseSeconds`: how long each run in `runs` and each `keep` is the node's from now.

A run the API believes the node holds, and the node didn't list, is free again at once. That is how a restart is noticed: a node that starts up claims with an empty `holding`, and the runs it lost come back in the same answer as a new attempt.

A node sends one claim at a time and waits for its answer before the next. Two claims in flight could leave out a run the first one was about to hand over, and the node would get it twice. Should a node ever hold two attempts of one run, the higher one is the run and the lower one is dropped.

A lease is a hold for a limited time. A node that goes silent loses it after `leaseSeconds`, and the run may be claimed again. Every claim of the same run is a new attempt: 1, 2, 3. The API gives a run out three times at most and never after the request expired.

Before it starts a run, the node checks it against what it is and what it allows: the tool and every option are ones its build knows, the run fits its limits, and the target is one it may probe. A run that fails the check is not started. The node sends a report with the outcome `refused` and the reason in `error`, through the same route as any result.

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

## Sending a result

A finished run goes to `POST /v1/runs` as its report. A run that was asked for carries the claim it was made under:

```
POST /v1/runs

{ "report": { ... }, "claim": { "run": "0e6d2f0a-8c0e-4f58-9a55-0b8f3f6f1c11", "attempt": 1 } }
```

- The report's `origin.kind` is `node` and `origin.node` is the node's name. Its `id` is the node's own, unique on that node.
- A node sends the report alone, without the `detail` a CLI adds for its other installs.
- The API takes a result when its attempt is the run's newest and the run has no result yet. Whether the lease ran out in between doesn't matter: a node that lost its uplink during an MTR still delivers that MTR, which is the run someone wanted.
- 409 `CLAIM_LOST` says a newer attempt has the run, or the run already has its result. The node drops the result. So a node that hung can't overwrite a newer result, and a run made twice is stored and counted once.
- 200 says the API had this report already. Nothing changed.
- A report without `claim` is a run the node made by itself, such as a scheduled run.

## The outbox

A finished run waits on the node until the API has answered for it.

- The outbox has a cap in runs, in bytes and in age: 500 runs, 64 MiB and 7 days on a node with a disk. A build for routers picks smaller ones.
- It sits on disk where there is one. On a router it sits in memory to spare the flash, so a reboot loses what was unsent. `outbox.keeps` in the heartbeat says which, and the router package says so too.
- When it is full, the oldest unsent run goes first, and the next heartbeat's `dropped` counts it.
- The node sends the oldest first, one at a time, and waits when the API answers 429.
- A run sent twice counts once.

## Asking for a run

A person's install asks, gets the run's id at once and reads the run until it ends.

```
POST /v1/run-requests

{ "node": "fra-1", "tool": "mtr", "target": "example.com", "options": { "rounds": 20 }, "expiresIn": 300, "key": "k7p2xd" }
```

- `node` is the node's name or its id. The API checks that the node belongs to the asker's workspace.
- `expiresIn` is how long the request may wait for the node, 10 to 86,400 seconds, 300 when left out.
- `key` is a word the install makes for this request. The same `key` from the same install answers 200 with the run it made the first time, and 422 when the rest of the body differs.

The answer is the run as `GET /v1/run-requests/{id}` gives it: `id`, `state`, `reason`, the `node` with `online` and `lastSeenAt`, what was asked, `requestedAt`, `expiresAt`, `attempt` and `report`. `report` is `null` until the run ended.

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

| Answer | The node |
| --- | --- |
| 200, 201 | Goes on |
| 401 | Its token is unknown or revoked. It closes the stream and makes no further call until it is given a token |
| 403 | It may not do this now. It asks again after an hour, and never after `NODE_REVOKED` |
| 404, 410, 413, 422 | The call is wrong and stays wrong. It is not repeated. A result answered this way leaves the outbox and counts as dropped |
| 409 | The claim is lost. The result leaves the outbox |
| 429 | Waits as long as `Retry-After` says |
| 5xx, or no answer | Tries again, with the same doubling window as a reconnect |

## The numbers

Every number the node acts on reaches it from the API, so each can change without an update to any node.

| What | Value | From |
| --- | --- | --- |
| A lease | 90 s | `leaseSeconds` in a claim's answer |
| Renewing a lease | Every third of it, 30 s | |
| One attempt, at most | 3,600 s | The API stops renewing |
| Attempts per run | 3 | The API |
| A request waits | 300 s, or the asker's 10 to 86,400 | `expiresIn` |
| A claim while idle | Every 240 s | `claimSeconds` in `hello` |
| A keep-alive | Every 25 s | `keepAliveSeconds` in `hello` |
| A heartbeat | Every 3,600 s | `heartbeatSeconds` in `hello` |
| The reconnect window | 10 s, doubling to 300 s | `reconnectSeconds` in `hello` |
| A read that waits | 30 s at most | `wait` |
| A result's body | 2 MiB | |

## Six cases

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
5. The uplink is back. A stream opens, `hello` arrives, the node sends a heartbeat with its `dropped` count, sends the outbox oldest first and claims.
6. Its token is the same as before. Nothing has to be set up again.

### A node one version behind the API

1. The API's answers carry fields the node doesn't know. The node ignores them. The same goes for an event with a new name.
2. Someone asks the node for a tool its build lacks, or sets an option it doesn't know. The node claims the run, refuses it and sends a report with the outcome `refused` and an `error` that names the tool or the option. The person sees why within a second or two.
3. The node's heartbeat lacks a field the newer API reads. The API treats the fact as unknown.
4. A node newer than the API sends fields the API doesn't know. The API drops them.
5. A change that none of this covers gets `/v2`, and `/v1` stays until the nodes have updated.

### A wake-up that never arrives

1. A request is stored and the `work` event for it is lost.
2. Where the stream was dead without the node knowing, the node misses its keep-alives, reconnects within 50 seconds plus its wait, gets `hello` and claims. The run is in the answer.
3. Where the stream was fine and the event was never written, the node's next idle claim comes within 240 seconds. The run is in the answer.
4. Both happen before a request with the default `expiresIn` of 300 seconds expires. An asker who sets less than 240 seconds accepts that a lost wake-up can cost the run.

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
5. A node with a disk loses nothing in a reboot. Its outbox is still there and goes out after `hello`.

## How the rvrs API keeps its side

Nothing here binds another controller. It is how the rvrs API meets the rules above.

- Postgres is the queue. A request for a run is stored and announced with `NOTIFY` in one transaction.
- Each API instance holds one `LISTEN` connection and writes `work` to the nodes whose streams it holds. A connected node that has nothing to do costs no query between its idle claims.
- The API starts listening before it looks for waiting runs, so a request can't fall between the two.
- A claim takes its runs in one short transaction (`FOR UPDATE SKIP LOCKED`), so two claims never get the same attempt.
- The API checks the workspace and the node on every enroll, claim and result. A node can't claim or answer another node's run.
- Revoking a node ends its stream at once, and every later call of its token answers 401.

## Left out on purpose

- Each hop as it arrives. In version 1 the asker sees the state change and then the whole result. A later version can let a node send a report while the run is still going, as an added field.
- A route for a person to stop a run on a node. The `stop` verdict is the node's half of it.
- A key pair per node. A node's token is a random credential today. A public key can be added to the enroll body later without a change to the rest.
- Schedules. The event name `schedules` is reserved.
- Asking many nodes at once.
- What a plan allows. An API that refuses a node or a request for its plan answers 403 with a code of its own.

## Where it is checked

`cargo test` reads `openapi.json` and checks every file in `fixtures/` against the schema of the body it is an example of, that every body has an example, and that the report inside each result is one the CLI reads. The rvrs API keeps a copy of the fixtures and reads them against its own copy of the shapes.
