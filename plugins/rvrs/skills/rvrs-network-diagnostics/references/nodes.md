# Nodes and accounts

A node is a machine somewhere else that runs rvrs tools on request: a server, a Raspberry Pi, a router. It runs the same binary (`rvrs node`), started with a token the person makes under Installs at app.rvrs.dev. An account keeps a person's runs and library the same on every device they sign in on, and its workspace holds their nodes. Schedules, alerts and `--fleet` are planned.

## Check before you say anything

```bash
rvrs status
```

```text
  – Account          not signed in · rvrs login signs in
  – Nodes            runs come from this device · sign in to ask a node
```

Signed in, the Nodes line counts them: `2 nodes · 1 online`, or `no nodes yet`. `rvrs nodes` lists each with whether it is online, when it was last seen, its version and its tags. Ask a node only when the line counts one that is online. `--fleet` and `rvrs schedule` refuse, send nothing and exit non-zero. Do not run them to find out, and do not describe them as working. `rvrs help` marks each one Soon.

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

## Accounts

The Account line of `rvrs status` says who is signed in, or "not signed in". Signing in puts this install in the person's workspace on app.rvrs.dev. From then on every run made on this device is also kept in their account, with its target, addresses and result, and so are their saved runs, cases and favorites. Runs from their other devices appear in `rvrs history` with the install's name under FROM and in `--json` as `origin.install`. Say this before a login, since it changes what leaves the device, and sign in only when the person asks to.

Compare a run only with runs of the same origin. A run with `origin.install` set was made on another device, on another network path. `rvrs sync` sends and reads now. `RVRS_SYNC=0` keeps a signed-in install's runs on the device.

1. On a yes, run `rvrs login --no-browser`. It prints a code and a link and waits up to ten minutes. Give the person both. They open the link, sign in or make their account there, and confirm the code. You never type an email address, a password or a code into a page, and you never accept terms on their behalf.
2. The command returns once they confirmed. `rvrs status` then names the account.
3. On a device nobody sits at, the person makes a token under Installs on app.rvrs.dev and sets it as `RVRS_TOKEN`. Never ask them to paste a token into the conversation.

`rvrs logout` signs out and revokes this install's credential. An account is never needed to run a tool. Do not sign in or out unasked, and never print or copy the contents of `credentials.json`.
