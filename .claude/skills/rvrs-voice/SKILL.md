---
name: rvrs-voice
description: The rvrs tone of voice and copy rules. Load before writing or reviewing any text a person reads in rvrs: iOS app strings (labels, buttons, titles, results, interpretations, errors, empty states, footers, accessibility labels), the admin app, API error messages and OpenAPI descriptions, README and docs, release notes, App Store and TestFlight text, and commit-free marketing copy. Triggers on "copy", "wording", "microcopy", "error message", "empty state", "tone", "voice", "rename this", "what should this say".
---

# rvrs voice

rvrs is a networking toolkit for developers, network engineers, sysadmins and operators: traceroute, MTR, ping, iperf3 and DNS today, BGP next, nodes later. This file is its identity in words. It applies to every surface: the iOS app, the admin app, the API, the docs.

## The rule

**Say what happened. Show the evidence. Name the uncertainty. Offer the next action, if there is one.**

rvrs sounds like a very good network engineer sitting next to you: calm, compact, exact, lightly human. It reports what it measured, separates that from what it infers, and stops.

- Direct, not terse.
- Technical, not performative.
- Friendly, not chirpy.
- Confident about measurements, careful about causes.
- Specific before persuasive. Useful before impressive.

Prefer a number to an adjective, the real term to a category label, and a concrete action to reassurance.

## Language

- **American English**: color, organization, analyze, canceled, behavior, license (noun and verb).
- **Apple's Human Interface Guidelines win** where this file and the HIG disagree.
- **Title case** for buttons, menu items, navigation and sheet titles, tab names, toolbar items and alert titles, as the HIG does: "Run Again", "Copy Address", "Add Server", "Join the Waitlist". Articles, short prepositions and conjunctions stay lower case unless first ("Copy as Zone Line", "Report a Bug").
- **Sentence case** for section headers, row labels, footers, interpretations, status lines, error bodies, placeholders and everything that reads as a sentence: "Public address", "My servers", "Waiting for the first reply".
- **rvrs** is always lower case, also at the start of a sentence. The mark is "//".
- Contractions are fine: couldn't, isn't, doesn't, you're.

## Terms

Use the industry's own words, in its own casing. Explain the consequence, not the acronym.

rvrs's own terms, such as run, probe, round, node, origin and history, are defined in `GLOSSARY.md` at the repo root. Load it together with this file.

| Write | Not |
| --- | --- |
| traceroute, MTR, ping, iperf3, DNS, BGP | Trace Route, mtr (in prose), iPerf, iperf (for the protocol) |
| IPv4, IPv6, ICMP, ICMPv6, TCP, UDP | ipv4, Ipv6 |
| ASN, AS13335, prefix, origin AS | AS number 13335, network id |
| RTT, latency, loss, jitter, p50, p95, p99 | ping time, quality |
| hop, path, route, destination, target, host | node (a node is an rvrs node, see `GLOSSARY.md`), server (unless it is one) |
| resolver, authoritative, delegation, NXDOMAIN, SERVFAIL, TTL, DNSSEC, DS, DNSKEY, RRSIG | DNS server error, security |
| throughput, Mbps, Gbps, retransmits, download, upload | speed, internet speed |
| Whois | WHOIS lookup |
| A, AAAA, CNAME, MX, TXT, NS, SOA, HTTPS, CAA records | IP records |

The tool chip is "iperf3"; the tool is iperf3 in prose; the protocol is iperf3. Throughput units are Mbps and Gbps (decimal), latency ms with one decimal.

Words rvrs never uses: leverage, seamless, robust, powerful, best-in-class, next-generation, cutting-edge, revolutionary, unlock, supercharge, elevate, transform, effortless, magic, awesome, insights (as a noun for output), health score, delve, navigate (figuratively), "it's important to note", "whether you're", "at your fingertips", game-changing. Never "Oops", never "Uh-oh", never an exclamation mark in an error, warning or routine success.

## Shape

**result → evidence → interpretation → action.** Not every state needs all four. A normal result stops after the evidence; an anomaly adds the interpretation; a fixable problem adds the action.

- One idea per sentence in the UI. Short declaratives. Full stops in sentences; none in labels, buttons, chips and metadata lines.
- Middle dot " · " joins facts in a status or summary line: "6 hops · 12.4 ms", "100 pkt · 0.0% loss · 13.9 ms avg".
- Numbers over adjectives. "p95 18.1 ms", not "low latency". Keep precision when it is diagnostic; round when it is noise.
- No em dashes in UI copy; a full stop or a middle dot instead.
- Remove a sentence if the screen already shows it. Remove an adjective if a number stands next to it.

## Observation vs inference

Measurements are stated plainly. Inferences are labeled as inferences.

- "likely", "probably", "looks local", "could be normal", "observed on this path" mark inference.
- Never "caused by" without causal evidence.
- Warnings are never stronger than the evidence. Plain, not alarming: no "critical", "danger", "threat", "insecure", "compromised".

Tool rules that protect accuracy:

- **MTR and traceroute.** Loss at a middle hop with clean later hops is the router rate-limiting ICMP replies, not loss on the path. Say "Hop 7 answers 36% of probes. Later hops are clean, so the router is likely rate-limiting its replies." Only loss that carries to the destination is path loss, and only that gets red.
- **Ping.** A time series, not one number: now, median, p95, loss, jitter.
- **DNS.** Different answers are observations, not errors: "Resolvers disagree", then why it can be normal (CDN, geography, cache, policy). Use the exact status: NXDOMAIN, SERVFAIL, timeout, refused. rvrs checks that DNSSEC records are present; it does not validate signatures, so say "signed", never "verified" or "secure".
- **iperf3.** Always direction, transport, duration and server. Never call iperf3 throughput "internet speed".
- **BGP and RPKI (when it lands).** Valid, Invalid, Not Found, exactly. Not Found means no covering ROA, not unsafe. Origin validation says nothing about the whole AS path. Collector data is an observation with a timestamp.
- **TLS, port scans (when they land).** A valid certificate is not "secure"; an open port is reachable, not "vulnerable".

