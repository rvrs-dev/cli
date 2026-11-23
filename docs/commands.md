# Commands

Every command rvrs has, the report `--json` prints, and the files rvrs keeps.

| Command | Does |
| --- | --- |
| `rvrs` | Opens the Console |
| `rvrs <target>` | Opens the Console and traces the target |
| `rvrs traceroute [flags] <target>` | Traces and prints the result. With `-T -p <port>` it probes with TCP: is the port open, closed or filtered, and where a SYN stops. With `--mtu` it finds the path MTU |
| `rvrs ping [flags] <target>` | Pings until Ctrl-C, or `-c` packets |
| `rvrs mtr [flags] <target>` | Measures loss and latency at every hop, round after round, until Ctrl-C or `-c` rounds |
| `rvrs dig <name>` | Looks the name up at every source and compares the answers. `rvrs dns` is the same command |
| `rvrs dig [@server] <name> [type] [+options]` | Asks one question and prints what dig prints |
| `rvrs bgp <host\|address\|prefix\|ASN>` | Looks up the route, its origin validation, AS paths, history and network |
| `rvrs bgp check <prefix> <ASN>` | Says whether that route would pass RPKI origin validation |
| `rvrs bgp glass` | Lists your looking glasses. `glass add`, `glass remove` and `glass test` manage them |
| `rvrs last` | Shows the newest run |
| `rvrs show <id>` | Shows a run from history. The first characters of the id are enough, and `rvrs history` lists six of its 26 |
| `rvrs history [tool] [target]` | Lists the runs on this device, all of them or those of a tool, a target or both: `rvrs history mtr example.com`. A target matches on any part of it. `rvrs history clear` deletes them |
| `rvrs save [id] [--case <name>]` | Saves the newest run, or the one named, into the case used last or the one named |
| `rvrs saved` | Lists saved runs. `rvrs saved remove <id>` takes one out |
| `rvrs case` | Lists the cases |
| `rvrs case <name>` | Lists the runs in a case |
| `rvrs case new\|rename\|delete <name>` | Starts, renames or deletes a case. Deleting a case keeps its runs saved |
| `rvrs case add\|remove <name> [id]` | Puts a run in a case or takes it out |
| `rvrs favorite <host>` | Adds a host to Favorites |
| `rvrs favorites` | Lists favorite hosts with their newest run |
| `rvrs login` | Signs this install in to your workspace: it shows a code, you confirm it in the browser. `--no-browser` only prints the link |
| `rvrs logout` | Signs out and revokes this install's credential |
| `rvrs sync` | Sends this device's runs to your account and reads what is new, now, and says what went each way |
| `rvrs nodes` | Lists the nodes in your fleet: who is online, when each was last seen, its version and its tags |
| `rvrs node` | Runs this machine as a node. It takes a token as `RVRS_TOKEN`, makes the runs asked of it and sends each result. [Nodes](node.md) has the flags |
| `rvrs status` | Checks that this device can run the tools, lists the build features of this build and says who is signed in |
| `rvrs completions install` | Sets up tab completion for zsh, bash or fish |
| `rvrs update` | Brings rvrs up to date, however it was installed. `--with` and `--without` change its [build features](build-features.md) |

Flags on every run: `--json` prints the report, `--classic` prints the original tool's output, `--color auto|always|never` sets color, and `--case <name>` saves the run into a case. `NO_COLOR` is honored.

`--node <name>` makes the run on a node instead of this device and prints the result here. With it, `--no-wait` prints the run's id and leaves, and `--expires-in <seconds>` says how long the request waits for the node. [Nodes](node.md) lists the flags of each tool that travel.

### Soon

`rvrs help` lists these with a Soon label. Until they exist, each one says so and sends nothing.

| Command | Will do |
| --- | --- |
| `rvrs iperf3` | Download and upload throughput against an iperf3 server, as the rvrs app measures it |
| `rvrs nodes show\|rename\|remove <name>` | Shows a node, renames it or takes it out of the fleet |
| `rvrs schedule <tool> <target> --node <name> --every <time>` | Has a node repeat a run |
| `rvrs schedules`, `rvrs alerts` | List the fleet's schedules and the alerts they sent |
| `rvrs share` | Print a run's share link |
| `--fleet` | Runs it from every node in the fleet |

## The report

`--json` prints the same `RunReport` the iOS app returns to Shortcuts. `rvrs last`, `rvrs show <id>`, `rvrs history`, `rvrs saved` and `rvrs case <name>` take it too, a list as an array.

