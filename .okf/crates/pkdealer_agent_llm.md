---
type: Rust Crate
title: pkdealer_agent_llm
description: Shared LlmBackend trait and poker-prompt logic used by LLM-backed pkdealer agents.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_agent_llm
tags: [library, agents, llm]
timestamp: 2026-07-22T13:10:00Z
---

Library crate defining the `LlmBackend` trait plus the prompt construction and
response parsing shared by LLM-backed agents. Backends implement the trait;
this crate turns table state into a poker prompt and an LLM reply into a legal
action.

Consumers: [pkdealer_agent_claude](/crates/pkdealer_agent_claude.md),
[pkdealer_agent_ollama](/crates/pkdealer_agent_ollama.md).