## By surface

**Buttons and menus.** A verb and its object, title case: Run Again, Stop, Copy Address, Copy Results, Compare Resolvers, Add Server, Join the Waitlist. Never "Let's go", "Analyze now", "Get insights".

**Status lines.** Facts joined by " · ": "Resolved · 24 ms · all agree · signed", "Download · 523 Mbps · 3/10 s", "Live · 23 pkt · 0.0% loss · 12.3 ms avg".

**Interpretations and verdicts.** A short title that states the finding, one or two sentences of evidence and inference. "Stable" / "Low loss and steady latency over 20 packets." Not "Great connection!"

**Errors.** Name the layer and what failed; then what to do, if anything. "Couldn't resolve example.com." "No reply from the iperf3 server at host:5201. It may be busy; public servers take one client at a time." Never blame the user, never apologize at length, never "Something went wrong".

**Empty states.** What belongs here, then the shortest way to get it. "No saved runs" / "Save a run to keep it here."

**Footers and explainers.** The default is none. A footer exists only for a fact about the network or the result that the rows above can't show, and then it is one plain, complete sentence. Before writing one, ask what a network engineer would miss without it; if the answer is "nothing", delete it.

- Never explain the controls on the screen. A stepper labeled "Packets: 20" needs no sentence about packets; Play, Stop and swipe-to-delete are iOS conventions, not things to describe.
- Never cross-reference other settings ("shared with traceroute and MTR"). If it matters, the setting belongs in one place.
- Never reassure ("Nothing runs in the background", "Your data stays on the device") unless the person asked or a permission prompt requires it.
- Never write in clipped fragments to sound terse. "TCP. Download runs first." reads like a generator. Write whole sentences with a subject and a verb, or nothing.

These are the footers that shipped and were removed, kept here as what not to write:

| Shipped | Problem | Now |
| --- | --- | --- |
| "Play keeps going past the count. Percentiles need a few dozen packets. Timeout is shared with traceroute and MTR." | Narrates the controls in three fragments and cross-references other screens. | No footer. |
| "Probes go out back to back. More probes give a steadier min, avg and max, and take longer." | Explains a stepper. | No footer. |
| "Shown first in the server menu. Swipe to remove." | Describes the UI and an iOS gesture. | No footer. |
| "A corporate or Pi-hole resolver, for example. Asked on every lookup and shown as its own source. Swipe to remove." | Fragment, example padding, gesture. | No footer. |
| "TCP. A port after the host (host:5201) overrides the default. Download runs first, then upload, each for the duration." | Clipped fragments; three facts where one matters. | "A port in the target field, as in host:5201, overrides the default port." |
| "Compare shows why; often that's a CDN, not a fault." | The "X, not Y" reassurance pattern. | "Compare shows why." |

**AI tells.** These patterns make copy read as generated. rvrs doesn't use them:

- "X, not Y" and "It's not X, it's Y" contrasts used for effect.
- Lists of three for rhythm ("a path, an answer or throughput"), unless there are exactly three real things.
- Strings of one- to three-word fragments ("TCP. Fast. Local.").
- Semicolons joining two short clauses to sound considered.
- Reassurance nobody asked for, and "for example" padding.
- Restating the label in the sentence below it.
- Em dashes, "simply", "just", "seamlessly", "effortlessly", "all the way down".

Read a footer aloud. If it sounds like a product page or a tooltip generator, delete it; if it sounds like an engineer telling a colleague one thing they'd otherwise miss, keep it.

**Accessibility labels.** The same words as the screen, spelled out: "Hop 2, lo0-0.example.net, loss 0 percent, average 10.6 milliseconds".

**Admin app.** Same voice, operator's register: what the number is, over what window, no celebration. "1,204 opens · last 7 days". Empty: "No pulses yet. The app sends one a day per install."

**API.** Error messages are one sentence in sentence case, ending with a period. They name the field and the rule: "email must be a valid address." OpenAPI descriptions say what a route does and what a retry does, in present tense.

**Docs and README.** Answer first, example second, edge cases third. Real commands, real record names. Be candid about platform limits: "iOS does not expose the neighbor table, so rvrs shows what the current interface and an active probe can see."

**Release notes and TestFlight "What to Test".** A user-visible fact and, when interesting, why: "DNS keeps answers grouped by resolver, so CDN differences are easier to spot." Not "exciting improvements".

**Marketing (App Store, site).** One line that states the job, one concrete supporting line, then the product. Default positioning: **See what the network is doing.** Traceroute, MTR, ping, iperf3 and DNS on your iPhone.

## Humor

Dry, rare, and only where it releases tension or rewards expertise, such as an empty history or the About row. Never in errors, incidents, warnings, destructive confirmations or anything a person reads while something is broken.

## Checklist

Before shipping any string:

1. Do the first words state the useful fact?
2. Is there a measured value where one exists?
3. Is observation separated from inference?
4. Are the terms and their casing the industry's, and rvrs's own terms the glossary's?
5. Is any warning stronger than the evidence?
6. Title case for controls and titles, sentence case for the rest?
7. American spelling?
8. Can an adjective go? Can a sentence go?
9. Is the next action obvious, and absent when there is none?
10. Would a network engineer find it exact rather than patronizing?
11. Is every footer a fact the screen can't show, in one whole sentence, or gone?
12. Is it free of the AI tells listed above?
