# Nodes

A node is a machine somewhere else that makes runs for you: a server, a Raspberry Pi, a box at home. It is the same rvrs, started as `rvrs node`. You ask it for a run with `--node`, and the result comes back to your terminal and into your account's history.

A node opens every connection itself, so it needs no open port and works behind NAT.

## Start a node

1. Make a token at [app.rvrs.dev](https://app.rvrs.dev), under Installs, New Token. The token's name becomes the node's name.
2. Start the node with it.

```bash
docker run -d --name rvrs-node --restart unless-stopped --network host \
  -e RVRS_TOKEN=<token> -v rvrs-node:/var/lib/rvrs ghcr.io/rvrs-dev/rvrs
```

Without Docker, on a machine that has rvrs:

```bash
RVRS_TOKEN=<token> rvrs node
```

- `--network host` has the runs start from the machine's own address, with IPv6 where the machine has it. On Docker's bridge every run would start one hop further away, from 172.17.0.x, without IPv6.
- The volume keeps the node's outbox. Results that wait for the uplink outlast a restart.
- The image is for x86_64 and arm64. It holds the [small build](build-features.md) and nothing else.
- A node uses its own token and never the account of someone signed in on the same machine. Without a token it makes no call at all.

## Ask a node for a run

```bash
rvrs nodes                                    # the workspace's nodes, and who is online
rvrs mtr example.com --node fra-1             # run it there, print the result here
rvrs traceroute -T -p 443 example.com --node fra-1
rvrs ping -c 20 example.com --node fra-1 --no-wait   # print the run's id and leave
```

Asking takes an account: `rvrs login`. The command says where the run stands, then prints the result as it prints any run. Ctrl-C ends the waiting and leaves the run going. It lands in history either way.

A request waits 300 seconds for its node. `--expires-in <seconds>` changes that, from 30 to 86400. A request to a node that stays offline expires and says so.

These flags travel to a node. Any other flag of the tool is refused with `--node`, since the run would measure something else than you asked.

| Tool | Flags |
| --- | --- |
| traceroute | `-4`, `-6`, `-q`, `-w`, `-m`, `-f`, `-T`, `-p`, `--mtu`, `-n`, `--no-lookups` |
| ping | `-4`, `-6`, `-c`, `-i`, `-W`, `-n`, `--no-lookups` |
| mtr | `-4`, `-6`, `-c`, `-i`, `-Z`, `-m`, `-f`, `-n`, `--no-lookups` |
| dig | the record type, `--resolver` |
| bgp | none |

Ping and MTR send 10 packets or rounds on a node when you name no count, since a run on a node always ends on its own.

## What the owner sets

Whoever starts the node sets these, as flags of `rvrs node` or in the environment. The API can change none of them.

| What | Flag | Environment |
| --- | --- | --- |
| Where the outbox and the token are kept | `--state <folder>` | `RVRS_NODE_STATE` |
| Tags | `--tag key=value`, repeatable | `RVRS_NODE_TAGS` |
| Location | `--location <lat>,<lon>`, `--place`, `--location-precision` | `RVRS_NODE_LOCATION`, `RVRS_NODE_PLACE`, `RVRS_NODE_LOCATION_PRECISION` |
| Private ranges the node may probe | `--allow <range>`, repeatable | `RVRS_NODE_ALLOW` |
| Limits | `--limit <name>=<value>`, repeatable | `RVRS_NODE_LIMITS` |
| The token | a file named `token` in the state folder | `RVRS_TOKEN` |
| Another API than api.rvrs.dev | | `RVRS_API` |

[The node's configuration](../schema/node-config.md) has the rules for tags and location. [What a node does and what it refuses](node-safety.md) has the limits, their ceilings and the addresses a node sends nothing to.

## When something is in the way

| What happened | The node |
| --- | --- |
| The uplink is gone | Keeps its results in the outbox, 500 runs, 64 MiB and 7 days at most, and sends them once it is back |
| Its token was revoked or replaced | Ends its runs and makes no further call. Give it the new token and start it again |
| An admin stopped the fleet | Ends its runs, keeps its results and asks again in an hour |
| It was asked for a run it won't make | Answers with the reason: a private address, an option it doesn't know, a run over its limits |
| It restarted in the middle of a run | Gets the run again, as the next attempt |
| It lost power while it wrote a result | Starts with the results that were written whole. A file that holds no result is removed and counted as dropped |
| The outbox's folder can't be written | Keeps the result in memory and says so in its log. Its heartbeat says `memory` until that result is sent |

The node's log names runs by their id. It never has a target, a token or anything from a report in it.
