# Workspace Crates

Core service and wire format:

* [pkdealer_proto](pkdealer_proto.md) - shared protobuf definitions and tonic-generated Rust types
* [pkdealer_service](pkdealer_service.md) - the gRPC dealer server binary
* [pkdealer_client](pkdealer_client.md) - gRPC client binary for the dealer service

Bot agents:

* [pkdealer_agent_core](pkdealer_agent_core.md) - shared gRPC agent infrastructure used by every bot
* [pkdealer_agent_random](pkdealer_agent_random.md) - random baseline bot agent
* [pkdealer_agent_rules](pkdealer_agent_rules.md) - rule-based bot driven by a pkcore BotProfile
* [pkdealer_agent_llm](pkdealer_agent_llm.md) - shared LlmBackend trait and poker-prompt logic
* [pkdealer_agent_claude](pkdealer_agent_claude.md) - Claude-backed LLM agent
* [pkdealer_agent_ollama](pkdealer_agent_ollama.md) - Ollama-backed local-LLM agent

Cost accounting:

* [pkdealer_pricing](pkdealer_pricing.md) - notional per-model token pricing shared by service and offline analysis
* [pkdealer_costsim](pkdealer_costsim.md) - offline token-accounting and cost analysis over recorded arena sessions

Collusion detection:

* [pkdealer_boss](pkdealer_boss.md) - the Boss: blind collusion detection (redacted signals + SPRT) over recorded arena sessions (EPIC-70)
