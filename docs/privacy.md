# What leaves this device

These are the requests rvrs makes.

- Probes and ping packets, to the target and the routers on the way.
- Reverse DNS for each hop, through the system resolver. `-n` turns it off.
- For each public hop address, a lookup of its network and location at stat.ripe.net (RIPEstat), cached for a week. Private addresses are never sent.
- One request to stat.ripe.net for this device's public address, shown in history as PUBLIC. The classic output skips it.
- The questions of `rvrs dig`: to this device's resolver, to 1.1.1.1, 8.8.8.8 and 9.9.9.9, and to the root, TLD and authoritative servers of the name. One question in dig's output goes to the one server asked.
- The requests of `rvrs bgp`, to stat.ripe.net: the address, prefix or AS number that was looked up (for a hostname, up to eight of the addresses it resolves to), the origin AS with up to 40 of its prefixes, and the AS numbers in the paths, for their holders' names. The prefix and the origin AS number also go to each of your looking glasses. `--no-glasses` leaves those out.
- Once a day, a look for a newer rvrs: a request to api.github.com for the newest release, or `git fetch` in the checkout rvrs was built from. It sends nothing about you or your runs. `RVRS_UPDATE_CHECK=0` turns it off.

`--no-lookups` turns off everything but the probes, the DNS questions and the requests a BGP lookup is made of. The CLI has no account and sends no usage data.
