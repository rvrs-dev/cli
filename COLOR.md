# rvrs color

Every figure rvrs measures has one color, and every state has one color. They are the same in the app, the CLI, the web app and the site, so jitter looks like jitter wherever a person reads it. Everything else is ink. Identical copies live in the app (`../ios-app`, the canonical one), API (`../app`), CLI (`../cli`) and site (`../site`) repos. Change them together.

## Figures

A figure keeps its color on every chart, legend, tooltip and chip that names it.

| Figure                                      | Color  | Web                 | App       | Terminal |
| ------------------------------------------- | ------ | ------------------- | --------- | -------- |
| latency: a probe's RTT, an average, the p50 | green  | `--figure-latency`  | `.green`  | green    |
| p95                                         | blue   | `--figure-p95`      | `.blue`   | blue     |
| p99                                         | violet | `--figure-p99`      | `.purple` | magenta  |
| jitter                                      | violet | `--figure-jitter`   | `.purple` | magenta  |
| loss: a probe without a reply, a loss rate  | red    | `--figure-loss`     | `.red`    | red      |
| download                                    | blue   | `--figure-download` | `.blue`   | blue     |
| upload                                      | violet | `--figure-upload`   | `.purple` | magenta  |

- Figures that share a color never share a chart. A run's chart draws jitter and leaves the p99 out, and the hop panel draws the p99 and leaves jitter out. An iperf3 chart draws no percentiles.
- A percentile drawn as a level across the chart is a dashed line.
- The area under a line is the line's own color, faint.
- A probe without a reply is a red mark on the baseline, and the line breaks there.
- When one chart draws a figure for several things, such as the nodes of a schedule or the runs in Compare, each thing is a step of the figure's color, the first the strongest. Outside the chart, in a legend or a table, its swatch is a short line in the same step of the ink, so it never reads as a state's dot.
- A figure's number in a table, a status line or a tile is ink. Color in text marks a state.

## States

A state is how something stands. It is shown as a dot beside its label or as the tint of the value it judges, and never by color alone.

| State        | Color | App          | Terminal | Where                                                                                                 |
| ------------ | ----- | ------------ | -------- | ----------------------------------------------------------------------------------------------------- |
| good         | green | `.green`     | green    | reached, ended, online, signed, Valid, resolvers agree                                                |
| worth a look | amber | `.orange`    | yellow   | expired, answers differ, a round far over the hop's median, loss at a hop that later hops don't share |
| problem      | red   | `.red`       | red      | failed, Invalid, broken DNSSEC, loss that reaches the destination                                     |
| in progress  | blue  | `.blue`      | blue     | a run a node is making                                                                                |
| none         | gray  | `.secondary` | dim      | requested, offline, paused, unknown                                                                   |

- A warning is never stronger than the evidence, as the voice guide says. Loss at a middle hop with clean later hops is amber at most.
- A scale that judges a figure uses green, amber and red, and states its steps where it is shown. The Overview's install strip is one. The CLI's latency is another: green under 50 ms, yellow from 50 to 150 ms, red above, in its output and in the Console's latency bars, at the user's request of 2026-11-15.

## Everything else is ink

- Nothing gets a hue for being itself. A tool, a node, a run, an install, a product, a role and a message's source are told apart by their names.
- A count of runs, opens or installs is ink, stronger where there are more.
- Parts of a whole, such as the rings on the web's Overview, take the categorical hues `--series-1` to `--series-5` in that order, and the rest is one faint part. They never use a figure's or a state's color.
- Chrome is ink at its opacities with one accent: `--brand` on the web and the near-black accent in the app. The Console's header keeps its yellow labels, blue keys and cyan selection, since a terminal has six hues and the header holds no figure.

## Before shipping a chart

1. Is every figure in its color from the table, in the chart, its legend and its tooltip?
2. Does one color stand for two figures on the same chart?
3. Does every colored dot sit beside a label?
4. Does a tool, a node, a run or an install carry a hue? It should be ink.
5. Is it checked in light and dark, and on a terminal with a dark and a light theme?
