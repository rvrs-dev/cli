# rvrs ping

`rvrs ping <target>` measures latency and loss packet by packet, until Ctrl-C or `-c` packets.

rvrs follows the Linux ping (iputils).

```
$ rvrs ping -i 0.2 github.com
// ping github.com  140.82.121.4 · IPv4 · this device

    1    21.9 ms  ttl 58
    2    23.6 ms  ttl 58
    3    21.4 ms  ttl 58

  ● p50 22.0  ● p95 26.2  ● p99 26.2  ● loss 0.0%
  ▇▇▇▇█▇▇▇▇▆▇▇█▇▇▇

16 pkt · 0.0% loss · 22.7 ms avg
min 20.2 · max 26.2 · jitter 2.0 ms
Stable. No loss over 16 packets. p95 26.2 ms, jitter 2.0 ms.
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `-c <n>` | Stop after n packets | until Ctrl-C |
| `-i <s>` | Seconds between packets, from 0.01 | 1 |
| `-W <s>` | Seconds before a packet counts as lost | 2 |
| `-w <s>` | Stop after this many seconds | |
| `-s <bytes>` | Payload size | 56 |
| `-t <ttl>` | TTL of the packets sent | 64 |
| `-n` | Addresses only, no reverse DNS | |
| `-q` | Only the first line and the statistics | |
| `-4`, `-6` | Resolve the target to IPv4 or IPv6 | the system's order |

In a pipe each line is ping's own (`64 bytes from one.one.one.one (1.1.1.1): icmp_seq=1 ttl=58 time=11.8 ms`), followed by the statistics with `rtt min/avg/max/mdev`.

Where it differs from Linux ping:

- A packet counts as lost after `-W` seconds, 2 by default. Linux ping waits up to 10 seconds.
- The verdict (Stable, Jittery, Occasional loss, Packet loss) appears after 10 packets and uses the app's thresholds. The report keeps it under `interpretation`.
- `-f`, `-I` and the rarer flags exit with an error.
- macOS ping spells some flags differently: there `-t` is a timeout and `-m` the TTL.

Exit codes are ping's: 0 when the target answered, 1 when it didn't, 2 when the run couldn't be made. `rvrs ping -c 1 -W 1 host && echo up` works.
