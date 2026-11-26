# The report

`get_run` answers with a run's `report`: the run as data, written by the device or node that made it and cut by what you read. Every rvrs surface reads the same report. Fields are only added, and a reader ignores the ones it doesn't know.

## Every tool

| Field | Holds |
| --- | --- |
| `id` | The id the run's maker gave it. The run's id in the account, which `get_run` and `compare_runs` take, is the reply's own `id`. |
| `tool` | The word the maker wrote: `Ping`, `Traceroute`, `MTR`, `DNS`, `BGP`, or `iPerf3` from the iPhone app. `find_runs` takes them in lower case. |
| `target` | What was typed. |
| `address` | What the target resolved to. Absent for DNS, BGP and a run that failed to resolve. |
| `family` | `IPv4` or `IPv6`. Absent for DNS and BGP. |
| `startedAt` | UTC, ISO 8601, by the maker's clock. |
| `origin` | `kind` (`device` or `node`), `node` (the node's name), `tags` (the origin's tags when the run was made), and `sourceAddress`, `publicAddress` and `location` while the person lets you read them. |
| `outcome` | `reached`, `exhausted`, `stopped`, `failed` or `refused`. |
| `summary` | The run in one line of facts: `6 hops · 12.4 ms`. |
| `error` | The reason, when `outcome` is `failed`. |
| `measured` | A list of `{name, value, unit}`. Names differ by tool, below. Read a figure by its name, since the order can change. |
| `interpretation` | rvrs's inferences, each a sentence worded as one. Empty when there is nothing to infer. |
| `sentence` | The run as one plain sentence. |

A field the maker didn't fill is absent, never zero. An older CLI or a node that measured little sends less, and that says nothing about the network.

## Ping

`measured`: `sent`, `received`, `loss` (%), then `min`, `avg`, `max`, `jitter`, `p50`, `p95` (ms) when anything came back. With `detail` `full`, `samples` is the round trip of each packet in the order it was sent, `null` for one that got no reply, so you can say when in the run a loss or a spike happened. Jitter is the mean step between consecutive replies. With fewer than about ten packets there is no verdict in `interpretation`.

## MTR

`measured`: `hops`, then the destination's `sent`, `received`, `loss`, `min`, `avg`, `max`, `jitter`, `p50`, `p95`. One round sends a probe to every hop, one round a second.

`hops`: one entry per router with `ttl`, `address`, `hostname`, `asn` (`"AS13335"`), `network`, `lossPercent`, `avgMilliseconds`, `minMilliseconds`, `maxMilliseconds`, `jitterMilliseconds`, `sent` and `received`. With `detail` `full`, each hop also has `samples`. A hop that never answered has no address. A hop in the private network has its figures and no address or name while `privateNetworks` is off.

`interpretation` carries the verdict on the destination and, when middle hops drop replies that later hops deliver, the sentence that names them as rate limiting.

## Traceroute

The same shape as MTR, with about three probes per hop. That is too few for its loss figures to mean much, so read it for the path and an MTR for loss.

A traceroute to a TCP port also has `probe`: `protocol` (`tcp`), `port`, and `state`, which is `open` (a SYN-ACK came back), `closed` (a reset came back) or `filtered` (nothing came back from the destination). An open port adds `connect` (ms) and `mss` (bytes) to `measured`. A filtered port ends the hops at the last router that answered, and `interpretation` names it.

A path MTU check also has `mtu`: `bytes` (the largest packet that crossed unfragmented), `finding` and `limitedBy`. `full` means the device's own MTU crossed, `limited` that a router named a smaller size, which is path MTU discovery working, `silent` that larger packets vanish with no message, and `unknown` that the target answered no ping.

## DNS

The maker asked its own resolver, public resolvers and the name's authoritative servers.

- `measured`: `addresses`, `sources`, `fastest` (ms).
- `records`: the reference answer, the authoritative one when it was reached: `type`, `name`, `ttl`, `value`.
- `sources`: one per source with `name`, `kind` (`device`, `public`, `authoritative`), `server`, `status` (`match`, `differs`, `error` against the reference), `milliseconds`, `addresses` and `error`.
- `summary` ends in `signed` or `unsigned`: whether DNSSEC records are present.

## BGP

The maker read it from RIPE RIS route collectors and sent no probe.

- `measured`: `peers seeing` and `peers total` (the collector peers that see the prefix), `distinct paths`, `origin asn`, and for the origin network `ipv4 prefixes` and `ipv6 prefixes`.
- `summary`: `AS13335 · Valid · 270/271 peers`. The middle word is the origin validation, one of Valid, Invalid, Unknown and Not Found.
- `outcome` `exhausted` means no route was observed for the target.

## iperf3

The iPhone app makes these. `measured` holds the throughput in Mbps by direction, such as `download peak` and `upload peak`, and `target` names the server and its port. Always say the direction and the server, and never call it internet speed.

## In a list

`find_runs` answers with one summary per run: `id`, `tool`, `target`, `state` (`ended`, or `requested`, `running` or `expired` for a run asked of a node), `outcome`, `summary`, `startedAt`, `origin`, `trigger` (`manual`, `request` or `schedule`), `scheduleId`, `address`, `family` and `link`. Read the run with `get_run` before you cite a figure the summary doesn't state.

## In a comparison

`compare_runs` answers with the runs, `differs` (which of `tool`, `target`, `address`, `family`, `origin` and `count` they don't share), `figures` (each figure per run, with its distance from the baseline) and `paths` (for each run after the baseline, the first TTL where its path leaves the baseline's, or null when no hop differs). A move of a millisecond or two between two short runs is noise. Look for loss that appears, a p95 that pulls away from the p50, a hop count that changes, or a network that enters or leaves the path.

## In a summary

`summarize_runs` answers with `groups`, one per origin or target: `runs` and `runsBefore`, `median` and `medianBefore` (the median of the runs' average round trips, in ms), `jitter`, `loss` (the average share of probes lost, in percent), `withLoss`, `failed` and `worthALook`. Then `latency`: for ping, traceroute and MTR runs, one point per step with `runs`, `latency`, `jitter`, `loss`, `withLoss` and `failed`, oldest first, and the window's figures. A step with no run has no point.
