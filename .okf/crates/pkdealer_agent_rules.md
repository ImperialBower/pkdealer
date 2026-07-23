---
type: Rust Crate
title: pkdealer_agent_rules
description: Rule-based poker bot agent driven by a pkcore BotProfile archetype.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_agent_rules
tags: [binary, agents, pkcore]
timestamp: 2026-07-22T13:10:00Z
---

Binary agent whose strategy comes from a `pkcore` `BotProfile` archetype
(e.g. GTO-leaning, loose-aggressive). The full 9-handed ring of archetypes is
what `./bin/botarena` launches — see the [arena runbook](/runbooks/arena.md).
Built on [pkdealer_agent_core](/crates/pkdealer_agent_core.md); runs fully
locally.
