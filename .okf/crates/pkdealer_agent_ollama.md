---
type: Rust Crate
title: pkdealer_agent_ollama
description: Ollama-backed local-LLM poker agent for pkdealer with OTel gen_ai instrumentation.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_agent_ollama
tags: [binary, agents, llm, ollama, otel]
timestamp: 2026-07-22T13:10:00Z
---

Binary agent backed by a local Ollama model via the `LlmBackend` trait from
[pkdealer_agent_llm](/crates/pkdealer_agent_llm.md). Requires Ollama running
on the host but no API keys — fully local and free. Emits OTel `gen_ai` spans
(see the [observability runbook](/runbooks/observability.md)). Used by
`./bin/aiarena` for the full demo stack.
