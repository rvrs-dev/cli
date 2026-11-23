# Build features

A build of rvrs is made of build features: the tools, the TCP probe, the path MTU check and the Console. The installer puts the full build in place, with every build feature. A build can leave some out, which makes the binary smaller for a server or for a router with little flash.

```bash
curl -fsSL https://rvrs.dev/install.sh | sh -s -- --without console
```

| Build feature | Gives |
| --- | --- |
| `traceroute` | `rvrs traceroute` |
| `ping` | `rvrs ping` |
| `mtr` | `rvrs mtr` |
| `dns` | `rvrs dig` |
| `bgp` | `rvrs bgp` |
| `tcp` | The TCP probe, `rvrs traceroute -T`. It works inside traceroute |
| `mtu` | The path MTU check, `rvrs traceroute --mtu`. It works inside traceroute |
| `console` | The Console, `rvrs` with no command |

`rvrs status` lists the build features a build has. `rvrs --version` ends with what it leaves out:

```
rvrs 0.14.0 (4ea5d4e) without console
```

A node will name its build features in its heartbeat, so these names never change ([the node protocol](../schema/node-protocol/README.md)). iperf3 joins as a build feature when the CLI runs it.

## A build without a build feature

It can't run the commands that need it, and it says how to add it:

```
$ rvrs bgp AS64500
rvrs: This build of rvrs leaves out BGP. rvrs update --with bgp adds it.
```

It shows every run, including runs of a tool it leaves out. Those are drawn from the run's report, the way a run another device made is. History, saved runs, cases, favorites, the account and sync work in every build. Without the Console, `rvrs` alone prints the help and `rvrs example.com` traces.

## Updates

`rvrs update` installs the build that leaves out what this one leaves out. A build feature that a later version adds comes with the update, unless you leave it out too. An rvrs installed before build features existed is the full build, and every update it takes is the full build.

```bash
rvrs update --without console
```

```bash
rvrs update --with console
```

The first leaves the Console out from now on, and the second takes it back. Both work while rvrs is up to date.

When a release has no build that leaves out the same build features, the update stops and rvrs stays as it is. It never puts a bigger build in place of a smaller one unasked, since a router may have no room for it.

Homebrew and the `.deb` and `.rpm` packages hold the full build. To leave build features out, use the installer.

## Prebuilt builds

Every release has two builds for each target: the full build, and the small build without the Console.

| Target | Archive of the full build | Archive of the small build |
| --- | --- | --- |
| macOS, Apple silicon | `rvrs-aarch64-apple-darwin.tar.gz` | `rvrs-aarch64-apple-darwin-without-console.tar.gz` |
| macOS, Intel | `rvrs-x86_64-apple-darwin.tar.gz` | `rvrs-x86_64-apple-darwin-without-console.tar.gz` |
| Linux, x86_64 | `rvrs-x86_64-unknown-linux-musl.tar.gz` | `rvrs-x86_64-unknown-linux-musl-without-console.tar.gz` |
| Linux, arm64 | `rvrs-aarch64-unknown-linux-musl.tar.gz` | `rvrs-aarch64-unknown-linux-musl-without-console.tar.gz` |
| Linux, ARMv7 | `rvrs-armv7-unknown-linux-musleabihf.tar.gz` | `rvrs-armv7-unknown-linux-musleabihf-without-console.tar.gz` |

An archive is named by what it leaves out, with the names sorted: a build without BGP and the Console would be `-without-bgp-console`. The installer and `rvrs update` ask for an archive by that name, and when a release doesn't have it they say which builds it has. ARMv7 is 32-bit ARM with hardware floating point, as on most routers and boards that run Linux on ARM.

## Another set of build features

From source, any set builds:

```bash
cargo build --release -p rvrs --no-default-features --features traceroute,ping,mtr,dns
```

In a checkout, `rvrs update --without bgp` builds this way, and every later `rvrs update` keeps the set.

## The small build

The small build is rvrs without the Console, built with the `small` profile in `Cargo.toml`, which optimizes for size: `opt-level = "z"` and LTO across every crate. A run waits on the network and never on the processor, so it measures the same as the full build.

| Target | Small build | Gzipped | Budget |
| --- | ---: | ---: | ---: |
| Linux, x86_64 | 3.6 MB | 1.8 MB | 4 MiB |
| Linux, arm64 | 3.0 MB | 1.7 MB | 3.5 MiB |
| Linux, ARMv7 | 2.8 MB | 1.5 MB | 3.5 MiB |

Measured on 2026-11-22, when the full build was 5.1 MB on arm64 and 5.0 MB on ARMv7. CI reports the size of every target on each push.

The budgets are in `scripts/small-build.budget`. On the router targets, 3.5 MiB is about 1.8 MB gzipped, which leaves room next to OpenWrt on a router with 16 MB of flash. x86_64 code is larger and runs on servers and virtual machines, so it gets 4 MiB. CI builds the small build for x86_64, arm64 and ARMv7 Linux on every push, runs it, and fails when it passes its budget or when a crate comes into it that `scripts/small-build.deps` doesn't list. A dependency added to `rvrs-core` shows up there with what it costs.

MIPS routers come later. Rust ships no standard library for MIPS, a tier 3 target, so a MIPS build needs a nightly compiler. ARMv7 came first for that reason.

The build features only decide what is in the binary. A node will also keep its runs in memory instead of a history file, which comes with `rvrs node`.
