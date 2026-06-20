# EPIC-44: Token Accounting & External Cost Simulation

## Status

| Component | Status |
|---|---|
| Token capture (`prompt_eval_count`/`eval_count` → `LlmResponse`) | ✅ Done (EPIC-40) |
| `AgentFidelity` carries `input_tokens`/`output_tokens`/`model` | ✅ Done (EPIC-25 P4) |
| Per-action persistence into `HandCollection` (`proto_agent_to_pkcore` → `write_collection_yaml`) | ✅ Done (EPIC-25) |
| Post-hoc cost analysis pass over `HandCollection` | ✅ Done — Phase 0 (`crates/pkdealer_costsim`) |
| Per-seat live token accumulator on `TableState` (`session_tokens`) | ✅ Done — Phase 1 |
| `SeatInfo` token fields (`input_tokens`/`output_tokens`) + `build_table_status` wiring | ✅ Done — Phase 1 |
| OTel gauges (`pkdealer.player.tokens_in`/`tokens_out`) + hand-end token debug log | ✅ Done — Phase 1 |
| Live token column (pkspectator / pktui render) | 📋 Planned — Phase 1 (external pktui repo) |
| `pricing.toml` + notional cost computation | 📋 Planned — Phase 2 |
| Live cost column (notional USD) | 📋 Planned — Phase 2 |
| Tokenizer-variance correction (reconstruct via `build_prompt`, re-tokenize) | 📋 Planned — Phase 3 |
| Cached-input / batch / reasoning-token modeling | ⏸ Stretch — Phase 4 |

> **The hard 80% already shipped.** EPIC-40 made the Ollama backend read
> `prompt_eval_count`/`eval_count`; EPIC-25 Phase 4 carries those through
> `AgentFidelity` onto the `PlayerAction.agent` proto and folds them into the
> recorded `HandCollection`. Every LLM decision already records its token usage
> end-to-end. **This epic adds only aggregation, display, and a notional-cost
> overlay** — no new capture path. Phase 0 needs *zero* changes to the live
> arena because it consumes the recorded YAML; it can ship first and standalone.

---

## Context

`pkdealer` runs mixed arenas — rule bots plus local Ollama LLMs. Token usage is
captured per decision and persisted, but three things are missing:

1. **No per-seat aggregation.** Tokens exist per *action* in the hand log; nothing
   sums them by seat over a session.
2. **No display.** The `TableStatus` / `SeatInfo` snapshot that spectators render
   carries `chips`, `profit_loss`, `bet`, etc., but no token totals.
3. **No cost figure.** Local Ollama has *no dollar cost* — "spend" here is a
   token *count*. The interesting question is a **simulation**: what would this
   exact session have cost on a commercial API, and how do different notional
   providers compare head-to-head?

The goal is a per-seat **token column** and an optional **notional-cost column**,
derivable two ways: live (extend the session accumulator) or post-hoc (a pass
over the recorded `HandCollection`). Because cost is a pure function of
`(model, input_tokens, output_tokens)` — all already in the hand log — the same
recorded session can be re-priced under several pricing scenarios without
replaying a hand.

**Decisions:**

- **Post-hoc first (Phase 0).** A standalone analysis binary over the EPIC-25
  YAML sink delivers value immediately with no risk to the live service, and
  supports re-pricing one session under many scenarios. Live columns (Phases 1–2)
  come after.
- **Mirror the profit accumulator.** `SessionState` already keeps
  `banked_profit: HashMap<u8, i64>` plus an OTel `player_profit_loss` gauge. The
  token accumulator copies that shape exactly: `session_tokens: HashMap<u8,
  (u64, u64)>` (input, output), incremented in the same `Act` path where
  `proto_agent_to_pkcore` already receives `p.input_tokens`/`p.output_tokens`.
