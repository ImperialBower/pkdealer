---
type: Rust Crate
title: pkdealer_costsim
description: Offline token-accounting and notional-cost analysis over recorded pkdealer arena sessions.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_costsim
tags: [binary, cost, tokens, epic-44]
timestamp: 2026-07-22T13:10:00Z
---

Offline analysis tool (EPIC-44 Phase 0) that replays recorded arena sessions
(exported via the `ExportSession` RPC — see
[Dealer gRPC API](/interfaces/dealer-grpc-api.md)) and computes token usage
and notional cost per agent, using prices from
[pkdealer_pricing](/crates/pkdealer_pricing.md).
See [EPIC docs](/references/epic-docs.md) for the design spec.
