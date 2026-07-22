---
type: Rust Crate
title: pkdealer_agent_core
description: Shared gRPC agent infrastructure used by every pkdealer bot agent.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_agent_core
tags: [library, agents, grpc]
timestamp: 2026-07-22T13:10:00Z
---

Library crate with the common plumbing every bot agent shares: connecting to
the [dealer service](/crates/pkdealer_service.md), seating itself, polling for
its turn, and submitting actions over the
[Dealer gRPC API](/interfaces/dealer-grpc-api.md).

Consumers: [pkdealer_agent_random](/crates/pkdealer_agent_random.md),
[pkdealer_agent_rules](/crates/pkdealer_agent_rules.md),
[pkdealer_agent_claude](/crates/pkdealer_agent_claude.md),
[pkdealer_agent_ollama](/crates/pkdealer_agent_ollama.md).
