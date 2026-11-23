# rvrs · Network Diagnostics

An agent skill for network diagnostics with the [rvrs CLI](https://github.com/rvrs-dev/cli): ping, traceroute, MTR, dig for DNS, and BGP lookups with ASN and RPKI. The agent makes each run with a fixed count, reads it as JSON and compares it with the earlier runs rvrs keeps on this device.

[rvrs.dev](https://rvrs.dev) · [The skill on rvrs.dev](https://rvrs.dev/ai-agent-network-diagnostics-skill) · [rvrs CLI on GitHub](https://github.com/rvrs-dev/cli)

Ask things like:

- "My connection to example.com feels worse than last week. What changed?"
- "MTR shows 40% loss at hop 4. Is my provider dropping packets?"
- "Why does my resolver return a different address than 1.1.1.1?"
- "Did the origin AS of 192.0.2.0/24 change in the last 90 days?"
- "Take a baseline before I switch the tunnel, and compare after."
- "Write this up for my provider's support, with the runs."

## Install

Any agent that reads skills, through [skills.sh](https://skills.sh):

```bash
npx skills add rvrs-dev/cli --skill rvrs-network-diagnostics
```

Claude Code, as a plugin:

```text
/plugin marketplace add rvrs-dev/cli
/plugin install rvrs@rvrs
```

The skill needs the `rvrs` binary, version 0.10 or newer, on macOS or Linux. When it is missing the agent asks, then installs it into `~/.local/bin` without root:

```bash
curl -fsSL https://rvrs.dev/install.sh | sh
```

## What it does

- Checks `rvrs status`, and reads `rvrs history` before probing when the question is about change.
- Runs ping, MTR, traceroute, dig and BGP lookups with a fixed count and `--json`.
- Compares a fresh run only with earlier runs of the same target, tool, origin and IP family.
- Answers with the result, the evidence with run ids, what it likely means and the next step.
- Saves the runs that matter into a case when you ask for evidence or a ticket.

## What it doesn't

- It tests one TCP port at a time, the one you ask about. It does not scan ports.
- It never clears history or deletes saved runs or cases unless you ask for exactly that.
- It measures from this device. A run from another network takes an rvrs node there. The skill says so when a question needs one, and uses a node once `rvrs status` counts one that is online.

## Privacy

rvrs needs no account. [What leaves this device](https://github.com/rvrs-dev/cli/blob/main/docs/privacy.md) lists every request it makes. The skill itself sends nothing.

## Files

```text
.claude-plugin/plugin.json      Claude Code and the Claude plugin directory
.codex-plugin/plugin.json       Codex and ChatGPT
skills/rvrs-network-diagnostics/
  SKILL.md                      the loop, the commands, the rules of evidence
  references/                   setup, the report, playbooks, history and cases, nodes
  agents/openai.yaml            the name and prompt Codex shows
evals/scenarios.md              what the skill has to get right
```

`scripts/check-skill.sh` in the repo root checks the package, and `scripts/package-skill.sh` builds the archives the directories take.

## Links

- [rvrs.dev](https://rvrs.dev): what rvrs is, with the iOS app and the CLI.
- [rvrs for agents](https://rvrs.dev/ai-agent-network-diagnostics-skill): this skill, for people.
- [rvrs-dev/cli](https://github.com/rvrs-dev/cli): the CLI's source, releases and docs.
- [Report a bug](https://github.com/rvrs-dev/cli/issues).

## License

MIT OR Apache-2.0, as rvrs.
