# Commands

Every command rvrs has, the report `--json` prints, and the files rvrs keeps.

| Command | Does |
| --- | --- |
| `rvrs` | Opens the Console |
| `rvrs <target>` | Opens the Console and traces the target |
| `rvrs traceroute [flags] <target>` | Traces and prints the result. With `-T -p <port>` it probes with TCP: is the port open, closed or filtered, and where a SYN stops |
| `rvrs ping [flags] <target>` | Pings until Ctrl-C, or `-c` packets |
| `rvrs mtr [flags] <target>` | Measures loss and latency at every hop, round after round, until Ctrl-C or `-c` rounds |
| `rvrs dig <name>` | Looks the name up at every source and compares the answers. `rvrs dns` is the same command |
| `rvrs dig [@server] <name> [type] [+options]` | Asks one question and prints what dig prints |
| `rvrs bgp <host\|address\|prefix\|ASN>` | Looks up the route, its origin validation, AS paths, history and network |
| `rvrs bgp check <prefix> <ASN>` | Says whether that route would pass RPKI origin validation |
| `rvrs bgp glass` | Lists your looking glasses. `glass add`, `glass remove` and `glass test` manage them |
| `rvrs last` | Shows the newest run |
| `rvrs show <id>` | Shows a run from history. The first characters of the id are enough |
| `rvrs history [tool] [target]` | Lists the runs on this device, all of them or those of a tool, a target or both: `rvrs history mtr example.com`. A target matches on any part of it. `rvrs history clear` deletes them |
| `rvrs save [id] [--case <name>]` | Saves the newest run, or the one named, into the case used last or the one named |
| `rvrs saved` | Lists saved runs. `rvrs saved remove <id>` takes one out |
| `rvrs case` | Lists the cases |
| `rvrs case <name>` | Lists the runs in a case |
| `rvrs case new\|rename\|delete <name>` | Starts, renames or deletes a case. Deleting a case keeps its runs saved |
| `rvrs case add\|remove <name> [id]` | Puts a run in a case or takes it out |
| `rvrs favorite <host>` | Adds a host to Favorites |
| `rvrs favorites` | Lists favorite hosts with their newest run |
| `rvrs status` | Checks that this device can run the tools |
| `rvrs completions install` | Sets up tab completion for zsh, bash or fish |
| `rvrs update` | Brings rvrs up to date, however it was installed |

Flags on every run: `--json` prints the report, `--classic` prints the original tool's output, `--color auto|always|never` sets color, and `--case <name>` saves the run into a case. `NO_COLOR` is honored.

### Soon

`rvrs help` lists these with a Soon label. Until they exist, each one says so and sends nothing.

| Command | Will do |
| --- | --- |
| `rvrs iperf3` | Download and upload throughput against an iperf3 server, as the rvrs app measures it |
| `rvrs node` | Runs this machine as a node in your fleet |
| `rvrs nodes` | Lists the nodes in your fleet. `nodes show`, `nodes rename` and `nodes remove` manage one |
| `rvrs schedule <tool> <target> --node <name> --every <time>` | Has a node repeat a run |
| `rvrs schedules`, `rvrs alerts` | List the fleet's schedules and the alerts they sent |
| `rvrs login`, `rvrs logout`, `rvrs share` | Sign in to your workspace, sign out, print a run's share link |
| `--node <name>` | Runs any tool from a node instead of this device |
| `--fleet` | Runs it from every node in the fleet |

## The report

`--json` prints the same `RunReport` the iOS app returns to Shortcuts. `rvrs last`, `rvrs show <id>`, `rvrs history`, `rvrs saved` and `rvrs case <name>` take it too, a list as an array.

| Field | Holds |
| --- | --- |
| `id` | The run's id in history, which `rvrs show` and `rvrs save` take |
| `tool`, `target`, `address`, `family`, `startedAt` | What ran, against what, and when (UTC) |
| `origin` | Where the run was made from: `node` (`null` is this device), `sourceAddress` and `publicAddress` |
| `outcome` | `reached`, `exhausted`, `stopped` or `failed`, with the reason in `error` |
| `measured` | What was measured, as `name`, `value` and `unit`. For a BGP lookup: the collector peers that see the prefix, the distinct paths and the origin AS number |
| `hops` | Traceroute and MTR: each hop's address, name, network, loss and average |
| `records` | DNS: the reference answer |
| `probe` | A TCP traceroute: `protocol`, `port` and `state` (`open`, `closed` or `filtered`) |
| `sources` | DNS: what each source answered (`addresses`), how long it took and whether it matches the reference (`match`, `differs`, `error`) |
| `interpretation` | What it likely means. Inferences only, each worded as one |
| `summary`, `sentence` | The run in one line of facts, and as one plain sentence |

```bash
rvrs traceroute --json example.com | jq '.hops[] | {ttl, hostname, asn, avgMilliseconds}'
rvrs dig example.com --json | jq '.sources[] | select(.status != "match")'
rvrs last --json | jq .sentence
```

A report prints whatever the outcome, and the error goes to stderr. [rvrs for agents](agents.md) has more on reading it from a script or an agent.

## Files

| What | Where |
| --- | --- |
| History, 500 runs at most | `~/.local/share/rvrs/history.json` |
| Saved runs | `~/.local/share/rvrs/saved.json` |
| Cases | `~/.local/share/rvrs/cases.json` |
| Favorites | `~/.local/share/rvrs/favorites.json` |
| Your looking glasses | `~/.config/rvrs/looking-glasses.json` |
| The Console's last tool | `~/.config/rvrs/console.json` |
| Network lookups, kept a week | `~/.cache/rvrs/networks.json` |
| What the last look for an update found | `~/.cache/rvrs/update.json` |

`XDG_DATA_HOME`, `XDG_CONFIG_HOME` and `XDG_CACHE_HOME` are honored. `RVRS_HOME=<dir>` puts everything in one folder. `RVRS_HISTORY=0` keeps a run out of history.

## Updates

`rvrs update` updates rvrs the way it was installed: the newest release for a binary from the installer, `git pull` and `cargo build` for a checkout. Homebrew and the `.deb` and `.rpm` packages are updated by their package managers. `rvrs update --check` only checks.

A release download is checked against the release's `SHA256SUMS` before it replaces anything, by the installer and by `rvrs update` alike.

Once a day rvrs checks on its own, and a result ends with one line when a newer version is out. `RVRS_UPDATE_CHECK=0` turns that off.

## Versions

`VERSION` holds major.minor, and the build adds the commits since `VERSION` last changed (`crates/rvrs/build.rs`). `rvrs --version` prints both, as in `rvrs 0.5.3 (4ea5d4e)`.
