# rvrs bgp

`rvrs bgp` takes a hostname, an address, a prefix or an AS number and runs the lookup the rvrs app runs. Nothing is probed. The route, its visibility, AS paths, history and the network come from [RIPEstat](https://stat.ripe.net/docs/data-api/) (RIPE RIS route collectors, a Routinator validator and the registries' statistics), and your looking glasses add what their own routers hold.

```text
$ rvrs bgp example.com

// bgp example.com  route collectors

Routed and authorized
192.0.2.0/24 is announced by AS64500 Example Net, seen by 318 of 320 collector peers. A ROA
authorizes this origin AS.
Seen by 318/320 peers · Paths 42 · First seen 2017

Route
  Address           192.0.2.10
  Announced prefix  192.0.2.0/24
  Example Net       AS64500
  RPKI              Valid
  Route object      RADB
  Registry          RIPE NCC

Reached through
  AS64501       Transit One   ████████████   68.6%
  AS64502       Transit Two   █████▌         31.4%

Your path
  AS64510 › AS64501 › AS64500
  Your traceroute entered AS64500 Example Net from AS64501 Transit One. 68.6% of collector
  peers see the route through the same network.
  Traceroute · 5m ago

Network
  About AS64500
  Example Net · 12 IPv4 and 3 IPv6 prefixes · 15 of 15 checked have a ROA

Recent changes
  No origin AS changes in 7 days
  1,204 BGP updates seen by collectors in that time.

Sources
  RIPE RIS and RPKI · observed 3h ago
```

```bash
rvrs bgp 192.0.2.0/24 --paths
rvrs bgp AS64500 --neighbors --prefixes
rvrs bgp example.com --history --days 90
rvrs bgp check 192.0.2.0/24 AS64500
rvrs bgp example.com --json | jq .interpretation
```

| Flag | Prints | Default |
| --- | --- | --- |
| (none) | Overview: the verdict, the route, who it is reached through, recent changes | |
| `--paths` | Every distinct AS path collectors see, and what your looking glasses hold | |
| `--prefixes` | The prefixes the network originates, with RPKI for the first 40 | |
| `--neighbors` | For an AS: the networks seen upstream and downstream of it | |
| `--history` | Who announced the prefix, its visibility and 7 days of BGP updates | |
| `--days <n>` | How far back History looks: 7, 30 or 90 | 7 |
| `--rpki` | The origin validation spelled out: the three checks and the covering ROAs | |
| `--sources` | Where each part came from and when it was observed | |
| `--all` | Everything above | |
| `--no-glasses` | Leaves your looking glasses out of this lookup | |
| `--no-lookups` | No lookup of the network this device is in, which marks its own paths | |

How to read it:

- A hostname is resolved to all its addresses and each is placed in its route. The first one that has a route is looked up in full, and the others are listed.
- RPKI belongs to a route: a prefix and one origin AS. The results are Origin authorized, Wrong origin ASN, Prefix too specific, No ROA found and Unable to check. No ROA found means no ROA covers the route, so RPKI says nothing about it.
- An AS path is what a collector peer or a looking glass sees from its own network. Your path lays the newest `rvrs traceroute` or `rvrs mtr` to the same target against those paths.
- No route observed means no collector reported the prefix. Collectors see only what their peers send, so the address may still be reachable.
- One data call that fails leaves its part empty and is listed as Unavailable under Sources. The lookup fails only when nothing answers.
- In a pipe or with `--classic` the lookup prints plain text that ends in one `;;` line per source with the time its data describes.
- The lookup exits 0 when it found a route or a network, 1 when no route was observed and 2 when it failed. `rvrs bgp check` exits 0 for Valid, 1 for Invalid or Not Found and 2 when the validator gave no answer.

## Looking glasses

A looking glass shows the routes its own routers hold. Add one and every `rvrs bgp` lookup of a route asks it for the prefix, beside the collectors. They are kept in `looking-glasses.json` in the config folder.

```bash
rvrs bgp glass add lg.example.net
rvrs bgp glass add lg.example.org --router "Amsterdam,Frankfurt"
rvrs bgp glass
rvrs bgp glass test lg.example.net 192.0.2.0/24
rvrs bgp glass remove lg.example.net
```

`glass add` tries each type in turn, or the one `--type` names (`alice-lg`, `lg-cgi`, `hyperglass`, `bird-lg-go`). A looking glass in front of up to four routers is added with all of them. With more, `glass add` lists them and takes the ones to ask with `--router`, since every router is one more request on every lookup. `glass test` asks one router for a prefix (1.1.1.0/24 unless you name one), prints each step, and prints the router's own output with `--raw`.

| Type | Read through | Example | Status |
| --- | --- | --- | --- |
| Alice-LG, an exchange's route servers | Its JSON API: accepted and filtered routes, and the origin AS's sessions with the route servers | lg.de-cix.net | Supported |
| lg.cgi, the classic Perl looking glass | The HTML form at `/`, `/lg.cgi` or `/cgi-bin/lg.cgi`, one request per router | lg.ip.tdc.net | Supported |
| hyperglass 1 and 2 | Its JSON API, text or structured routes | lg.nforce.com, lg.solnet.ch | Supported |
| bird-lg-go | Its pages around BIRD's `show route all` | lg.burble.com | Supported |
| bird-lg (the Python original), bgplg and custom forms | | | Not supported |
| A looking glass behind a CAPTCHA | | lg.twelve99.net | Not supported: rvrs does not work around a CAPTCHA |

Router output is read as Junos, Cisco or FRR, or BIRD. A looking glass that doesn't answer is listed as No answer and never fails the lookup.
