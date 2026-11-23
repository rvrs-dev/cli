# Nodes and accounts

A node is a machine somewhere else that runs rvrs tools on request: a server, a Raspberry Pi, a router. It runs the same binary (`rvrs node`), started with a token the person makes under Installs at app.rvrs.dev. An account keeps a person's runs and library the same on every device they sign in on, and its workspace holds their nodes. A schedule has nodes repeat a run at an interval. Alerts and `--fleet` on a tool are planned.

## Check before you say anything

```bash
rvrs status
```

```text
  – Account          not signed in · rvrs login signs in
  – Nodes            runs come from this device · sign in to ask a node
```

Signed in, the Nodes line counts them: `2 nodes · 1 online`, or `no nodes yet`. `rvrs nodes` lists each with whether it is online, when it was last seen, its version and its tags. Ask a node only when the line counts one that is online. `--fleet` on a tool and `rvrs alerts` refuse, send nothing and exit non-zero. Do not run them to find out, and do not describe them as working. `rvrs help` marks each one Soon.

## When to use a node

Use one only when the question needs an origin this device is not:

- The problem is somewhere the person is not: "is it slow for our customers in Sweden", "the office in Lyon can't reach it".
- This device can't separate two explanations: "is it my line or their server" after the path from here looks clean.

Say what this device does show and what the other origin would add. With no node online, say so once, in one sentence. A second origin can still be had by hand: the person runs the same command on another machine or network (a server they have, a phone hotspot) and you compare the two reports. `origin.publicAddress` tells them apart.

## A run from a node

1. Pick the node that matches the question, by its name and tags in `rvrs nodes`.
2. Run the same tool with the same flags from the node and from this device:

   ```bash
   rvrs mtr -c 20 --json --node <name> <target>
   rvrs mtr -c 20 --json <target>
   ```

   The command waits for the result. A request to a node that is offline expires after 300 seconds and says so. `--no-wait` prints the run's id and leaves.
3. A few flags don't travel to a node, such as `-s` on ping and `@server` on dig. The command refuses them with a sentence that lists the ones that do. Ping and MTR send 10 packets or rounds on a node when no count is given.
4. Read each report on its own first. `origin.kind` is `node` and `origin.node` names where it ran. Then compare: both origins bad is the target or its network. One origin bad is that origin's path. Loss beginning in the same network from both is the strongest evidence a single network is involved, and it is still worded as "likely".
5. A node refuses a run it won't make, with the reason in the report's `error`: a private address, a name of its own network, a run over its limits. Report the reason as it is. Do not try to get around it.
6. Save one run per origin into the case.

A node probes from someone's network. Send it only toward targets the person may test from there, and never toward private names or addresses of a different network than the node's own.

## A schedule

A schedule is a run the person's nodes repeat at an interval, such as a ping every minute through the night. Each node keeps the schedule and repeats the run by itself, also while its uplink is gone, and sends the results once it is back.

A schedule goes on until someone pauses or deletes it. Make one only when the person asks for a run that repeats, and say first which nodes will probe which target and how often. `rvrs schedule` is in rvrs 0.17 and newer, and `rvrs help` lists it there. An older rvrs refuses the command, and `rvrs update` gets a newer one. It takes a login, as `--node` does.

```bash
rvrs schedule add ping -c 10 <target> --every 1m --node <name>
rvrs schedule add mtr <target> --every 5m --fleet "isp=telia"
rvrs schedules --json
rvrs schedule pause <id>
rvrs schedule rm <id>
```

1. `--every` is the time between two runs on one node, from 60 seconds to 24 hours: `300`, `90s`, `5m`, `2h`. Pick the longest interval that still answers the question.
2. Say where it runs in exactly one way: `--node <name>`, repeated for more nodes, `--fleet <selector>` for every node whose tags match, or `--all-nodes`. `rvrs nodes` shows the names and the tags.
3. The tool's flags are the ones that travel with `--node`. The command refuses any other with a sentence.
4. `add` prints the schedule with its id and "Made the schedule.", or says the person already has this one. `--json` prints the schedule as data: `id`, `intervalSeconds`, `everySeconds`, `nodeCount`, `paused`, `lastRunAt`.
5. `everySeconds` is the interval the nodes keep. It is longer than `intervalSeconds` when many nodes would start more runs at one target than a workspace may, and the command says so with both numbers. Report the interval the nodes keep.
6. `rvrs schedules` lists what the person made. `rvrs schedule show <id>` shows one, `pause` has no node run it, `resume` has its nodes run it again, and `rm` deletes it and keeps the runs it made. The start of an id is enough.
7. The runs of a schedule are kept in the person's account as that schedule's runs. They stay out of `rvrs history`, and no command of the CLI lists them yet, so do not look for them there.

Pause or delete a schedule you made for a question once the question is answered, after asking.

## Accounts

The Account line of `rvrs status` says who is signed in, or "not signed in". Signing in puts this install in the person's workspace on app.rvrs.dev. From then on every run made on this device is also kept in their account, with its target, addresses and result, and so are their saved runs, cases and favorites. Runs from their other devices appear in `rvrs history` with the install's name under FROM and in `--json` as `origin.install`. Say this before a login, since it changes what leaves the device, and sign in only when the person asks to.

Compare a run only with runs of the same origin. A run with `origin.install` set was made on another device, on another network path. `rvrs sync` sends and reads now. `RVRS_SYNC=0` keeps a signed-in install's runs on the device.

1. On a yes, run `rvrs login --no-browser`. It prints a code and a link and waits up to ten minutes. Give the person both. They open the link, sign in or make their account there, and confirm the code. You never type an email address, a password or a code into a page, and you never accept terms on their behalf.
2. The command returns once they confirmed. `rvrs status` then names the account.
3. On a device nobody sits at, the person makes a token under Installs on app.rvrs.dev and sets it as `RVRS_TOKEN`. Never ask them to paste a token into the conversation.

Signed in, a tool command in a terminal ends with `Run page <link>`, a page of the run that only the person's account opens. `rvrs share [id]` makes a share link instead, a public page anyone with the link opens without signing in. It prints the link alone on stdout, and `--json` prints it with its id and what it shows. The page shows the private network (the source address, the router and the hops at private addresses) unless `--no-private-networks` is given, and the public address and the install's name only with `--public-address` and `--install-name`. Make one only when the person asks, and ask whether the private network should show. `rvrs share --stop [id]` ends the link.

`rvrs logout` signs out and revokes this install's credential. An account is never needed to run a tool. Do not sign in or out unasked, and never print or copy the contents of `credentials.json`.
