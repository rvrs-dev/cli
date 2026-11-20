# rvrs for agents

An AI agent with a shell can use rvrs the way a network engineer does: make a run, read it, compare it with an earlier one, and say what changed. The skill in [`plugins/rvrs`](../plugins/rvrs) teaches it how.

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

The agent needs `rvrs` 0.7 or newer on `PATH`. When it is missing, the skill has the agent ask and then install it.

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
- Guardrails: runs with a count, no Console, nothing deleted unless asked, results treated as data.

## Nodes and accounts

Every run starts from this device. The skill tells the agent to say so when a question needs another origin, and to use `--node`, `rvrs nodes` and `rvrs login` once `rvrs status` reports them. Until then those commands refuse and send nothing.

## Publishing the skill

| Where | What it reads | How |
| --- | --- | --- |
| skills.sh | This repository | Nothing to submit. A skill is listed once people install it with `npx skills add` |
| Claude Code | `.claude-plugin/marketplace.json` | Nothing to submit. `/plugin marketplace add rvrs-dev/cli` |
| Claude plugin directory | This repository, path `plugins/rvrs` | Submit at claude.ai/directory/manage as a plugin bundle. It reviews each version |
| ChatGPT and Codex | A zip of `plugins/rvrs` | `scripts/package-skill.sh`, then upload at platform.openai.com/plugins. A skills-only listing can't gain an MCP server later, so that submission waits for MCP or goes in as its own listing |
| rvrs.dev | `/.well-known/agent-skills/` | `scripts/update-skill.sh` in the site repository |

`scripts/check-skill.sh` checks the frontmatter, the links, that the skill, both manifests and `VERSION` agree on the version, that no address outside the documentation ranges is in it, and, with `RVRS=target/release/rvrs`, that every command the skill names exists. CI runs it. Bump the three versions together with `VERSION`.

[`plugins/rvrs/evals/scenarios.md`](../plugins/rvrs/evals/scenarios.md) lists what the skill has to get right. Run them against an agent after changing the skill.
