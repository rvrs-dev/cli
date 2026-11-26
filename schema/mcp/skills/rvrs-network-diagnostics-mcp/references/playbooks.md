# Playbooks

Each one starts from what the person said and ends when the question is answered. Stop at the first step that answers it.

## "What is getting worse?"

1. `summarize_runs` with `window` `24h` and `by` `install`, then the same with `by` `target`. A group with `worthALook`, a `median` well above `medianBefore`, or `withLoss` that grew is where to look.
2. For the origins that stand out, `list_nodes` shows their tags. When they share one, such as `district=sodermalm`, list every node with it and count how many show the change. A trend on 6 of 8 nodes in one district is a finding about the district. A trend on one node is a finding about that node.
3. `find_runs` with those `tags` and a `tool` that has a path (`traceroute` or `mtr`), `since` `24h`. Read the newest with `get_run`.
4. Find where it starts: the first hop from which the latency, jitter or loss holds to the end. Name the hop and its network.
5. `find_runs` with `like` on that run, and `compare_runs` with the run from about a day before as the baseline. `paths` says whether the path changed. If it didn't, the hop that got worse is the finding.

## "Was it like this yesterday?"

1. The newest run to the target: `find_runs` with `target`, `limit` 1.
2. `find_runs` with `like` on it, `since` `7d`: the comparable runs, newest first.
3. `compare_runs` with the one closest to a day before as the baseline and the newest after it. Say what `differs` names first. A different origin, address or IP family makes it no before and after.
4. Answer with the figures that moved, both runs linked. With no comparable run, describe now and say that there is no earlier run to hold it against.

## "It's slow"

1. `find_runs` with `target`, `since` `7d`: is there a run at all, and from where?
2. `get_run` on the newest ping or MTR: loss, p50, p95, jitter to the target.
   - No loss, p95 close to p50, figures like the runs before: this ICMP sample shows no trouble on the path from that origin. Say so, say that TCP and the application were not tested, and look at DNS or above the network.
   - Loss, or a p95 several times the p50: go on.
3. In an MTR or traceroute, find the first hop from which the loss or the higher latency holds to the end. Name the hop and its network.
4. Narrow it by where it starts:
   - At hop 1 or 2: the local network or the access line. Other origins to the same target will be clean.
   - Inside the provider's network: runs from that origin to a second target behind the same provider. Both bad points at the access network. One bad points at that path.
   - Near the target: the far end or its provider. Runs over the other IP family often take another path.
5. A DNS run of the name: a slow or failing resolver makes every first connection slow while ping looks fine.

## "Is it them or us?"

1. `summarize_runs` with `by` `install` and the `target`: the same target as each origin measured it.
2. One origin bad and the rest clean: the trouble is near that origin, on its line or in its network. Every origin bad from where their paths join: the trouble is from that point on, often near the target.
3. `compare_runs` on the newest comparable run from each origin, and read `paths` for where they part.
4. When every run comes from one origin, say that one origin can't tell, and name the run that would: the same tool and target from a node on another network.

## "It drops sometimes"

A short run will miss it. Say so first.

1. `summarize_runs` with `tool`, `target` and `window` `7d`: `latency.points` shows when loss and jitter come and go.
2. When a schedule measures the target, `find_runs` with `schedule` lists its runs, so the times of day the trouble shows up are in the history.
3. Read the runs that caught it. Loss with rising latency at the same hop points at congestion. Loss with flat latency points at a faulty link or radio. Both are inferences: word them that way.
4. When nothing caught it, say so, and name what would: a schedule on a node, which repeats the run at an interval. Schedules are made at app.rvrs.dev or with `rvrs schedule`.

## A schedule

1. `find_runs` with `trigger` `schedule` to see which schedules have runs, then with `schedule` set to one.
2. `summarize_runs` with the schedule's `tool` and `target` and `window` `24h` or `7d` for its figures over time, and with `by` `install` for each node it runs on.
3. A node whose runs stop is offline or stopped: `list_nodes` says which. A run of a schedule that ended `stopped` was stopped by a change to the schedule.

## Before and after a change

1. The runs before: `find_runs` with `target` and `since`, from the same origin.
2. The runs after the change, from the same origin, over the same IP family.
3. `compare_runs` with a run before as the baseline and the runs after it. Say what `differs` names first, then the figures that moved.

## DNS

1. `get_run` on the newest DNS run of the name.
2. Read `sources` in this order:
   - A source with `error`: which one? Only the device's resolver failing is a local resolver problem. The authoritative servers failing is the zone's problem.
   - `differs` on the device's resolver only: compare its `addresses` with the authoritative ones. A subset of them is a forwarder handing out part of the record set, which is normal. Addresses the zone doesn't have are a local override, a filter, split-horizon DNS or a stale cache.
   - `differs` among public resolvers with every answer inside one provider's range: usually a CDN answering by location. Normal.
3. Earlier DNS runs of the name with `find_runs`: changed addresses or nameservers since then are the finding.

## Writing the ticket

When the person needs to hand this to a provider, give them one block they can paste:

- What they see, in one sentence, and since when: the last clean run and the first affected one, each linked.
- Origin: the node or device, its network, and its public address if the person lets you read it. Target: name and address. IP family.
- The measurement: tool, count, loss, p50 and p95, and the hop and network from which the loss or latency carries to the target.
- What was ruled out: the local network, DNS, the other family, a second target, another origin.
- The links to the runs. A provider reads an MTR's hop table, which the run's page shows.

Ask what to redact before it leaves the conversation.
