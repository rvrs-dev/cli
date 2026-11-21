# Setup: installing, updating and checking rvrs

## Is it there?

```bash
command -v rvrs && rvrs --version
```

## Installing

If `rvrs` is missing, tell the person in one sentence and ask once:

> rvrs isn't installed. It's one open source binary that goes into `~/.local/bin`, with no root and no account. Install it?

If they already asked you to set rvrs up, or you are running with standing permission to install tools, that is the yes. Then:

```bash
# macOS or Linux with Homebrew
brew install rvrs-dev/tap/rvrs

# anywhere else on macOS or Linux, x86_64 or arm64
curl -fsSL https://rvrs.dev/install.sh | sh
```

Run without a terminal, as you do, the installer asks nothing, links nothing over the system's tools and checks the download against the release's SHA256SUMS. It prints where it put the binary. If `~/.local/bin` is not on `PATH`, call rvrs by that full path for this session and tell the person the line to add to their shell profile. Do not edit their profile yourself.

Do not pass `--link`. That makes rvrs answer to `traceroute`, `ping`, `mtr` and `dig`, which is the person's choice to make at their own terminal.

A `.deb` and an `.rpm` are on https://github.com/rvrs-dev/cli/releases for machines that want a package. Those need root, so hand the command to the person.

If the install is refused or fails, say so and offer the choice: stop, or carry on with the system's tools for this one question, with the note that nothing will be kept in history.

## Checking

```bash
rvrs status
```

It prints one line per check. Read these:

- **ICMP over IPv4 / IPv6**: whether this user may send probes. On Linux a refusal names `net.ipv4.ping_group_range`. That is a system setting, so give the person the line and do not change it yourself.
- **IPv6**: whether this device has a public IPv6 address. Without one, `-6` runs fail with exit code 2 and that is not a finding about the target.
- **History / Saved**: how many runs and cases there are to compare against.
- **Version**: this skill needs 0.10 or newer. Older versions have no `rvrs login`. Older versions have no path MTU check (`traceroute --mtu`), before 0.8 no TCP probes (`traceroute -T`), and before 0.7 no `id` in a report and no filter on `rvrs history`.
- **Account / Nodes**: whether runs can start anywhere but this device. See [nodes and accounts](nodes.md).

## Updating

`rvrs traceroute -T` answering that it doesn't send that probe type, or a report without an `id` field, means an older rvrs. Ask, then:

```bash
rvrs update
```

It updates rvrs the way it was installed and names the package manager's command when a package manager owns the binary. `rvrs update --check` only looks.

## What leaves this device

Tell the person when they ask, or before a run toward something private:

- Probes go to the target and the routers on the way.
- Each public hop address is looked up at stat.ripe.net for its network. Private addresses are never sent. `--no-lookups` turns this off, and hops then carry no `asn`.
- `rvrs dig <name>` asks this device's resolver, 1.1.1.1, 8.8.8.8, 9.9.9.9 and the name's own servers.
- `rvrs bgp` sends the address, prefix or AS number to stat.ripe.net, and to the person's own looking glasses when they added any.
- rvrs needs no account. Once a day it looks for a newer release (`RVRS_UPDATE_CHECK=0` turns that off), and every 12 hours it counts runs per tool to api.rvrs.dev (`RVRS_PULSE=0` turns that off). No target or result is sent.

## Environment

| Variable | Does |
| --- | --- |
| `RVRS_HISTORY=0` | Keeps this run out of history. Use it only for a throwaway check the person does not want kept. |
| `RVRS_HOME=<dir>` | Puts history, saved runs, cases and settings in one folder. Useful for a test that must not touch the person's history. |
| `NO_COLOR=1` | No color codes. A pipe gets none anyway. |
