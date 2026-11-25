# The MCP server

How an agent reads a person's rvrs account through the Model Context Protocol. `tools.json` is every MCP tool as the server lists it, with its input and output schemas, `instructions.md` is what the server tells every agent, and `fixtures/` holds one reply per MCP tool. This page says what each MCP tool answers and what holds for all of them.

Status: version 1, the first of three releases. It reads and draws. It makes no runs and changes nothing. The rvrs API repository serves it at `https://mcp.rvrs.dev`, and a self-hosted install serves it at its own address. Anyone may build an agent that speaks it.

## Speaking to it

- Streamable HTTP at the server's address, one JSON-RPC request per POST. GET and DELETE answer 405, since nothing is held between requests.
- MCP `2026-07-28` first: `server/discover`, and every request on its own with its `_meta`. An agent on the 2025 handshake (`2025-11-25`, `2025-06-18`, `2025-03-26`) gets its `initialize` answered and each later request served without a session.
- Every request carries a bearer token from the identity provider the server names. Without one the answer is 401 with `WWW-Authenticate: Bearer resource_metadata="<server>/.well-known/oauth-protected-resource", scope="openid email profile offline_access"`. The metadata names the provider as the one authorization server.
- The token must be one the provider issued for this server's address (RFC 8707), signed with the provider's keys. A token for anything else is refused.
- The token's subject is the person and its OAuth client is the agent. A token that names no OAuth client (`client_id` or `azp`) is refused. An agent reads nothing until the person allows it on its page in the web app, which shows the name the agent gave itself beside its OAuth client. Until then every MCP tool answers with that page's link, as an error result.

## The MCP tools

All of them only read, and say so: `readOnlyHint` true, `destructiveHint` false, `openWorldHint` false. Each has a title and an output schema.

| MCP tool | Answers | Takes |
| --- | --- | --- |
| `get_status` | Whose account it is, the workspace and plan, the runs it holds and the newest, the installs by product (`ios`, `cli`, `node`, `mcp`), the nodes and how many are online, and what this agent may read | Nothing |
| `find_runs` | Runs, newest first, as one summary each with a link | `tool`, `target`, `search` (part of a target), `install` (a name or id), `tags` (a selector), `trigger`, `schedule`, `outcome`, `loss`, `since` (`24h`, `7d`, `30d`), `case`, `saved`, `like` (a run id), `limit` (up to 50) and `cursor` |
| `get_run` | One run: who made it and from where, and its report. A run asked of a node that hasn't ended is waited for up to 30 seconds | `id`, and `detail`: `brief` leaves out every probe's sample, `full` keeps them |
| `list_nodes` | The workspace's nodes by name: online or not, when last heard from, version, build features, tags | `tags` (a selector), `limit` (up to 200), `cursor` |
| `compare_runs` | Two to six runs against the first: what they don't share, each figure with its distance from the baseline, and the TTL where each path leaves the baseline's | `ids` |
| `summarize_runs` | A window's runs grouped by origin or by target, each against the window before, with why a group is worth a look, then latency, jitter and loss over time for ping, traceroute and MTR | `window`, `by` (`install`, `target`), and `tool`, `install` and `target` for the latency over time |

`like` lists the runs of the same tool and target from the same origin, to the same destination address over the same IP family, without the run itself. `compare_runs` names what differs from these fields, plus the probe count, in `differs` before any figure: `tool`, `target`, `address`, `family`, `origin`, `count`.

A selector is the one `rvrs schedule` and the node protocol take: `isp=telia`, `isp!=telia`, `isp`, `!isp`, `line in (ftth,cable)`, `line notin (dsl,lte)`, joined by commas to mean and.

## A reply

Every successful reply has three parts:

- **Text**, which an agent shows or reads as it is: a status line with facts joined by " · ", times in UTC, and a link for every run, node and list. Agents that read no structured content get the whole answer from it.
- **`structuredContent`**, which matches the MCP tool's output schema. `fixtures/<tool>.reply.json` is an example of each.
- **`_meta`**, for `get_run` only: `dev.rvrs/report` is the report with every sample, for the drawing. A model doesn't read it.

A refused call is a result with `isError` and one sentence that says why and what to do: `No run <id> in this account.`, a page to open, a selector that can't be read.

The report inside a reply is the [run report](../README.md), and every rule under Reading a report holds for it: the sentences are the maker's, a missing figure is absent and never zero, and `interpretation` holds inferences.

## What an agent reads

An agent reads what the server leaves in a report after one rule, applied before any MCP tool works with it, so text, data, comparisons and the drawing all see the same:

- At the start it reads figures, outcomes, interpretations, paths and the names a workspace gave its installs, nodes and tags. The two parts of [What points to a home](../README.md#what-points-to-a-home) are both left out: a hop of the private network the run started in keeps its figures and loses its address and its name, and the line's public address, what gives it away and the origin's location are gone.
- The person turns either part on, for one agent, on its page in the web app: Private Networks and Public Address.
- The sentences go by the same rule. `summary`, `error`, `interpretation` and `sentence`, and a run's summary in a list, lose every address and name the cut took off, matched whole, and while Private Networks is off any private or link-local address they name. What was taken out reads `(left out)`.

## The drawing

`get_run` names `ui://rvrs/run.html` in `_meta.ui.resourceUri`. It is an MCP App (`io.modelcontextprotocol/ui`, `2026-01-26`) with the type `text/html;profile=mcp-app`: one HTML file that loads nothing from the network and asks for no address in its policy. An agent that draws shows the run as the web app does: the status line, the figures, the latency chart with p50 and p95, jitter and loss to switch off, the path by network with each hop opening for its figures, rounds and network, and a DNS run's sources and records. It takes the agent's theme, colors and fonts and keeps rvrs's colors for the data. Open Run asks the agent to open the run's page. An agent that draws nothing shows the text.

## Instructions and prompts

`instructions.md` goes to every agent with `server/discover` and `initialize`. It holds the skill's rules for reading a report and for evidence, and what to say when no run in the account answers a question: which run would, with the command that makes it on the person's own machine, in the iPhone and iPad app or from a node.

Three prompts a person picks in their agent, each taking a `target`: `was_it_like_this_yesterday`, `compare_origins` and `collect_evidence`.

## Changes

- Changes are additive. A field, an MCP tool or a value is added and never renamed or given another meaning. A new shape is a new MCP tool.
- A reader ignores fields it doesn't know.
- The app repository copies these files to `packages/schema/fixtures/mcp`, and its tests fail while the server's `tools/list` or its instructions differ from them. Change them here first.

## Planned

The second release runs: `request_run` and `get_request`, a yes rvrs checks on a web page before a run starts, and requests for several nodes. The third adds cases, favorites, schedules, BGP, saving, comments and sharing, more drawings, and `rvrs mcp`, the same MCP tools served over stdio for this device.
