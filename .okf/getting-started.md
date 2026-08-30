---
type: Project Overview
title: PKDealer — gRPC Poker Dealer Service
description: Rust workspace providing a gRPC poker dealer service, client, shared protobuf definitions, and a family of bot agents.
resource: https://github.com/ImperialBower/pkdealer
tags: [poker, grpc, rust, agents, overview]
timestamp: 2026-08-30T12:00:00Z
---

# Overview

PKDealer is a Rust workspace (edition 2024, 14 member crates) built around a
gRPC **dealer service** that manages a poker table: seating players, dealing
hands, processing actions (bet / call / raise / fold), advancing streets, and
resolving showdowns. Bot agents connect over gRPC and play autonomously —
rule-based archetypes from the upstream `pkcore` crate, a random baseline, and
live LLM players backed by Claude or a local Ollama model.

Start here, then follow:

* [Crates](/crates/index.md) — one concept per workspace crate.
* [Dealer gRPC API](/interfaces/dealer-grpc-api.md) — the wire contract.
* [Arena runbook](/runbooks/arena.md) — composing tables of bots.
* [Observability runbook](/runbooks/observability.md) — the OTel stack.
* [Developer workflow](/runbooks/developer-workflow.md) — build, test, lint.
* [EPIC docs](/references/epic-docs.md) — where design work is specified.

# Key facts

| Fact | Value |
|---|---|
| Workspace root | `Cargo.toml` (resolver 2, 14 crates under `crates/`) |
| Upstream domain crate | `pkcore` (hand evaluation, bot profiles) |
| Wire format | Protobuf / tonic gRPC (`proto/dealer.proto`) |
| Spectator UI | Separate repo: [pkspectator](https://github.com/ImperialBower/pkspectator) |
| License | MIT OR Apache-2.0 |

# Citations

[1] [README.md](https://github.com/ImperialBower/pkdealer/blob/main/README.md)
