# Nodes

A node is a machine somewhere else that makes runs for you: a server, a Raspberry Pi, a box at home. It is the same rvrs, started as `rvrs node`. You ask it for a run with `--node`, and the result comes back to your terminal and into your account's history. With `rvrs schedule` it repeats a run at an interval.

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
- The volume keeps the node's outbox and its copy of its schedules. Results that wait for the uplink outlast a restart, and so do the schedules.
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

## Schedules

A schedule is a run your nodes repeat at an interval. You make it once, and each node keeps it and repeats the run by itself.

```bash
rvrs schedule add ping -c 10 example.com --every 1m --node fra-1 --node cpe-4471
rvrs schedule add mtr example.com --every 5m --fleet "isp=telia,line in (ftth,cable)"
rvrs schedule add dig example.com --every 1h --all-nodes --name "Hourly DNS"
rvrs schedules                   # what you made, where it runs and when it last ran
rvrs schedule show 7a1c9e52      # one schedule
rvrs schedule pause 7a1c9e52     # no node runs it until rvrs schedule resume
rvrs schedule rm 7a1c9e52        # deletes it and keeps the runs it made
```

Making a schedule takes an account: `rvrs login`. The start of an id is enough, as `rvrs schedules` shows it.

- `--every` is the time between two runs on one node, from 60 seconds to 24 hours: `300`, `90s`, `5m` or `2h`.
- A schedule runs in one of three places. `--node` names a node and can be repeated, for up to 50 nodes. `--fleet` takes a selector and runs on every node whose tags match. `--all-nodes` runs on every node of the workspace.
- A selector is terms joined by commas, which mean and: `isp=telia`, `isp!=telia`, `isp` (has the tag), `!isp` (lacks it), `line in (ftth,cable)` and `line notin (dsl,lte)`. A node that gains the tag starts the schedule, and one that loses it stops.
- The tool's flags are the ones that travel with `--node`, in the table above. A schedule's tool and target never change. Another target is another schedule.
- The runs of a schedule are kept in your account as that schedule's runs. They stay out of `rvrs history`, which lists the runs a person made.
- The API gives every node its own place in the interval, so the nodes of one schedule never all start in the same second.
- One target gets at most 10 scheduled run starts a second from a workspace. When a schedule would start more, each node runs it at a longer interval, and `rvrs schedules` shows that interval beside the one you asked for.

What a node does with a schedule:

- It reads its schedules when it connects and when one of them changes, and keeps its copy in the state folder, beside the outbox.
- It starts each run by a clock of its own that only counts forward. So it goes on while the uplink is gone: each result waits in the outbox and goes out once the uplink is back, oldest first, with how long ago its run started.
- A node that starts without its uplink starts from the copy it kept, each schedule at a random time within its interval.
- A scheduled run counts against the node's limits and passes the checks a run asked with `--node` passes: the tool, every option, the limits and the target.
- A run is due once. When the node is at a limit, or the schedule's last run is still going, the run is left out. A node never makes up a run it missed, also after a restart or a day without power.
- A schedule the node won't run is refused once. The node sends one result with the outcome `refused` and the reason, and starts no run of that schedule until the schedule changes or the node restarts.
- A copy is good for 7 days without the API. After that the node starts no scheduled run until it has read its schedules again.

## What the owner sets

Whoever starts the node sets these, as flags of `rvrs node` or in the environment. The API can change none of them.

| What | Flag | Environment |
| --- | --- | --- |
| Where the outbox, the schedules and the token are kept | `--state <folder>` | `RVRS_NODE_STATE` |
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
| The uplink is gone and it has schedules | Goes on with each schedule by its own clock. The results wait in the outbox with the rest |
| It couldn't read its schedules for 7 days | Starts no scheduled run until it has read them again |
| Its token was revoked or replaced | Ends its runs, deletes its copy of its schedules and makes no further call. Give it the new token and start it again |
| An admin stopped the fleet | Ends its runs, keeps its results and its schedules, and asks again in an hour. No scheduled run starts until the fleet is resumed |
| It was asked for a run it won't make | Answers with the reason: a private address, an option it doesn't know, a run over its limits |
| A schedule names a run it won't make | Sends one result with the reason and starts no run of that schedule until the schedule changes or the node restarts |
| A scheduled run is due while the node is at a limit, or while the schedule's last run is still going | Leaves that run out and says so in its log. It makes up no run |
| The API it talks to has no schedules yet | Runs without schedules and asks again when it next connects |
| It restarted in the middle of a run | Gets the run again, as the next attempt |
| It lost power while it wrote a result | Starts with the results that were written whole. A file that holds no result is removed and counted as dropped |
| The outbox's folder can't be written | Keeps the result in memory and says so in its log. Its heartbeat says `memory` until that result is sent |

The node's log names runs and schedules by their id. It never has a target, a token or anything from a report in it.
