# History and cases

rvrs keeps every run on this device: 500 in history, newest first, and saved runs for as long as the person wants them. A case is a named set of saved runs collected to look into one question.

## Finding earlier runs

```bash
rvrs history                      # every run
rvrs history example.com          # the runs to one target
rvrs history mtr example.com      # one tool's runs to it
```

```text
ID      TOOL        TARGET       SUMMARY                          SOURCE        PUBLIC       AGE
2u8gqj  mtr         example.com  20 pkt · 0.0% loss · 10.8 ms avg  198.51.100.7  203.0.113.9  2 h ago
fvx2vn  ping        example.com  20 pkt · 0.0% loss · 11.7 ms avg  198.51.100.7  203.0.113.9  3 d ago
```

SOURCE and PUBLIC are the origin: the address probes left from and the address the internet saw. PUBLIC is `–` when rvrs didn't ask for it on that run, which says nothing about the network. Then go by SOURCE. A word after `history` is a tool (`traceroute`, `ping`, `mtr`, `dig`, `bgp`) or part of a target, and every word has to match. Add `--json` for the same runs as reports. `rvrs saved` and `rvrs case <name>` list in the same columns. For the exact time and the figures, `rvrs show <id> --json`.

If history has nothing for the target, say so. Then the fresh run is the first data point, and you offer to repeat it later.

## Picking a baseline

A baseline is an earlier run that differs from the new one in time only.

1. Same target, as typed. `example.com` and `www.example.com` can be different hosts.
2. Same tool. A ping's loss and an MTR's loss at the destination are comparable. A traceroute's three probes are not.
3. Same origin. If PUBLIC differs between two runs that both have one, the device was on another network (home, office, a tethered phone, a VPN) and the path began somewhere else. Say that before any comparison.
4. Same IP family. `family` in the report says which was used.
5. A similar count. A run of 5 packets is too short to set against one of 100.

Prefer several earlier runs to one. If three runs last week sit at 11 to 12 ms and today's sits at 48 ms, that is a change. One earlier run at 11 ms is a hint.

Then make the fresh run to match the baseline. Its report gives the count (`sent` in `measured`) and the `family`. The interval and other flags are not recorded, so use this skill's defaults.

## Saying when

History holds the runs that were made, and nothing about the time between them. Give the last clean run and the first affected run you can see, with their times, and say the change happened between them. If the gap is three days, the answer is "between Tuesday 09:10 and Friday 14:02".

BGP history is a different record: what route collectors saw, in 8-hour snapshots, whether or not this device ran anything. `rvrs bgp <target> --history --days 90` reads it. A route change there near the time a path changed in the person's runs is worth saying, as two observations that line up. It is not proof that one caused the other.

## Keeping evidence in a case

When the person is building a ticket, watching a problem or making a change:

```bash
rvrs case                                   # is there a case for this already?
rvrs mtr -c 30 --json --case isp-evening example.com    # run and save in one step
rvrs save 2u8gqj --case isp-evening         # add an earlier run from history
rvrs case isp-evening --json                # every run of the case as reports
```

`--case` and `rvrs save --case` make the case when it is new. Name it after the question in the person's words, lower case with hyphens. Save the runs that carry the argument: the clean baseline, the first affected run, one run per origin or family that narrows it. Ten near-identical runs make a case harder to read.

A case holds runs. The reasoning is yours: keep it in your answer, with the run ids, so the person can paste both.

## Before and after a change

When the person is about to change a route, a resolver, a provider or a tunnel, offer this before they do it:

```bash
rvrs mtr -c 30 --json --case before-after-<change> <target>
rvrs dig <name> --json --case before-after-<change>
rvrs bgp <prefix> --json --case before-after-<change>
```

Run the same three after the change and compare by id. A baseline can't be collected once the change is made.

## Not yours to delete

`rvrs history clear`, `rvrs saved remove <id>`, `rvrs case delete <name>` and `rvrs case remove <name> <id>` remove what the person kept. Run them only when asked for exactly that, and say what will go first.
