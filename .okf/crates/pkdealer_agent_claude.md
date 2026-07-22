---
type: Rust Crate
title: pkdealer_agent_claude
description: Claude LLM poker agent for pkdealer with OTel gen_ai instrumentation.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_agent_claude
tags: [binary, agents, llm, claude, otel]
timestamp: 2026-07-22T13:10:00Z
---

Binary agent backed by the Anthropic Claude API via the `LlmBackend` trait
from [pkdealer_agent_llm](/crates/pkdealer_agent_llm.md). Requires
`ANTHROPIC_API_KEY` in the calling shell — calls are **live and billed**.
Emits OTel `gen_ai` spans so model calls are traceable end-to-end (see the
[observability runbook](/runbooks/observability.md)).
