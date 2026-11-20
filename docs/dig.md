# rvrs dig

`rvrs dig <name>` runs the lookup the rvrs app runs. It asks this device's resolver, Cloudflare, Google, Quad9 and the zone's authoritative servers for nine record types, walks the delegation from the root, and compares the answers.

```
$ rvrs dig example.com

// dig example.com  this device

All sources agree
Resolves to 2 addresses, zone served by example-dns.net. DNSSEC records are present from the root down.
A / AAAA 1 / 1 · TTL 5m · DNSSEC Signed

Key records
  A      192.0.2.10                                        5m
  AAAA   2001:db8::10                                      5m
  MX     10 mail.example.com                               1h
  NS     ns1.example-dns.net                               1d

Compare
  Source                Server           A                 TTL      Time  Status
  Router                192.168.1.1      192.0.2.10        2m       4 ms  Match
  Cloudflare            1.1.1.1          192.0.2.10        5m      12 ms  Match
  Google                8.8.8.8          192.0.2.10        5m      27 ms  Match
  Quad9                 9.9.9.9          192.0.2.10        4m      27 ms  Match
  ns1.example-dns.net   198.51.100.53    192.0.2.10        5m      21 ms  Match

Delegation path
  Root           .            a.root-servers.net · 27 ms · delegates com to 13 servers · DS · RRSIG
  TLD            com          a.gtld-servers.net · 23 ms · delegates example.com to 2 servers · DS · RRSIG
  Authoritative  example.com  ns1.example-dns.net · 21 ms · answers for the name · RRSIG
  com publishes a DS for example.com, the zone serves its DNSKEY records and its answers are signed.
  A validating resolver can check every step.

Recent changes
  Same addresses, names and nameservers as the lookup 2h ago.
```

When sources answer differently, a Differences section names each difference with its usual reason, such as a CDN that answers by where the resolver asks from. Recent changes compares the lookup with the one before it in history. `--resolver <address>` adds a resolver of your own to the sources, and can be given more than once.

rvrs checks that DNSSEC records are present along the delegation. It doesn't validate signatures.

With a server, a record type or a `+option`, rvrs asks one question and prints what dig prints. A pipe, `--classic` and a link named `dig` do the same for a name alone.

```bash
rvrs dig example.com MX +short
rvrs dig @1.1.1.1 example.com AAAA
rvrs dig -x 1.1.1.1 +short
rvrs dig example.com +trace
rvrs dig example.com --json | jq .interpretation
```

| Flag | Meaning | Default |
| --- | --- | --- |
| `@<server>` | The server to ask, by address or name | This device's resolver |
| `<type>`, `-t <type>` | A, AAAA, CNAME, MX, TXT, NS, SOA, HTTPS, CAA, DS, DNSKEY, PTR, SRV, ANY or `TYPE99` | A |
| `-x <address>` | The name for an address, through its PTR record | |
| `-q <name>` | The name, where it could be read as a type | |
| `-p <port>` | The server's port | 53 |
| `-4`, `-6` | Ask over IPv4 or IPv6 | |
| `+short` | The answer's values and nothing else | |
| `+trace` | From the root down, one server per zone | |
| `+tcp`, `+vc` | Ask over TCP from the start | UDP, then TCP when the answer is truncated |
| `+dnssec`, `+do` | Ask for DNSSEC records with the answer | |
| `+norecurse` | Ask the server for what it holds itself | |
| `+noedns` | No OPT record in the question | |
| `+noall`, `+answer`, `+authority`, `+additional` | Which sections print | All |
| `+question`, `+comments`, `+stats`, `+cmd` | Which lines print around them, each with a `+no` form | All |
| `+time=<s>`, `+tries=<n>`, `+retry=<n>` | Seconds per try and the number of tries | 5 and 3 |
| `--resolver <address>` | A resolver of your own, asked beside the public ones in the lookup | |
| `--no-lookups` | No lookup of this device's public address | |

Differences from the original:

- The first line names rvrs and its version where dig names itself. Every other line is dig's, as dig 9.10 prints it.
- Class IN only. `-b`, `-f`, `-k`, `-y`, `+multiline`, `+cookie`, `+subnet` and any option not in the table return an error that names it.
- `+trace` asks this device's resolver for the root servers, then one server per zone. It adds DNSSEC records only with `+dnssec`.
- One question is printed and not kept. The lookup is a run: it goes to history, `rvrs last` shows it again, and `--case` saves it.
- A question exits with dig's codes: 0 for any answer, NXDOMAIN included, and 9 when no server answered. The lookup exits 0 when a source answered and 1 when none did.
- `--json` prints the lookup as a report and takes a name alone.
