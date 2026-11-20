# Playbooks

Each one starts from what the person said and ends when the question is answered. Stop at the first step that answers it.

## "It's slow"

1. `rvrs history <target>`: is there a run to compare against?
2. `rvrs ping -c 20 -i 0.2 --json <target>`: loss, p50, p95, jitter to the target.
   - No loss, p95 close to p50, figures like the baseline: the path from this device is fine. Say so and look at DNS (step 5) or above the network.
   - Loss, or a p95 several times the p50: go on.
3. `rvrs mtr -c 20 --json <target>`: find the first hop from which the loss or the higher latency holds to the end. Name the hop and its network.
4. Narrow it with one more run, picked by where it starts:
   - At hop 1 or 2: the local network or the access line. Ping the gateway (`hops[0].address`) with `-c 20`. Loss there is Wi-Fi or LAN.
   - Inside the provider's network: run the same MTR to a second target that leaves through the same provider. Both bad is the access network. One bad is that path.
   - Near the target: the far end or its provider. Try `-6` or `-4`, since the other family often takes another path.
5. `rvrs dig <name> --json`: a slow or failing resolver makes every first connection slow while ping looks fine. Check `sources[].milliseconds` and `status` for the device's resolver.

## "I can't reach it"

1. `rvrs dig <name> --json`. `failed` or no addresses: it is DNS, go to that playbook.
2. `rvrs ping -c 5 --json <target>`.
   - `reached`: the host answers. The problem is the port or the service, which rvrs does not probe. Say that.
   - `exhausted`: no reply to ICMP. This alone proves nothing.
3. `rvrs traceroute --json <target>`: how far do the probes get? The last hop that answers and its network is the finding. A path that ends inside the target's own network with the target silent usually means the target drops ICMP.
4. `rvrs bgp <address> --json`: is the prefix announced, and by how many collector peers? `exhausted` or low visibility is a routing problem, and `--history --days 7` shows when it began.
5. Compare with history. A target that never answered ping before is not newly down.

## "It drops sometimes"

A short run will miss it. Say so first.

1. History first: are there earlier runs with loss, and at what times of day?
2. `rvrs mtr -c 60 --json <target>`: one minute. Read the destination's loss and jitter.
3. If it is clean, the problem did not happen in that minute. Offer a longer run (`-c 300`, five minutes, in the background if you can) or a repeat when the person next notices it. Save runs into a case so the pattern builds up: `--case <name>`.
4. When a run does catch it, read where the loss begins and whether jitter rises with it. Loss with rising latency at the same hop points at congestion. Loss with flat latency points at a faulty link or radio. Both are inferences: word them that way.

## "DNS is broken" or "it resolves to the wrong address"

1. `rvrs dig <name> --json`, then `jq '.sources[] | {name, kind, status, milliseconds, addresses, error}'`.
2. Read it in this order:
   - A source with `error`: which one? Only the device's resolver failing is a local resolver problem. The authoritative servers failing is the zone's problem.
   - `differs` on the device's resolver only: compare its `addresses` with the authoritative ones. A subset of them is a forwarder handing out part of the record set, which is normal. Addresses the zone doesn't have are a local override, a filter such as Pi-hole, split-horizon DNS or a stale cache.
   - `differs` among public resolvers with every answer inside one provider's range: usually a CDN answering by location. Normal.
   - `summary` ends `unsigned` where the person expects DNSSEC: check DS at the parent with `rvrs dig <name> DS +short`.
3. One record at one server settles a detail: `rvrs dig @<server> <name> <type> +short`. `rvrs dig <name> +trace` walks the delegation from the root.
4. If the name was looked up before, compare with `rvrs show <id> --json`: changed addresses or nameservers since then are the finding.

## "Did the route change?" and other BGP questions

1. `rvrs bgp <host|address|prefix> --json`: origin AS, origin validation, how many collector peers see it.
2. `rvrs bgp <target> --history --days 30` (or 7, or 90): `Events` lists each change of origin AS and each drop or recovery of visibility, or `No changes`.
3. `rvrs bgp check <prefix> <ASN>` before the person announces a prefix or when a route is rejected: it names which of the three checks fails and the covering ROAs.
4. `rvrs bgp AS64500 --neighbors --prefixes` for a network: what it originates and which networks are seen next to it.
5. To tie routing to a symptom, run `rvrs traceroute --json <host>` and compare the networks in `hops[].asn` with the AS paths. Collectors see paths from their peers. This device's path is its own.

Words: say origin AS where people say "owner", and Valid, Invalid or Not Found for RPKI. Say "seen upstream of" where people say "provider of", because collectors show where a network sits in a path and nothing about contracts.

## IPv4 against IPv6

1. `rvrs status`: does this device have a public IPv6 address? If not, stop and say that.
2. The same run twice, changing only the flag:

   ```bash
   rvrs mtr -c 20 -4 --json <target>
   rvrs mtr -c 20 -6 --json <target>
   ```
3. Compare destination loss, p50, p95 and the networks on each path. The two families often leave through different networks, so a difference is common and worth naming when one is clearly worse.

## Writing the ticket

When the person needs to hand this to a provider, give them one block they can paste:

- What they see, in one sentence, and since when (last clean run, first affected run).
- Origin: public address and network. Target: name and address. IP family.
- The measurement: tool, count, loss, p50 and p95, and the hop and network from which the loss carries to the target.
- What was ruled out: local network, DNS, the other family, a second target.
- The run ids, saved in a case, and the MTR's hop table from `rvrs show <id>`.

Provider support reads MTR. Include a run toward the target and, if they can get one, a run in the other direction. Ask what to redact before it leaves the conversation.