- **`model` is the join key.** `AgentFidelity.model` is set from the arena.toml
  `model` string (EPIC-42), so cost lookup is a straight join into `pricing.toml`
  keyed by model id. The *notional* model can be overridden independently of the
  model that actually ran (every seat is local Ollama, so "price seat 0 as
  GPT-5.5" is a config choice, not a code change).
- **Notional ≠ actual.** Pricing is decoupled from the real backend. A
  `--price-as 'gemma=claude-opus-4-8'` style override maps a seat/model to any
  entry in `pricing.toml`.
- **Cost as integer micro-USD in proto.** `uint64 cost_micro_usd` (1e-6 USD)
  keeps the wire format integer-exact, consistent with the existing
  integer-typed `SeatInfo` fields; renderers divide by 1e6 for display.

---

## Architecture & Phases

### Phase 0 — Post-hoc cost analysis pass (zero live changes)

A new workspace binary that reads a recorded `HandCollection` and a `pricing.toml`,
joins each action's `AgentFidelity` by seat, and prints a costed leaderboard.
Pure consumer of EPIC-25 output; **the live service is untouched.**

New crate: `crates/pkdealer_costsim`

```rust
// Reads pkcore::hand_history::HandCollection (the EPIC-25 YAML sink).
// For each recorded action with an AgentFidelity, accumulate by seat:
//   (input_tokens, output_tokens) and notional USD.

struct Price { input: f64, output: f64 }            // USD per 1M tokens

fn cost_usd(p: &Price, in_tok: u64, out_tok: u64) -> f64 {
    (in_tok as f64 / 1e6) * p.input + (out_tok as f64 / 1e6) * p.output
}

// Per-seat rollup keyed by the notional model (default: AgentFidelity.model,
// overridable via --price-as). Bots have AgentFidelity == default (all None)
// and contribute zero tokens / zero cost — rendered blank.
```

CLI:

```
pkdealer_costsim <hand_collection.yaml> [--pricing pricing.toml]
                 [--price-as <model>=<notional_model> ...]
                 [--scenario all-haiku | all-opus | mixed | <name>]
```

Output — a per-seat table (tokens in / out, notional USD), plus a session total.
Because it is a pure overlay, the *same* file can be re-run under several
scenarios to compare provider economics on identical play.

> **Why this is the cheapest win:** capture and persistence are done, so Phase 0
> is just a join + a sum + a table. It also doubles as the offline benchmark
> cost tool for PokerBench-style runs.

### Phase 1 — Per-seat token accumulator + live token column

Add live aggregation to the service, mirroring the `banked_profit` pattern.

**`SessionState`** (`crates/pkdealer_service/src/main.rs`, near `banked_profit`):

```rust
/// Per-seat cumulative LLM token usage (input, output) over the session.
/// Mirrors `banked_profit`; bots never increment it. Surfaced on SeatInfo.
session_tokens: std::collections::HashMap<u8, (u64, u64)>,
```

Increment in the `Act` handler at the existing fold point — the same place
`proto_agent_to_pkcore` already reads `p.input_tokens`/`p.output_tokens` and the
acting seat is in scope:

```rust
if let (Some(i), Some(o)) = (p.input_tokens, p.output_tokens) {
    let e = state.session_tokens.entry(seat).or_default();
    e.0 += u64::from(i);
    e.1 += u64::from(o);
}
```

**Proto** — extend `SeatInfo` (next free field is 10; `bet = 9` is the last):

```protobuf
message SeatInfo {
  // ... existing fields 1–9 ...
  uint64 input_tokens   = 10;  // cumulative prompt tokens this session (LLM seats)
  uint64 output_tokens  = 11;  // cumulative completion tokens this session
}
```

Populate in `build_table_status` (the `seats.push(SeatInfo { .. })` site):

```rust
let (in_tok, out_tok) = state.session_tokens.get(&i).copied().unwrap_or((0, 0));
seats.push(SeatInfo { /* … */ input_tokens: in_tok, output_tokens: out_tok });
```

Optional OTel gauges mirroring `pkdealer.player.profit_loss`:
`pkdealer.player.tokens_in` / `pkdealer.player.tokens_out`.

Render the column wherever `SeatInfo` is displayed (pkspectator / pktui). Bots
show blank/zero (correct — their fidelity is all `None`).

### Phase 2 — Live cost column (notional USD)

Layer the pricing overlay onto the live accumulator.

**`pricing.toml`** at repo root (keyed by model id; same id space as arena.toml):

```toml
# pricing.toml — USD per million tokens. Verify against provider pages; rates drift.
[models."gpt-5.5"]           = { input = 5.00, output = 30.00 }
[models."gpt-4.1"]           = { input = 2.00, output = 8.00 }
[models."gpt-4.1-mini"]      = { input = 0.40, output = 1.60 }
[models."gpt-4.1-nano"]      = { input = 0.10, output = 0.40 }
[models."claude-opus-4-8"]   = { input = 5.00, output = 25.00 }
[models."claude-sonnet-4-6"] = { input = 3.00, output = 15.00 }
[models."claude-haiku-4-5"]  = { input = 1.00, output = 5.00 }
[models."gemini-3.1-pro"]    = { input = 2.00, output = 12.00 }
[models."deepseek-v3.2"]     = { input = 0.14, output = 0.28 }
```

- Load once at service start; key on `AgentFidelity.model` (default) or a
  seat-level notional override.
- Add `uint64 cost_micro_usd = 12;` to `SeatInfo`; compute in `build_table_status`
  from the per-seat token totals and the resolved price.
- Renderers divide by 1e6 for a `$0.0231`-style figure beside the token column.

Shared cost logic lives in `pkdealer_costsim` (Phase 0) and is reused by the
service so the post-hoc and live figures agree exactly.

### Phase 3 — Tokenizer-variance correction (exact per-model counts)

Ollama reports `prompt_eval_count` using the **local model's** tokenizer. Pricing
those counts at Claude/GPT rates carries a tokenizer-skew error — typically
~10–30% on the (dominant) input term, direction depending on the tokenizers. For
*relative* rankings across same-model seats the skew is a shared constant and
cancels; for *absolute* dollars it is a real bias.

Correction — re-tokenize against the target model, no prompt storage required:

1. The prompt is built deterministically from `HandState` by
   `pkdealer_agent_llm::prompt::build_prompt(state) -> String`.
2. `HandState` is recorded in the `HandCollection`, so each prompt can be
   **reconstructed offline** by replaying `build_prompt` over the recorded states.
3. Re-tokenize the reconstructed prompt with the *target* tokenizer:
   - **Claude** → Anthropic `count_tokens` endpoint (counts only, no generation).
   - **GPT** → `tiktoken` locally.
4. Output is constrained to a single action, so output-side skew is a handful of
   tokens — correction focuses on input.

Emit a corrected per-seat figure in `pkdealer_costsim` (`--exact` mode) so the
benchmark can report true per-model cost rather than Ollama-tokenizer-priced
estimates.

### Phase 4 — Pricing realism (stretch)

- **Cached-input modeling.** Poker decisioning is input-dominated (big prompt,
  one-token action), so prompt caching is the single largest lever — cached reads
  run ~0.1x base input on Claude. Add `cached_input` rates and a per-model
  `cached_fraction` knob; this can swing simulated spend several-fold.
- **Batch discount (50%).** Not for live play, but relevant when costing offline
  benchmark runs.
- **Reasoning-token multiplier.** Reasoning models bill hidden reasoning as
  output; local Ollama produces none, so this can't be derived from observed
  tokens — model it as an output multiplier per reasoning model.

---

## Files to create / modify

| File | Action |
|---|---|
| `crates/pkdealer_costsim/Cargo.toml` | Create — analysis binary |
| `crates/pkdealer_costsim/src/main.rs` | Create — CLI: read `HandCollection`, join, roll up |
| `crates/pkdealer_costsim/src/pricing.rs` | Create — `Price`, `cost_usd`, `pricing.toml` loader (shared with service) |
| `pricing.toml` | Create — notional per-MTok rate table |
| `Cargo.toml` (workspace) | Modify — add `pkdealer_costsim` to members |
| `crates/pkdealer_proto/proto/*.proto` | Modify — add `input_tokens`/`output_tokens`/`cost_micro_usd` to `SeatInfo` (fields 10–12) |
| `crates/pkdealer_service/src/main.rs` | Modify — `session_tokens` field, `Act` increment, `build_table_status` population, optional OTel gauges, pricing load |
| pkspectator / pktui renderer | Modify — token (+ cost) column from `SeatInfo` |
| `crates/pkdealer_costsim/src/exact.rs` | Create (Phase 3) — reconstruct via `build_prompt`, re-tokenize |
| `DEMO.md` | Modify — document `pkdealer_costsim` and the live columns |

---

## Pricing reference (seed)

Standard per-million-token rates, **verified June 2026** (USD, input / output).
Rates change; treat `pricing.toml` as the source of truth and re-verify against
provider pricing pages.

| Model | Input | Output |
|---|---|---|
| OpenAI GPT-5.5 | $5.00 | $30.00 |
| OpenAI GPT-4.1 | $2.00 | $8.00 |
| OpenAI GPT-4.1 mini | $0.40 | $1.60 |
| OpenAI GPT-4.1 nano | $0.10 | $0.40 |
| Claude Opus 4.8 | $5.00 | $25.00 |
| Claude Sonnet 4.6 | $3.00 | $15.00 |
| Claude Haiku 4.5 | $1.00 | $5.00 |
| Gemini 3.1 Pro | $2.00 | $12.00 |
| DeepSeek V3.2 | $0.14 | $0.28 |

Cache discounts (Phase 4): Claude cached input reads ≈ 0.1× base; OpenAI / batch
≈ 50% off — material given the input-dominated workload.

---

## Example sessions

```bash
# Phase 0 — cost a recorded session at face-value model assignment:
pkdealer_costsim recordings/session_2026-06-19.yaml

# Re-price the SAME session three ways to compare provider economics:
pkdealer_costsim session.yaml --scenario all-haiku
pkdealer_costsim session.yaml --scenario all-opus
pkdealer_costsim session.yaml --price-as 'gemma=claude-opus-4-8' \
                              --price-as 'llama=deepseek-v3.2'

# Phase 3 — exact per-model counts (re-tokenize against the target):
pkdealer_costsim session.yaml --exact --price-as 'gemma=claude-sonnet-4-6'

# Phases 1–2 — live: token + notional-cost columns appear in pkspectator/pktui
# automatically once SeatInfo carries the fields.
./bin/arena gto lag llama gemma claude
```

---

## Verification

1. **Phase 0 rollup:** running `pkdealer_costsim` on a fixture `HandCollection`
   with known per-action tokens produces the expected per-seat input/output
   totals and USD, and bots show zero.
2. **Scenario re-pricing:** the same fixture under `--scenario all-haiku` vs
   `all-opus` yields costs in the exact 5× ratio implied by `pricing.toml`.
3. **Join key:** an action whose `AgentFidelity.model` is absent from
   `pricing.toml` is reported (not silently zero) and skipped from cost (counted
   in tokens).
4. **Accumulator parity:** for a scripted session, the live `session_tokens`
   totals equal the Phase 0 post-hoc totals for the same hands (shared pricing
   module guarantees identical USD).
5. **Proto round-trip:** `SeatInfo` with `input_tokens`/`output_tokens`/
   `cost_micro_usd` encodes/decodes; `build_table_status` populates them; a
   spectator snapshot renders the column.
6. **Bot blankness:** rule/random seats render blank token + cost cells
   (`AgentFidelity::default()` → no contribution).
7. **Phase 3 correction:** for a reconstructed prompt, `build_prompt(&state)`
   reproduces the recorded input text byte-for-byte, and the re-tokenized count
   differs from Ollama's `prompt_eval_count` (skew demonstrated, not assumed).
8. **Workspace gates:** `OTEL_SDK_DISABLED=true cargo test --workspace` passes;
   `cargo clippy --workspace --all-targets -- -D warnings` is clean.
9. Every new public fn/struct gets a doc comment with `# Examples`, a doctest,
   and unit tests (happy path + edge + error) per `CLAUDE.md`;
   `cargo test --doc -p pkdealer_costsim` passes.
