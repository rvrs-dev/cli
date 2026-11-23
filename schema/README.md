# The run report

`run-report.schema.json` is the shape of one run as data: what `rvrs <tool> --json` prints, what `rvrs show <id> --json` prints later, and what an account stores. It is JSON Schema 2020-12. `fixtures/` holds a report per tool with documentation addresses in place of this device's, plus `iperf3.json`, which the iOS app makes, and `sparse.json`, a report from an origin that measured little.

## Rules

- `schema` is the version, `1` today. Within a version fields are only added. A change that would break a reader raises the number.
- A reader ignores fields it doesn't know and keeps values it doesn't know as they are: a new `tool`, a new `origin.kind`, a new `outcome`.
- Every figure is optional. `measured`, `hops`, `records`, `sources`, `probe` and `mtu` say what a run found, and a run from a small device finds less than one from a laptop.
- `id` is unique on the install that made the run. An account tells runs apart by the install and this id.
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
- Not in a report today: the DNS delegation path, a BGP lookup's paths, prefixes and history, and iperf3's intervals. Running the tool again on this device brings them.

## What points to a home

A run from a node on someone's line says where that line is. A surface that shares a run leaves these out unless the person sharing keeps them in:

- `origin.publicAddress` and `origin.sourceAddress`.
- The address and hostname of a hop that is the origin itself: a hop at either of those addresses, or an IPv6 hop in the same /56 as one of them, which is the line's own prefix.

The hop stays, with its figures. Everything else in a report is about the path and the target. A location, when a report carries one, belongs to the same list, and is never more precise than a person set it.

## Where it is checked

`cargo test` checks that every field of a report and of each fixture is one the schema names, that each fixture is read and written back unchanged, and that each becomes a run every look can draw. `traceroute.json`, `mtr.json` and `bgp.json` are the reports of runs the tests build, so they can't drift (`RVRS_WRITE_FIXTURES=1 cargo test` rewrites them). The iOS app's tests read copies of the same files. The rvrs API reads the same fixtures against its own copy of the shape.

## The node protocol

`node-protocol/` is the other contract in this folder: how a node talks to the rvrs API and how a person's install asks a node for a run, as a page, an OpenAPI document and fixtures. A node's result is this report, sent through the route every install uses.
