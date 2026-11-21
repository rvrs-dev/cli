# The report

`--json` on a tool, on `rvrs last`, `rvrs show <id>`, `rvrs history`, `rvrs saved` and `rvrs case <name>` prints the same report: one object for a run, an array for a list. `rvrs history mtr example.com --json` is the array of one tool's runs to one target. `rvrs case --json` lists the cases themselves (`name`, `runs`, `run_ids`, `newest_run_at`).

## Every tool

| Field | Holds |
| --- | --- |
| `id` | The run's id in history. `rvrs show <id>` and `rvrs save <id>` take it, and its first characters are enough. |
| `tool` | `Traceroute`, `MTR`, `Ping`, `DNS` or `BGP`. |
| `target` | What was typed. |
| `address` | What the target resolved to. `null` for DNS and for a run that failed to resolve. |
| `family` | `IPv4` or `IPv6`. `null` for DNS and BGP. |
| `startedAt` | UTC, ISO 8601. |
| `origin` | `node` (`null` is this device), `sourceAddress` (the address probes left from) and `publicAddress` (what the internet saw this device as, when lookups were on). |
| `outcome` | `reached`, `exhausted`, `stopped` or `failed`. |
| `summary` | The run in one line of facts: `6 hops · 12.4 ms`. The history list words a DNS run's summary differently from its report. |
| `error` | The reason, when `outcome` is `failed`. |
| `measured` | A list of `{name, value, unit}`. Names differ by tool, below. |
| `interpretation` | rvrs's inferences, each a sentence worded as one. Empty when there is nothing to infer. |
| `sentence` | The run as one plain sentence. |

Read a measurement by its name. The order can change:

```bash
rvrs ping -c 20 -i 0.2 --json example.com | jq '.measured | map({(.name): .value}) | add'
# {"sent":20,"received":20,"loss":0,"min":10.6,"avg":11.7,"max":13,"jitter":1.5,"p50":11.2,"p95":13}
```

## Ping

`measured`: `sent`, `received`, `loss` (%), then `min`, `avg`, `max`, `jitter`, `p50`, `p95` (ms) when anything came back. `hops` is `null`. Jitter is the mean step between consecutive replies. With fewer than about ten packets there is no verdict in `interpretation`.

## MTR

`measured`: `hops`, then the destination's `sent`, `received`, `loss`, `min`, `avg`, `max`, `jitter`, `p50`, `p95`. `-c` counts rounds, one probe to every hop per round, one round a second.

`hops`: one entry per router with `ttl`, `address`, `hostname`, `asn` (`"AS13335"`), `network`, `lossPercent` and `avgMilliseconds`. A hop that never answered has `address: null`.

`interpretation` carries the verdict on the destination and, when middle hops drop replies that later hops deliver, the sentence that names them as rate limiting.

```bash
# where loss that reaches the target begins
rvrs mtr -c 20 --json example.com | jq '{target: .measured | map({(.name): .value}) | add | {loss, avg, p95}, hops: [.hops[] | {ttl, asn, lossPercent, avgMilliseconds}]}'

# the networks the path crosses, in order
rvrs show <id> --json | jq '[.hops[].asn | select(. != null)] | reduce .[] as $a ([]; if .[-1] == $a then . else . + [$a] end)'
```

## Traceroute

Same shape as MTR with three probes per hop by default (`-q`). Too few probes for loss figures to mean much: use it for the path and MTR for loss.

## Traceroute to a TCP port

`rvrs traceroute -T -p 443 <target> --json` is a traceroute whose report also has:

- `probe`: `protocol` (`tcp`), `port`, and `state`: `open` (a SYN-ACK came back), `closed` (a reset came back) or `filtered` (nothing came back from the destination).
- `measured`: `connect` (ms, how long the first handshake took) and `mss` (bytes, what the handshake settled on) for an open port.
- `outcome`: `reached` for an open or a closed port, `exhausted` for a filtered one. `hops` then ends at the last router that answered.
- `interpretation`: for a filtered port, the sentence that names that router and its network.

An MSS under 1460 over IPv4 means the path or the server announced less than a 1500-byte MTU. That alone is common (tunnels, PPPoE) and no fault. It matters when large transfers or TLS handshakes hang while small requests work.

## DNS

`rvrs dig <name> --json` asks this device's resolver, Cloudflare, Google, Quad9 and the name's authoritative servers.

- `measured`: `addresses`, `sources`, `fastest` (ms).
- `records`: the reference answer, the authoritative one when it was reached: `type`, `name`, `ttl`, `value`. It holds every type that was asked, so expect NS, SOA, MX, TXT, HTTPS and RRSIG beside A and AAAA. For addresses alone, read `sources[].addresses`.
- `sources`: one entry per source with `name`, `kind` (`device`, `public`, `authoritative`), `server`, `status` (`match`, `differs`, `error` against the reference), `milliseconds`, `addresses` (its A and AAAA values, sorted) and `error` (a failure to ask, or the rcode by type, as in `A: SERVFAIL`).
- `summary` ends in `signed` or `unsigned`: whether DNSSEC records are present.

```bash
rvrs dig example.com --json | jq '.sources[] | select(.status != "match") | {name, server, status, addresses, error}'
```

With `@server`, a record type or a `+option`, `rvrs dig` asks one question and prints what dig prints. That form is not a run, is not kept and has no `--json`. Use it to check one record at one server: `rvrs dig @9.9.9.9 example.com MX +short`.

## BGP

`rvrs bgp <host|address|prefix|ASN> --json` reads RIPE RIS collectors through RIPEstat. Nothing is probed.

- `measured`: `peers seeing` and `peers total` (collector peers that see the prefix), `distinct paths`, `origin asn`, and for the origin network `ipv4 prefixes` and `ipv6 prefixes`.
- `summary`: `AS13335 · Valid · 270/271 peers`. The middle word is the origin validation: Valid, Invalid, Not Found or Unknown.
- `interpretation`: the verdict with its evidence.
- Exit code 1 and `outcome: "exhausted"` mean no route was observed for the target.

AS paths, the history and the sources with their observation times are in the text output, which a pipe gets without color:

```bash
rvrs bgp 192.0.2.0/24 --history --days 90
```

It prints the prefix, each origin AS with its RPKI result, the most common AS paths with the number of collector peers on each, the origin network, a `;;` line per data source with when it was observed, and under `Events` every change of origin AS or visibility in the window, or `No changes`. Times come from 8-hour collector snapshots.

`rvrs bgp check <prefix> <ASN>` prints the three checks of origin validation and the covering ROAs. Exit code 0 is Valid, 1 anything else.

## Comparing two runs

There is no compare command yet. Put the two reports side by side:

```bash
for id in fvx2vn 2u8gqj; do
  rvrs show "$id" --json | jq -c '{id, startedAt, address, origin: .origin.sourceAddress, family, m: (.measured | map({(.name): .value}) | add | {loss, avg, p95, jitter, hops}), path: ([.hops[]?.asn | select(. != null)] | reduce .[] as $a ([]; if .[-1] == $a then . else . + [$a] end))}'
done
```

`address` is in the output on purpose: two runs to one name that resolved to different addresses measured different destinations. `path` is the networks in the order the hops cross them, with repeats collapsed, so a path that visits the same networks in another order shows as different. A ping has no `hops`, so its `path` is empty. State the difference as numbers with both ids. A move of a millisecond or two between two short runs is noise. Look for loss that appears, a p95 that pulls away from the p50, a hop count that changes, or a network that enters or leaves the path.
