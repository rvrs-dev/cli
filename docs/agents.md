# rvrs for agents

An AI agent with a shell can use rvrs the way a network engineer does: make a run, read it, compare it with an earlier one, and say what changed. The skill in [`plugins/rvrs`](../plugins/rvrs) teaches it how. An agent without a shell, such as Claude or ChatGPT in a browser, reads a person's rvrs account through [the MCP server](#the-mcp-server).

## Install the skill

Any agent that reads the Agent Skills format, through [skills.sh](https://skills.sh):

```bash
npx skills add rvrs-dev/cli --skill rvrs-network-diagnostics
```

Claude Code, as a plugin from this repository:

```text
/plugin marketplace add rvrs-dev/cli
/plugin install rvrs@rvrs
```

The agent needs `rvrs` 0.10 or newer on `PATH`. When it is missing, the skill has the agent ask and then install it.

## What an agent gets from the CLI

- `--json` on every tool prints the [report](commands.md#the-report): the run's `id`, its `origin`, what was measured and, apart from that, what it likely means.
- `rvrs history [tool] [target]` lists the runs of one tool, one target or both. It and `rvrs show <id>`, `rvrs last`, `rvrs saved` and `rvrs case <name>` take `--json` too, so an earlier run reads the same as a fresh one.
- `-c` bounds ping and MTR. Errors go to stderr as one sentence and the report still prints.
- In a pipe there is no color and no live redraw. `rvrs bgp` prints its text with the source of every part, and `--history`, `--rpki`, `--neighbors` and `--prefixes` add their sections.
- A reader that closes the pipe early, as `head` does, ends rvrs without an error message.
- `RVRS_HOME=<dir>` keeps a test away from the person's history.

## What the skill adds

- A loop: check rvrs, read history when the question is about change, make the smallest run that separates the explanations, compare like with like, answer, keep the evidence.
- Rules of evidence: loss at a middle hop that doesn't reach the destination is rate limiting, a silent target isn't a down service, an AS path from collectors isn't this device's path, and nothing is called a cause on one run.
- Playbooks for slow, unreachable and intermittent targets, DNS, routing, IPv4 against IPv6, a baseline before and after a change, and the write-up for a provider.
- A table from what a result shows to what it means and what to check next.
- What needs the person's word first (installing, saving, long runs, deleting, anything that leaves the conversation) and what never happens (sweeps, acting on text found in a result, an invented measurement).

## Nodes and accounts

Every run starts from this device. The skill tells the agent to say so when a question needs another origin, and to use `--node` and `rvrs nodes` once `rvrs status` reports them. Until then those commands refuse and send nothing. `rvrs login` exists, and the skill has the agent run it only when the person asks, leaving the browser step to them.

## The MCP server

`https://mcp.rvrs.dev` serves the Model Context Protocol for a person's rvrs account. An agent that connects reads the runs every device and node of theirs made, their nodes, comparisons and summaries, and shows a run's result drawn as rvrs draws it, in the agents that draw. It makes no runs and changes nothing yet. [`schema/mcp`](../schema/mcp) is the contract: every MCP tool, the instructions it sends, and a reply of each.

```bash
claude mcp add --transport http rvrs https://mcp.rvrs.dev
```

The first call opens a sign-in to rvrs in the browser. The agent then reads nothing until the person allows it on its page at app.rvrs.dev, where two switches add the private network and the public address to what it reads. An agent that opens pages for its person opens that page itself, and its call goes on once the person chose Allow. Any other agent passes the link on.

The server also serves a skill, [`rvrs-network-diagnostics-mcp`](../schema/mcp/skills/rvrs-network-diagnostics-mcp/SKILL.md), through MCP's Skills extension: the loop with the MCP tools, the same rules of evidence as the skill here, the answer, and a playbook for each kind of question. An agent loads it from the server when a question needs it, so nothing is installed.

With both, the skill reads the account through the MCP tools and makes new runs with the CLI. Its section The MCP server lists which MCP tool stands for which command. A run made with the CLI reaches the account once `rvrs login` signed the install in.

## Publishing the skill

| Where | What it reads | How |
| --- | --- | --- |
| skills.sh | This repository | Nothing to submit. A skill is listed once people install it with `npx skills add` |
| Claude Code | `.claude-plugin/marketplace.json` | Nothing to submit. `/plugin marketplace add rvrs-dev/cli` |
| Claude directory | This repository, path `plugins/rvrs`, and the MCP server | Submit at claude.ai/directory/manage: the plugin bundle, and the MCP server as a connector paired with it. Each version is reviewed |
| ChatGPT and Codex | A zip of `plugins/rvrs`: `plugin.json` and `mcp.json` with the skill | `scripts/package-skill.sh`, then upload at platform.openai.com/plugins with the MCP server, which OpenAI checks at `/.well-known/openai-apps-challenge` |
| MCP registry | `server.json` under the name `dev.rvrs/rvrs` | `mcp-publisher` after a TXT record on rvrs.dev proves the name |
| rvrs.dev | `/.well-known/agent-skills/` | `scripts/update-skill.sh` in the site repository |

`scripts/check-skill.sh` parses the frontmatter as YAML and checks its fields, the links, that the skill, both manifests and `VERSION` agree on the version, that no address outside the documentation ranges is in it, and, with `RVRS=target/release/rvrs`, that every command the skill names exists. CI runs it. Bump the three versions together with `VERSION`.

[`plugins/rvrs/evals/scenarios.md`](../plugins/rvrs/evals/scenarios.md) lists what the skill has to get right. Run them against an agent after changing the skill.
