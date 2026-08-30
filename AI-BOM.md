# AI Bill of Materials — pkdealer

_Last updated: 2026-08-30 · pkdealer workspace v0.1.26 · pkcore v0.11.0_

An inventory of every AI component in this repository — the development tools
used to build it, the machine-readable context it ships, the algorithms
implemented within it, and the external AI services it calls at runtime.
Modeled on the SBOM concept applied to AI systems, and on the companion
[`AI-BOM.md`](https://github.com/ImperialBower/pkcore/blob/main/AI-BOM.md) in
`pkcore`.

Where `pkcore` is the pure poker engine, **pkdealer is the repository where the
external LLM integrations actually live** — the ones `pkcore`'s AI-BOM lists as
"planned in pkdealer".

---

## 1. Development Tools

AI tools used to author this codebase. Not shipped, but relevant to provenance.

| Tool | Vendor | Role | Notes |
|------|--------|------|-------|
| Claude Code | Anthropic | Primary AI coding assistant | Every EPIC in `docs/`; standards in [`CLAUDE.md`](./CLAUDE.md) |

---

## 2. AI Context Infrastructure

Machine-readable knowledge that AI development tools consume — and maintain —
when working on this repository.

| Component | Format | Notes |
|-----------|--------|-------|
| [`CLAUDE.md`](./CLAUDE.md) | Markdown instructions | Testing gates, doc-test rules, `unwrap`/`expect` ban, OKF maintenance + validation gate |
| [`.okf/`](./.okf/index.md) | OKF v0.1 knowledge bundle (21 concepts) | One concept per workspace crate, the `DealerService` gRPC contract, runbooks (arena, observability, developer workflow), the EPIC doc convention |
| OKF skills plugin | `scaccogatto/okf` v0.4.0 (Claude Code plugin) | Authors, maintains and validates the bundle (`/okf:validate .okf --strict`) |
| [`arena.toml`](./arena.toml) | TOML roster | Declares each seat's agent `type` and `model`; the single source of truth the dynamic arena runner reads |
| [`pricing.toml`](./pricing.toml) | TOML rate table | Notional per-million-token USD rates keyed by model id, consumed by cost simulation |

Nothing in the bundle is embedded into a compiled binary (no `include_str!` /
`include_dir!`), so it costs zero bytes at runtime.

---

## 3. AI Audits

Formal reviews performed by AI models. Full reports in [`docs/audits/`](./docs/audits).

| Date | Model | Report | Scope |
|------|-------|--------|-------|
| 2026-07-23 | Claude | [`docs/audits/2026-07-23_Security_Review_boss_branch.md`](./docs/audits/2026-07-23_Security_Review_boss_branch.md) | Security review of the collusion-detection branch |
| 2026-07 | GPT-5.4 (high effort) | [`docs/audits/EPIC-46_Collusion_Detection_EVAL_GPT-5.4-High.md`](./docs/audits/EPIC-46_Collusion_Detection_EVAL_GPT-5.4-High.md) | Evaluation of the collusion-detection design |

---

## 4. External AI Integrations

Live model calls made by agent binaries during play. Each agent is a separate
process that speaks gRPC to the dealer; the dealer itself calls no model.

| Service | Crate | Endpoint | Default model | Status |
|---------|-------|----------|---------------|--------|
| Anthropic Claude | [`pkdealer_agent_claude`](./crates/pkdealer_agent_claude) | `https://api.anthropic.com` | `claude-sonnet-4-6` (`ANTHROPIC_MODEL`) | Complete |
| Ollama (local / on-prem) | [`pkdealer_agent_ollama`](./crates/pkdealer_agent_ollama) | `http://localhost:11434` (`OLLAMA_HOST`) | `llama3.1` (`OLLAMA_MODEL`) | Complete |
| Google Gemini | `pkdealer_agent_gemini` | — | `gemini-2.0-flash` | **Planned** — reserved in `arena.toml` (`type = "gemini"`, seat `gwen`); crate not yet built |

Credentials are read from the environment only (`ANTHROPIC_API_KEY`); no key is
committed. See [`.env.example`](./.env.example).

The shared prompt/parse layer is [`pkdealer_agent_llm`](./crates/pkdealer_agent_llm),
whose `LlmBackend` trait is the single seam every provider implements — it
returns the model's text plus input/output token counts, which is what makes
§6 cost accounting possible.

### Local model variants

`make pokerbench-models` bakes sampled solver-optimal decisions into each base
model's system prompt, producing the `pkpoker-*` Ollama variants
(`pkpoker-gemma`, `pkpoker-llama`, `pkpoker-mistral`, `pkpoker-qwen`) used as
PokerBench-guided seats. See EPIC-43 and EPIC-44 in [`docs/`](./docs).

---

## 5. Algorithms Implemented

Decision and detection logic built directly into this workspace. Poker
evaluation and the rule-based deciders come from `pkcore`; the entries below are
what pkdealer adds.

| Algorithm | Crate | Status | EPIC |
|-----------|-------|--------|------|
| LLM prompt/parse decision loop | `pkdealer_agent_llm` | Complete | EPIC-23 |
| Rule-based agent (wraps `pkcore::BotProfile`) | `pkdealer_agent_rules` | Complete | EPIC-23 |
| Random baseline agent | `pkdealer_agent_random` | Complete | EPIC-23 |
| Wald SPRT collusion detector over redacted hands | `pkdealer_boss` | Complete | EPIC-70 |
| Card-aware ground-truth scorer (EV-sacrifice oracle) | `pkdealer_boss::scorer` | Complete | EPIC-70 |
| Live redact-at-ingest detection loop | `pkdealer_agent_boss` | Authored, not live-validated | EPIC-70 |
| Vector-B collusion backchannel (adversary simulation) | `pkdealer_backchannel` | Complete | EPIC-70 |
| Token accounting and cost simulation | `pkdealer_costsim` | Complete | EPIC-44 |

**Blindness by type:** the detection pipeline accepts only `RedactedHand`, a
type with no field able to hold a hole card. "The Boss cannot peek" is enforced
by the compiler, not by convention.

---

## 6. Cost Accounting

Every LLM decision records an `AgentFidelity` provenance record (from
`pkcore::hand_history`) carrying the backend, model id, and input/output token
counts. Offline, [`pkdealer_costsim`](./crates/pkdealer_costsim) joins those
counts against [`pricing.toml`](./pricing.toml) via the shared
[`pkdealer_pricing`](./crates/pkdealer_pricing) crate to answer "what would this
recorded session have cost on a commercial API?".

Local Ollama play has no real dollar cost; those rates are a notional overlay.
Rates drift — re-verify against provider pricing pages.

---

## 7. Observability

The service and agents are OpenTelemetry-instrumented. LLM decision spans carry
the `gen_ai.*` semantic conventions alongside poker-domain attributes:

```
gen_ai.system              → "anthropic" | "ollama"
gen_ai.request.model       → "claude-sonnet-4-6" | "llama3.1" | ...
gen_ai.usage.input_tokens
gen_ai.usage.output_tokens
poker.street               → "preflop" | "flop" | "turn" | "river"
poker.pot
poker.pot_odds
poker.action_chosen        → "fold" | "call" | "raise" | "all-in"
```

No vendor SDK required — everything is exported over OTLP. `ops/` brings up
collector + Jaeger + Prometheus + Grafana; set `OTEL_SDK_DISABLED=true` to run
without a collector. See [`.okf/runbooks/observability.md`](./.okf/runbooks/observability.md).

---

## 8. References

| Document | Purpose |
|----------|---------|
| [`.okf/index.md`](./.okf/index.md) | Knowledge-bundle entry point |
| [`docs/BACKLOG.md`](./docs/BACKLOG.md) | Outstanding work |
| [`docs/EPIC-23_Bot_Agents-CLOSED.md`](./docs/EPIC-23_Bot_Agents-CLOSED.md) | LLM agent clients |
| [`docs/EPIC-43_PokerBench_Integration.md`](./docs/EPIC-43_PokerBench_Integration.md) | PokerBench-guided local models |
| [`docs/EPIC-44_Token_Accounting_and_Cost_Simulation-INC.md`](./docs/EPIC-44_Token_Accounting_and_Cost_Simulation-INC.md) | Token accounting and cost simulation |
| [`docs/EPIC-70_Collusion_and_Cheat_Detection.md`](./docs/EPIC-70_Collusion_and_Cheat_Detection.md) | The Boss, backchannel, and detection scoring |
| [`docs/GUIDE_Bot_Decision_Capabilities.md`](./docs/GUIDE_Bot_Decision_Capabilities.md) | What each agent type can and cannot see |
