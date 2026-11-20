# rvrs traceroute

`rvrs traceroute <target>` traces the path to a target and groups the hops by network. Called as `traceroute`, or in a pipe, it prints what traceroute prints.

rvrs follows the Linux traceroute.

| Flag | Meaning | Default |
| --- | --- | --- |
| `-m`, `--max-hops <n>` | Last hop to probe, 1 to 255 | 30 |
| `-q`, `--queries <n>` | Probes per hop, 1 to 10 | 3 |
| `-w`, `--wait <s>` | Longest wait for a reply, in seconds | 5 |
| `-f`, `--first <n>` | First hop to probe | 1 |
| `-z`, `--sendwait <t>` | Pause between probes: seconds up to 10, milliseconds above | 5 ms |
| `-n` | Addresses only, no reverse DNS | |
| `-A`, `--as-path-lookups` | Each hop's AS number in the classic output | |
| `-4`, `-6` | Resolve the target to IPv4 or IPv6 | the system's order |
| `-I`, `--icmp` | ICMP echo probes, which rvrs always sends | |
| `<packet length>` | Total packet size, after the target | 60 |

Where it differs from Linux traceroute:

- Probes are ICMP echo only. Linux defaults to UDP. `-T`, `-U`, `-p`, `-s` and `-i` exit with an error that says so.
- All probes go out in parallel, so `-N` is accepted and does nothing.
- Once the destination answers, a probe still out waits ten times the slowest reply, at least 0.5 s and at most `-w`.
- A router that rate-limits ICMP replies may answer only the first probe of its hop. `-z 0.2` spaces the probes out.
- The classic output has traceroute's format (`host (address)  1.234 ms`, `* * *`, `!H`), but not byte for byte. Scripts should use `--json`.

Exit codes: 0 when the run was made, whether or not the target answered. 1 when it couldn't be made, such as a name that didn't resolve. 2 for invalid arguments. 130 after a second Ctrl-C.