| Field | Holds |
| --- | --- |
| `id` | The run's id in history, which `rvrs show` and `rvrs save` take |
| `tool`, `target`, `address`, `family`, `startedAt` | What ran, against what, and when (UTC) |
| `origin` | Where the run was made from: `node` (`null` is this device), `sourceAddress` and `publicAddress`. A run from a node stored in an account also has the node's `tags` and `location` as they were that day |
| `outcome` | `reached`, `exhausted`, `stopped` or `failed`, with the reason in `error` |
| `measured` | What was measured, as `name`, `value` and `unit`. For a BGP lookup: the collector peers that see the prefix, the distinct paths and the origin AS number |
| `hops` | Traceroute and MTR: each hop's address, name, network, loss and average |
| `records` | DNS: the reference answer |
| `mtu` | `traceroute --mtu`: `bytes`, `finding` (`full`, `limited`, `silent` or `unknown`), `limitedBy` and `tried` |
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
| Runs your other installs made, signed in | `~/.local/share/rvrs/account-runs.json` |
| Runs waiting to go to your account | `~/.local/share/rvrs/outbox/` |
| Your looking glasses | `~/.config/rvrs/looking-glasses.json` |
| The Console's last tool | `~/.config/rvrs/console.json` |
| This install's credential, after `rvrs login` | `~/.config/rvrs/credentials.json` |
| Network lookups, kept a week | `~/.cache/rvrs/networks.json` |
| What the last look for an update found | `~/.cache/rvrs/update.json` |

`XDG_DATA_HOME`, `XDG_CONFIG_HOME` and `XDG_CACHE_HOME` are honored. `RVRS_HOME=<dir>` puts everything in one folder. `RVRS_HISTORY=0` keeps a run out of history.

## Accounts

Every tool runs without an account, and runs stay in this device's history whether you sign in or not.

Signed in, every run you make is also kept in your account, where app.rvrs.dev lists it under Runs. Saved runs, cases and favorites are kept there too, so they are the same on every device you sign in on. A device you sign in on later gets your newest 200 runs from the other devices, every saved run, and from then on what is new. `rvrs history` names the install a run came from in a FROM column, and `--json` carries it as `origin.install`. Runs made in the iOS app are among them: `rvrs show` and the Console open them from their report, with the app's own summary and interpretation. A DNS or BGP lookup and an iperf3 test from the app show their figures, records and sources. Run the tool again here for the rest.

rvrs syncs on its own: after each run, and at most every ten minutes when it starts. `rvrs sync` does it now and says what went each way. A run made offline waits on this device and goes with the next sync, for up to a week. `RVRS_SYNC=0` turns syncing off, so a signed-in install keeps its runs to itself.

An install that signs in with `RVRS_TOKEN` only sends its runs. It reads nothing from the account.

`rvrs login` asks the rvrs API for a login, prints a code of eight characters and a link, and opens the link. Sign in at app.rvrs.dev, or make your account there, and confirm the code. The command has been asking every three seconds and now receives this install's credential, which it keeps in `~/.config/rvrs/credentials.json`, readable by you alone. The code lives ten minutes.

On a device nobody sits at, make a token under Installs at app.rvrs.dev and set it as `RVRS_TOKEN`. It takes the place of the stored credential.

`rvrs logout` revokes the credential at the API, removes the file and removes the runs your other installs made from this device. Runs made here, saved runs, cases and favorites stay. You can also revoke any install at app.rvrs.dev.

`RVRS_API=<url>` points rvrs at another API, such as one you host. A stored credential is only ever sent to the API it came from.

## Updates

`rvrs update` updates rvrs the way it was installed: the newest release for a binary from the installer, `git pull` and `cargo build` for a checkout. Homebrew and the `.deb` and `.rpm` packages are updated by their package managers. `rvrs update --check` only checks.

A release download is checked against the release's `SHA256SUMS` before it replaces anything, by the installer and by `rvrs update` alike.

An update keeps the [build features](build-features.md): it installs the build that leaves out what this one leaves out, so a build without the Console stays without it. `rvrs update --without console` leaves the Console out from now on, and `rvrs update --with console` takes it back.

Once a day rvrs checks on its own, and a result ends with one line when a newer version is out. `RVRS_UPDATE_CHECK=0` turns that off.

## Versions

`VERSION` holds major.minor, and the build adds the commits since `VERSION` last changed (`crates/rvrs/build.rs`). `rvrs --version` prints both, as in `rvrs 0.5.3 (4ea5d4e)`, and a build that leaves out build features names them after it: `rvrs 0.14.0 (4ea5d4e) without console`.
