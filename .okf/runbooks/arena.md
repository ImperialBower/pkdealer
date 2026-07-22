---
type: Runbook
title: Arena — launching tables of bots
description: How to bring up a dealer plus a line-up of bot agents with the launcher scripts in bin/.
tags: [arena, docker, agents, runbook]
timestamp: 2026-07-22T13:10:00Z
---

Three launcher scripts in `bin/` bring up a dealer plus agents via Docker
Compose, from simplest to most flexible:

| Script | Line-up | External deps |
|---|---|---|
| `./bin/botarena` | Full 9-handed ring of every `pkcore` rule archetype | none |
| `./bin/aiarena` | Fixed 3 rule bots + 3 local LLMs (the full demo — see repo `DEMO.md`) | Ollama on host |
| `./bin/arena` | Any line-up you name, from the `arena.toml` registry | depends on agents |

# Examples

```sh
./bin/arena gto gto lag llama     # two GTO bots, a LAG, one llama
./bin/arena gto:3 claude          # colon multiplicity: 3 GTO + 1 Claude
make arena PLAYERS="gto lag llama"
make arena-down                   # force-tear-down ALL arena containers + volumes
```

`arena.toml` maps short names to an agent type (`rules`, `ollama`, `claude`,
`gemini`) and config. The [claude agent](/crates/pkdealer_agent_claude.md)
needs `ANTHROPIC_API_KEY` (live, billed); [rules](/crates/pkdealer_agent_rules.md)
and [ollama](/crates/pkdealer_agent_ollama.md) agents run fully locally.

The arena stack also brings up the full telemetry stack — see the
[observability runbook](/runbooks/observability.md).
