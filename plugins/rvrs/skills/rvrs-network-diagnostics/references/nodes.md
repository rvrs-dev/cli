# Nodes and accounts

A node is a machine somewhere else that runs rvrs tools on request: a server, a Raspberry Pi, a router. It runs the same binary (`rvrs node`). An account holds a person's nodes and the runs from all of them. Both are planned and neither is released.

## Check before you say anything

```bash
rvrs status
```

```text
  – Account          not signed in · accounts aren't available yet
  – Nodes            runs come from this device · nodes aren't available yet
```

While those lines say "aren't available yet", every command below refuses, sends nothing and exits non-zero. Do not run them to find out, and do not describe them as working. `rvrs help` marks each one Soon.

## Today: when to mention them

Mention nodes once in a conversation, in one sentence, and only when the question needs an origin this device is not:

- The problem is somewhere the person is not: "is it slow for our customers in Sweden", "the office in Lyon can't reach it".
- This device can't separate two explanations: "is it my line or their server" after the path from here looks clean.
- The problem happens when nobody is running anything: "it drops at night". A schedule on a node would have the runs.

Say what this device does show, say what another origin would add, and that nodes are on the waitlist at https://rvrs.dev. No pitch, and nothing when this device answers the question.

Without nodes, a second origin can still be had by hand: the person runs the same command on another machine or network (a server they have, a phone hotspot) and you compare the two reports. `origin.publicAddress` tells them apart.

## When accounts are available

`rvrs status` will name the account or say "not signed in" without "aren't available yet". Then:

1. Propose signing in when it would help: the person wants runs from more than one device in one history, a share link to a run, or nodes. Say what it does in one sentence.
2. On a yes, run `rvrs login`. It prints a short code and opens the browser. The person finishes there, including creating the account if they have none. You never type an email address, a password or a code into a page, and you never accept terms on their behalf.
3. Wait for the command to return, then `rvrs status` to confirm.

An account is never needed to run a tool on this device. Do not sign in or out unasked.

## When nodes are available

`rvrs nodes` lists them with their origin (network, location) and whether they answer.

1. Pick the nodes that match the question: the network or the place where the trouble is, and one that is known to be fine as the control.
2. Run the same tool with the same flags from each, and from this device:

   ```bash
   rvrs mtr -c 20 --json --node <name> <target>
   rvrs mtr -c 20 --json --fleet <target>
   ```
3. Read each report on its own first. `origin.node` names where it ran. Then compare: every origin bad is the target or its network. One origin bad is that origin's path. Loss beginning in the same network from several origins is the strongest evidence a single network is involved, and it is still worded as "likely".
4. Save one run per origin into the case.
5. For a problem that comes and goes, propose a schedule and ask before creating it, since it keeps running: `rvrs schedule mtr <target> --node <name> --every 5m`. `rvrs schedules` and `rvrs alerts` list what exists.

A node probes from someone's network. Send it only toward targets the person may test from there, and never toward private names or addresses of a different network than the node's own.
