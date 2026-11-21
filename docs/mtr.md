# rvrs mtr

`rvrs mtr <target>` measures loss and latency at every hop, round after round, until Ctrl-C or `-c` rounds.

rvrs follows mtr.

```
$ rvrs mtr example.com
// mtr example.com  93.184.216.34 · IPv4 · this device

                                             Loss  Snt   Last    Avg   Best  Worst Jitter
      AS64500 Example Net · Frankfurt, DE
  1 │ 192.168.1.1                           16.7%   12    0.5    0.6    0.4    0.9    0.2  ▂▂▃▂▂▃▂▃××▂▂
  2 │ lo0.edge1.fra.example.net 198.51.100.1 0.0%   16    8.6   10.1    6.7   16.5    3.0  ▄▃▄▅▃▄▄▄▃▃▃▄▃▃▄▃
      AS64501 Example Edge
  3 ● 93.184.216.34                          0.0%   16   23.3   21.4   19.8   24.9    1.7  ▆▆▆▆▇▆▆▇▆▆▆▆▆▆▆▇

16 pkt · 0.0% loss · 21.4 ms avg · 3 hops · 2 networks · 16 s
min 19.8 · max 24.9 · jitter 1.7 · p50 20.7 · p95 24.9 ms
Stable. No loss over 16 packets. p95 24.9 ms, jitter 1.7 ms.
Hop 1 answers fewer probes than the hops after it. Routers often rate-limit these replies, so the
path itself is likely fine.
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `-c`, `--report-cycles <n>` | Stop after n rounds | until Ctrl-C, or 10 for a report |
| `-r`, `--report` | Print mtr's report when the rounds are done | |
| `-w`, `--report-wide` | A report whose host column fits the longest name | |
| `-i`, `--interval <s>` | Seconds between rounds, from 0.1 | 1 |
| `-m`, `--max-ttl <n>` | Last hop to probe, 1 to 255 | 30 |
| `-f`, `--first-ttl <n>` | First hop to probe | 1 |
| `-s`, `--psize <bytes>` | Packet size, headers included | 64 |
| `-Z`, `--timeout <s>` | Seconds a probe may take before it counts as a loss | 2 |
| `-U`, `--max-unknown <n>` | Silent hops probed past the farthest that answered | 5 |
| `-n`, `--no-dns` | Addresses only, no reverse DNS | |
| `-b`, `--show-ips` | Names and addresses in the report | |
| `-z`, `--aslookup` | Each hop's AS number in the report | |
| `-4`, `-6` | Resolve the target to IPv4 or IPv6 | the system's order |
| `-j` | Prints the run as a report, as `--json` does | |

On a terminal the table updates in place and stays in the scrollback when the run ends. In a pipe, with `-r`, or called as `mtr`, rvrs prints mtr's report: a `Start:` line, the `HOST:` line with `Loss%   Snt   Last   Avg  Best  Wrst StDev`, and a line per hop.

What rvrs adds to what was measured:

- **Loss is red only when it reaches the end of the path.** A middle hop that drops replies while later hops are clean is likely rate-limiting them. rvrs shows that loss in gray and names the hop under the table. It needs two losses and 10% at a hop before it says so.
- **A verdict on the destination** after 10 packets, with the thresholds the app uses.
- **A strip of each hop's newest rounds** on a wide terminal: the height is the latency, a cross is a loss.

Differences from the original:

- MTR sends ICMP echo probes only. `-u`, `-T`, `-a` and `-I` return an error. For a TCP port, `rvrs traceroute -T -p <port>`.
- `rvrs mtr` never opens a full-screen view, so `-t` and `-g` return an error. `rvrs` on its own opens the Console, and `mtr example.com` typed there runs an MTR.
- `-j` and `--json` print the rvrs report. mtr's own JSON, CSV and XML are not written.
- A long run keeps its newest 1200 rounds per hop.
- The first round probes every hop up to `-m` to find the path at once. After that rvrs stops `-U` hops past the farthest one that answered until the destination is known.
