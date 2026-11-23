# The run report

`run-report.schema.json` is the shape of one run as data: what `rvrs <tool> --json` prints, what `rvrs show <id> --json` prints later, and what an account stores. It is JSON Schema 2020-12. `fixtures/` holds a report per tool with documentation addresses in place of this device's, plus `iperf3.json`, which the iOS app makes, `sparse.json`, a report from an origin that measured little, and `tagged.json`, a node's MTR with its tags and location.

## Rules

- `schema` is the version, `1` today. Within a version fields are only added. A change that would break a reader raises the number.
- A reader ignores fields it doesn't know and keeps values it doesn't know as they are: a new `tool`, a new `origin.kind`, a new `outcome`.
- Every figure is optional. `measured`, `hops`, `records`, `sources`, `probe` and `mtu` say what a run found, and a run from a small device finds less than one from a laptop.
- `id` is unique on the install that made the run. An account tells runs apart by the install and this id, for good, so an install never uses one twice. rvrs makes 26 characters from 128 random bits. A reader takes any string of up to 64 characters.
- `measured` and `hops` are what was measured. `interpretation` holds inferences only, each a sentence that says it is one.

## Reading a report

Every surface shows a run it didn't make from the report alone. The CLI does it in `Run::from_report`, the app in `TraceRun(report:)`, the web app in its run view. They follow the same rules.

- The sentences are the maker's. A reader shows `summary`, `error` and `interpretation` as they came and never works them out again from figures it only has part of.
- The figures are the report's: `measured`, and each hop's own. A figure that is missing is left out. It is never shown as zero.
- A probing tool (traceroute, MTR, ping) gets its full view when the report has `samples`: the hops under their networks, the chart, each hop's rounds. Without samples the parts drawn probe by probe are left out.
- Where a report keeps fewer samples than the run sent probes, the stated figures stand and the samples only draw the chart.
- The destination is the last hop that answered from `address`. A run stopped by hand reached it too, so `outcome` alone doesn't say.
- A DNS lookup shows `records`, the reference answer, and `sources` with each one's status, time and addresses.
- BGP, iperf3 and any tool a reader doesn't know show the status line, the figures and the interpretation.
- Tags and location are the origin's. A reader shows them where that helps, such as a column for a tag or the place beside a node's name, and never requires them. A run without them is whole. A reader that can't read the shape it finds leaves them out and shows the run. A precision it doesn't know is read as `city`.
- Not in a report today: the DNS delegation path, a BGP lookup's paths, prefixes and history, and iperf3's intervals. Running the tool again on this device brings them.

## What points to a home

A run from a node on someone's line says where that line is, in two parts. Every surface that shares a run treats them the same way.

The private network the run started in. A share shows it unless the person sharing turns Private Networks off, since the first hops are where a line's trouble often is.

- `origin.sourceAddress` when it is a private or link-local address.
- The address and hostname of each hop of the line's own network: every hop at a private or link-local address before the first hop that answered from anywhere else. That is the home's router, at `192.168.1.1` under a name someone gave it, and whatever sits between it and the provider. Shared address space (100.64.0.0/10) is the provider's, and a private address further along the path is inside a provider. Both stay in every share.
- The `server` of a DNS source of the kind `device` when it is a private or link-local address: the resolver is the line's own router.

The line's public address, and what gives it away. A share leaves it out unless the person sharing keeps it in.

- `origin.publicAddress`, and `origin.sourceAddress` when it is a public address.
- The address and hostname of a hop that is the origin itself, wherever it is in the path: a hop at one of the origin's public addresses, or an IPv6 hop in the same /48 as one of them. A line is given a /48, a /56, a /60 or a /64 and a report doesn't say which, so the widest decides.
- The `server` of a DNS source of the kind `device` at one of the origin's public addresses.

A hop that is left out stays, with its figures, so the path keeps its length. Everything else in a report is about the path and the target. A location, when a report carries one, belongs to the same list, and is never more precise than a person set it.

Two things the rule can't see, which whoever shares a run decides about: a node's name and its tags are the workspace's own words and can name a customer, and a sentence the maker wrote (`summary`, `error`, `interpretation`) may name an address.

## Tags and location

`origin.tags` and `origin.location` are the origin's tags and location as they were when the run was stored. A tag is a key and a value, as in `isp=telia`, at most 20 per origin. A location is `lat`, `lon`, an optional `place` and a `precision` (`exact`, `area` or `city`) that says how many digits of the coordinates were kept.

- `origin.metadataRevision` is the revision of the origin's tags and location the run was made under, which a node takes from its API's last answer and keeps with the run until the API has it. The API checks it against that node's own history and writes `tags` and `location` from it, so a run that waited in an outbox keeps the tags of the day it ran. A reader never needs it.
- The API writes both and replaces whatever the sender put there. A node need not write them. Changing an install's tags or location afterwards leaves every earlier run as it was. A run stored before this, and a run made on this device, has neither.
- The API rounds the coordinates to the precision before it keeps them, so a report never shows more than the person chose. A report the API wrote always carries `precision`. A reader that doesn't know the word treats it as `city`.
- A node declares its own tags and location in its configuration and in the enroll and the heartbeat. [`node-config.md`](node-config.md) is that configuration and [the node protocol](node-protocol/README.md) has the rules for the bodies.

## Where it is checked

`cargo test` checks that every field of a report and of each fixture is one the schema names, that each fixture is read and written back unchanged, and that each becomes a run every look can draw. `traceroute.json`, `mtr.json` and `bgp.json` are the reports of runs the tests build, so they can't drift (`RVRS_WRITE_FIXTURES=1 cargo test` rewrites them). The iOS app's tests read copies of the same files. The rvrs API reads the same fixtures against its own copy of the shape.

## The node protocol

`node-protocol/` is the other contract in this folder: how a node talks to the rvrs API and how a person's install asks a node for a run, as a page, an OpenAPI document and fixtures. A node's result is this report, sent through the route every install uses. `node-config.md` is the contract for what a node declares about itself, its tags and its location.
