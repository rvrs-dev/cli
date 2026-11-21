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
| `-I`, `--icmp` | ICMP echo probes | on |
| `-T`, `--tcp` | TCP probes to a port | |
| `-p`, `--port <n>` | The port for `-T` | 80 |
| `<packet length>` | Total packet size, after the target | 60 |

Where it differs from Linux traceroute:

- Probes are ICMP echo, or TCP with `-T`. Linux defaults to UDP. `-U`, `-s` and `-i` exit with an error that says so.
- All probes go out in parallel, so `-N` is accepted and does nothing.
- Once the destination answers, a probe still out waits ten times the slowest reply, at least 0.5 s and at most `-w`.
- A router that rate-limits ICMP replies may answer only the first probe of its hop. `-z 0.2` spaces the probes out.
- The classic output has traceroute's format (`host (address)  1.234 ms`, `* * *`, `!H`), but not byte for byte. Scripts should use `--json`.

Exit codes: 0 when the run was made, whether or not the target answered. 1 when it couldn't be made, such as a name that didn't resolve. 2 for invalid arguments. 130 after a second Ctrl-C.

## A port: `-T -p`

```bash
rvrs traceroute -T -p 443 example.com
```

It answers two questions: what does the port do, and where does a SYN to it stop.

First rvrs connects to the port once. A SYN-ACK is an open port, a reset is a closed one, and no answer is a filtered one. Then it traces with TCP SYNs.

| The port is | The trace | The result says |
| --- | --- | --- |
| open | runs to the destination | `6 hops · 10.7 ms · port 443 open`, with the connect time and the MSS of the handshake |
| closed | runs to the destination | `port 81 closed`: the host answers and nothing listens |
| filtered | stops five silent hops past the last router that answered | `Port 81 filtered · no reply after 5 hops`, and which router answered last |

`--json` adds `probe` (`protocol`, `port`, `state`) to the report, and `connect` (ms) and `mss` (bytes) to `measured` for an open port. The MSS is what the two ends agreed on. Add 40 bytes for IPv4 or 60 for IPv6 to read it as a packet size, and a value under 1460 means something on the path, or the server, announced less than a 1500-byte MTU.

It needs no root. At most six SYNs are on the wire at once, and every connection that opens is reset at once. Where it differs from `traceroute -T`: rvrs completes the handshake before it resets, since an unprivileged process can't send a bare SYN, so the destination sees short connections.

The iOS app has no TCP probes.
