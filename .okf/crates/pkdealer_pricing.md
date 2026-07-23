---
type: Rust Crate
title: pkdealer_pricing
description: Notional per-model token pricing and cost computation, shared by the service and offline analysis.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_pricing
tags: [library, cost, tokens, epic-44]
timestamp: 2026-07-22T13:10:00Z
---

Library crate (EPIC-44) providing per-model token pricing and cost
computation. Prices are configured in the repo-root `pricing.toml`. Shared by
the [service](/crates/pkdealer_service.md) (live token/cost columns) and
[pkdealer_costsim](/crates/pkdealer_costsim.md) (offline analysis).
See [EPIC docs](/references/epic-docs.md) for the design spec.
