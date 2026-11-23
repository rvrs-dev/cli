# What a node does and what it refuses

Nodes are not released. This page says what `rvrs node` is built to, so the rules are public before the first node runs on someone's line. [The node protocol](../schema/node-protocol/README.md) has the routes.

A node is a machine that makes runs on request: a server, a virtual machine, a router. It runs the tools `rvrs` runs on a laptop. Every rule on this page is checked on the node, by its own build, with settings only the node's owner writes. No request, claim or event from the API changes one of them.

## What a node runs

- The tools its build has: traceroute, ping, MTR, DNS and BGP, less the [build features](build-features.md) it was built without.
- A request names a tool and options from a fixed list ([What a run is](../schema/node-protocol/README.md#what-a-run-is)). The node turns them into the options of its own engine. It never runs a command, a shell line or a flag it was sent.
- A tool or an option the node doesn't know is refused. Running without the option would measure something else than what was asked.
- Ping and MTR go on until stopped on a laptop. On a node they send 10 packets or rounds unless the request names a number.

## Limits

A node keeps these whatever it is asked, and says them in its heartbeat.

| Limit | Holds | Default | Ceiling |
| --- | --- | ---: | ---: |
| `runsAtOnce` | Runs at the same time | 2 | 8 |
| `runsPerMinute` | Runs started in any 60 seconds | 12 | 60 |
| `packetsPerRun` | Probes one run sends | 1,000 | 10,000 |
| `secondsPerRun` | How long one run may take | 300 | 3,600 |
| `reportBytes` | The largest report it sends | 262,144 | 1,900,000 |

- A request for more packets or more time than the limit is refused before a packet is sent. An MTR of 100 rounds to 30 hops is 3,000 packets.
- The packets of a ping and the rounds of an MTR are 200 ms apart or slower.
- A run still going at `secondsPerRun` is ended there, with what it measured.
- A probe is a packet of a traceroute, a ping or an MTR, toward the target and the routers on the way. The questions of a DNS lookup and the requests of a BGP lookup are not probes and don't count toward `packetsPerRun`. Their number is the tool's own: a request names one record type and at most one resolver more, and nothing that multiplies them. A node set to 0 packets still makes lookups, and 0 runs at once turns every tool off.
- `secondsPerRun` ends every run, a lookup as much as an MTR.
- A report over `reportBytes` loses its samples first, then its path, then everything but how the run ended, and says so. What is left of any report is under 4,096 bytes, which is the least `reportBytes` can be set to. The node checks the size after the last cut, and a report still over the limit is never sent.
- The owner can lower a limit, and raise it as far as the ceiling. The ceiling is in the build, so no setting and no API makes a node send more.
- A node asks for no more runs than it has room for. A run handed to it beyond that is refused.

## Where a node sends packets

A node probes public addresses.

| Address space | A node |
| --- | --- |
| Public | Probes it |
| Private: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, fc00::/7 | Refuses it, unless its owner allowed a range that holds the address |
| Shared: 100.64.0.0/10 | Refuses it, unless its owner allowed a range that holds the address |
| Loopback, link-local, multicast, broadcast | Always refuses it |
| Reserved: documentation ranges, 0.0.0.0/8, 240.0.0.0/4, unallocated IPv6 | Always refuses it |

- An IPv4 address inside an IPv6 one (IPv4-mapped, NAT64, 6to4) is judged as the IPv4 address.
- A target is checked twice: as typed when the request arrives, and again after its name resolves. The node resolves the name once and hands its engine the address it checked, so the address that was checked is the address that is probed. A name that resolves to a private address the next time is refused then.
- A name of the node's own network is refused before the node asks its resolver: a name of one label, and names under `local`, `lan`, `home`, `home.arpa`, `internal`, `intranet`, `corp`, `private`, `localdomain`, `localhost`, `test`, `invalid`, `onion` and `alt`. A request can't be used to learn which devices are on the line.
- A resolver named in a DNS request is an address the node sends questions to, and is held to the same rule.
- A BGP lookup sends nothing to its target. It asks RIPEstat.
- The refusal for a name says what kind of address it resolved to and never which one.

### Allowed ranges

A node's owner can allow ranges of private or shared address space, for diagnostics on their own network. They are written where the node is set up, with `--allow` on `rvrs node` or in `RVRS_NODE_ALLOW`:

```
RVRS_NODE_ALLOW=192.168.1.0/24,fd12:3456::/32
```

- The API never sets a range. A request that carries one is refused as an option the node doesn't know.
- A range opens private and shared space only. `0.0.0.0/0` still leaves loopback, link-local, multicast and reserved addresses refused.
- With a range allowed, a name of the node's own network is looked up and judged by the address it resolves to.

## A refusal

A run the node won't make ends with a report whose outcome is `refused`, sent the way every result is sent. `error` says why.

```json
{
  "tool": "Ping",
  "target": "192.168.1.1",
  "origin": { "kind": "node", "node": "fra-1" },
  "outcome": "refused",
  "summary": "Refused · fra-1 sends nothing to private addresses",
  "error": "192.168.1.1 is a private address, and fra-1's owner allowed no range that holds it."
}
```

## Setting a node up, and turning it off

- A node enrolls and sends only after someone set it up: with a token of its own, or with an enrollment token it trades for one. The API's address alone does neither. A binary on a device nobody set up makes no call to the API.
- A node that was turned off on its device makes no call either, whatever token it still holds.
- Revoking a node in the web app ends its stream, and every later call with its token is refused. An admin can stop every node of a workspace at once. A node that is told so ends the runs it is making.
- A log line names a run by its id and says what kind of rule refused it. It carries no token, no target and nothing from a report. The same holds for a heartbeat.

## For the person on the line

A node on your line was put there by whoever runs it: you, your team or your provider. This is what it does there.

- It measures the path, latency and loss to targets its workspace asks for, what resolvers answer for a name, and what route collectors see for a prefix.
- It sends only its own probes and reads only the replies to them. It never looks at traffic that isn't its own, and it never lists the devices on your network.
- A report carries the target, the addresses on the path, the address the probes left from and your line's public address. The first hops of a path are your own router and your provider's.
- The runs belong to the node's workspace. What points to your line is for the workspace's admins and is left out when a run is shared, unless someone keeps it in: the public address, the address the probes left from, your router and the devices of your network, and a location.
- It sends nothing to devices on your network unless the node's owner allowed that range.
- You can turn it off on the device. Turned off, it makes no call until it is turned on again.
