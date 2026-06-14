# EPIC-43: PokerBench Integration

**Companion spec:** [`docs/EPIC-43_pkcore_PokerBench_spec.md`](EPIC-43_pkcore_PokerBench_spec.md)
(pkcore-side scenario model + scoring).
**Target repo:** `ImperialBower/pkdealer` (arena wiring) + `ImperialBower/pkcore`
(poker-domain library — see companion spec).
**Touches:** `crates/pkdealer_agent_llm`, `crates/pkdealer_agent_core`, a new
`crates/pkdealer_pokerbench`, `arena.toml`, EPIC-25 export, EPIC-41 scenarios.

## Status

| Phase / Component | Status |
|---|---|
| Phase 1 — pkcore `PokerBenchScenario` model + loaders (see companion spec) | ◻️ Planned — committed |
| Phase 1 — pkcore `score_action` (accuracy + size/EV-loss) | ◻️ Planned — committed |
| Phase 2 — pkdealer offline eval harness (`crates/pkdealer_pokerbench`) | ◻️ Planned — committed |
| Phase 2 — per-model leaderboard report (reuses EPIC-25 export) | ◻️ Planned — committed |
| Phase 3 — PokerBench prompt alignment + few-shot injection | 🔒 Gated — design only |
| Phase 4 — fine-tune a local Ollama model on the train split | 🌟 Stretch — exploratory |
| Phase 5 — live GTO-deviation annotation during arena hands | 🌟 Stretch — exploratory |

> **Phases 1–2 are the committed foundation** (an offline benchmark that scores the
> arena's existing LLM backends against solver-optimal labels). Phases 3–5 are
> documented as integration vectors but **gated** behind sign-off on the open
> decisions below — Phase 3 changes live play, and Phases 4–5 add training/indexing
> cost and a dataset-license dependency.

---

## Context

The arena's LLM players — Claude via `pkdealer_agent_claude` and local models
(llama, mistral, gemma) via `pkdealer_agent_ollama` — play poker with **no measured
skill baseline**. We cannot currently say "claude-sonnet plays at X% of solver
accuracy" or rank two local models by decision quality. The EPIC-25 recorder
captures *what* each agent did and the EPIC-40 `AgentFidelity` captures *how* it was
produced, but nothing scores those actions against a ground truth.

[PokerBench](https://github.com/pokerllm/pokerbench) (AAAI 2025; HuggingFace dataset
`RZ412/PokerBench`) is the missing yardstick. It is **No-Limit Texas Hold'em,
6-handed**, with each item an instruction (natural-language game state, position-
centric: UTG/HJ/CO/BTN/SB/BB) paired with a **solver-optimal action** (fold / check
/ call, or bet/raise with a specific size). Splits:

| Split | Train | Test |
|---|---|---|
| Pre-flop | 60,000 | 1,000 |
| Post-flop | 500,000 | 10,000 |

Available as JSON (prompt + label) and CSV (structured: action sequences, pot
sizes, available moves, hole cards). This makes it usable three ways — as a
**benchmark** (test split), as **few-shot exemplars** (train split), and as a
**fine-tuning corpus** (train split).

This epic explores **five integration vectors**, mapped to phases below:

1. **Benchmark** the arena's LLM backends offline (Phase 2).
2. **Align** the arena prompt with PokerBench's instruction format (Phase 3).
3. **Few-shot** inject train-split exemplars into the live arena prompt (Phase 3).
4. **Fine-tune** a local model on the train split, then play it in the arena (Phase 4).
5. **Live deviation**: annotate each arena LLM action with the nearest solver-
   optimal action for real-time GTO-distance tracking (Phase 5).

The poker-domain pieces (scenario model, action normalization, scoring against
pkcore's equity/GTO machinery) live in **pkcore** as reusable library code; the
arena wiring (harness, leaderboard, prompt work, model registration) lives here.
This mirrors the EPIC-25 Phase-4 split, where pkdealer drove a companion pkcore
spec ([`EPIC-25_Phase4_pkcore_AgentFidelity_spec.md`](EPIC-25_Phase4_pkcore_AgentFidelity_spec.md)).

**Success criteria (Phases 1–2):** running one command produces a leaderboard
table — per model, on each split — of action accuracy, size/EV-loss, coercion rate,
and tokens-per-decision, computed by feeding PokerBench test scenarios through the
*same* `build_prompt` → `LlmBackend::complete` → `parse_action_opt` path the arena
uses live.

---

## Design

### Reuse, don't reinvent

The whole point of the harness is to measure **real arena behavior**, so it runs
scenarios through the existing pipeline rather than a parallel one:

- `pkdealer_agent_llm::build_prompt(&HandState)` — `crates/pkdealer_agent_llm/src/prompt.rs:36`
- `pkdealer_agent_llm::parse_action_opt(&str)` — `crates/pkdealer_agent_llm/src/parse.rs`
  (with the existing `fallback_decision` for unparseable output, surfaced as a
  coercion).
- `pkdealer_agent_llm::pot_odds(&HandState)` — already computed per decision
  (`prompt.rs:107`); reusable as a scoring covariate.
- `LlmBackend` trait + the `ClaudeBackend` / `OllamaBackend` impls — the harness
  iterates whichever backends are configured, exactly as the arena does.
- `pkdealer_agent_core::HandState` — `crates/pkdealer_agent_core/src/hand_state.rs:31` —
  the target type every scenario is converted into.
- `Decision` enum + `AgentFidelity` — `crates/pkdealer_agent_core/src/agent.rs` — the
  harness records the same fidelity it would in a live hand.

### Scenario → `HandState` conversion (the core mapping)

PokerBench items must become `HandState` values. Two structural mismatches to
resolve in the converter (Phase 1, pkcore side; consumed here):

1. **Position vs seat.** PokerBench is position-labeled (UTG/HJ/CO/BTN/SB/BB);
   `HandState` is seat-indexed (`seat: u8`, `stacks: Vec<(u8, String, u32)>`). The
   converter assigns canonical 6-max seats to positions and synthesizes player
   names from positions so `build_prompt`'s "Seat stacks" line is well-formed.
2. **Action history granularity.** `HandState.action_history` is **this-street
   only** ("Human-readable descriptions of actions taken this street", `hand_state.rs:51`),
   while a PokerBench post-flop instruction encodes the full multi-street line. The
   converter populates `action_history` with the current street's actions and folds
   the prior-street context into the board/pot/`to_call` snapshot — losing nothing
   the prompt actually renders. (Phase 3 may widen the prompt to carry full history.)

The converter is the only piece that parses PokerBench prose; everything downstream
works on `HandState` + the structured solver `label`.

### Harness data flow (Phase 2)

```
PokerBench test split (JSON/CSV)
  └─ pkcore: parse → PokerBenchScenario { state, optimal: Decision-like label }
       └─ pkcore: PokerBenchScenario → HandState
            └─ pkdealer harness: build_prompt(&HandState)
                 └─ LlmBackend::complete(prompt)        [claude / ollama models]
                      └─ parse_action_opt(text) | fallback_decision   → Decision (+ AgentFidelity)
                           └─ pkcore: score_action(Decision, label)   → ScoreRow
                                └─ aggregate per (model, split) → Leaderboard
                                     └─ EPIC-25 export (YAML/JSON) + printed table
```

### Metrics

Per `(model, split)` aggregate, reported separately for preflop and postflop
(PokerBench's own convention):

- **Action accuracy** — exact match of action *type* (fold/check/call/bet/raise).
- **Size error** — for bet/raise, normalized chip distance |predicted − label| (e.g.
  as a fraction of pot), since exact-size match is too strict.
- **EV-loss** *(optional, refine later)* — solver-equity delta via pkcore's equity
  machinery; start with accuracy + size error and layer EV-loss in once defined.
- **Coercion rate** — share of decisions where `parse_action_opt` returned `None`
  and `fallback_decision` fired (a quality signal already tracked by `AgentFidelity`).
- **Tokens / decision** — mean input/output tokens from the backend response.

### New crate

`crates/pkdealer_pokerbench` — a library + thin binary (preferred over a `bin/`
shell driver so it gets unit + doc tests per `CLAUDE.md`). Owns the harness loop,
backend iteration, aggregation, and report emission. Depends on
`pkdealer_agent_llm`, `pkdealer_agent_core`, and `pkcore` (≥ the version that ships
the companion spec).

---

## Work Items

### Phase 1 — pkcore (committed; full detail in companion spec)
1. `PokerBenchScenario` type + JSON and CSV loaders for both splits.
2. Solver `label` → canonical action representation.
3. `PokerBenchScenario → pkcore` canonical state (position→seat, history folding).
4. `score_action(predicted, label)` → accuracy + size error (+ EV-loss hook).
5. Tests/doctests per pkcore conventions; additive release + changelog.

### Phase 2 — pkdealer (committed)
6. Add `crates/pkdealer_pokerbench` to the workspace; pin the pkcore version.
7. Scenario→`HandState` adapter using the pkcore conversion from Phase 1.
8. Harness loop: iterate configured backends, run `build_prompt`/`complete`/
   `parse_action_opt`, capture `AgentFidelity`, call `score_action`.
9. Aggregation + `Leaderboard` type; emit via EPIC-25 export (YAML/JSON) + a
   printed table.
10. CLI: select split (preflop/postflop), sample size, model list, output path.
11. Unit + doc tests (mock `LlmBackend` for determinism); a tiny vendored sample
    fixture so tests need no network.

### Phase 3 — prompt alignment + few-shot *(gated)*
12. Add a PokerBench-format prompt mode to `build_prompt` (or a translation layer),
    selectable so live-arena and benchmark prompts can A/B.
13. Optional few-shot: retrieve K similar train-split exemplars → prepend to the
    prompt. Measure lift via EPIC-41 reproducible scenarios.

### Phase 4 — fine-tune local model *(stretch)*
14. SFT a local model on the train split (HF `huggingface-llm-trainer` / TRL → GGUF),
    register in `arena.toml`, re-run the Phase-2 harness to quantify lift.

### Phase 5 — live GTO-deviation annotation *(stretch)*
15. Index the dataset for nearest-scenario lookup; during live hands annotate each
    LLM action with the nearest solver-optimal action as an `AgentFidelity`-style
    field surfaced through the EPIC-25 recorder.

---

## Configuration

| Variable | Default | Purpose |
|---|---|---|
| `POKERBENCH_DATA_DIR` | `./data/pokerbench` | Local dataset location (downloaded via `hf`). |
| `POKERBENCH_SPLIT` | `preflop` | `preflop` \| `postflop`. |
| `POKERBENCH_SAMPLE` | `0` (all) | Cap scenarios for a fast run. |
| `POKERBENCH_MODELS` | from `arena.toml` | Comma-list of backends to score. |
| `ANTHROPIC_API_KEY` / `OLLAMA_HOST` | — | Reuse the existing backend env (EPIC-40/42). |

Dataset acquisition: `hf download RZ412/PokerBench --repo-type dataset` into
`POKERBENCH_DATA_DIR`. A tiny sample is vendored under the new crate's test fixtures
for offline CI; the full set is **not** committed.

---

## Verification

- `cargo test -p pkdealer_pokerbench` (+ `--doc`) passes using the vendored sample
  and a mock `LlmBackend` — no network required.
- A scenario whose `label` is known scores as a hit through the real
  `build_prompt`/`parse_action_opt` path (a fixed-output mock backend).
- End-to-end smoke: `POKERBENCH_SAMPLE=20 cargo run -p pkdealer_pokerbench --
  --split preflop --models llama` against a local `ollama serve` prints a
  leaderboard row and writes a YAML report that round-trips via the EPIC-25 exporter.
- Manual: compare two local models (`llama` vs `mistral`) on the 20-scenario sample
  and confirm distinct accuracy/size-error/token numbers.

---

## Rollout phases

1. **Phase 1** — pkcore scenario model + scoring (additive release). *Committed.*
2. **Phase 2** — pkdealer offline harness + leaderboard. *Committed.*
3. **Phase 3** — prompt alignment + few-shot, A/B'd via EPIC-41. *Gated.*
4. **Phase 4** — fine-tuned local model in the arena. *Stretch.*
5. **Phase 5** — live GTO-deviation annotation via the EPIC-25 recorder. *Stretch.*

---

## Open decisions for sign-off

| # | Decision | Recommendation |
|---|---|---|
| 1 | pkcore version that ships Phase 1, and the pkdealer pin bump | Additive minor release, like the EPIC-40 `AgentFidelity` ship; bump the pin in lockstep. |
| 2 | Dataset acquisition & license | `hf` download at eval time + tiny vendored sample for CI. `RZ412/PokerBench`'s license is **unstated upstream** — confirm terms **before** any Phase-4 training use. |
| 3 | "EV-loss" metric definition | Start with action accuracy + pot-normalized size error; add solver-equity EV-loss once defined, behind a flag. |
| 4 | Harness as crate vs `bin/` shell driver | New `crates/pkdealer_pokerbench` crate, to satisfy the unit/doc-test requirements in `CLAUDE.md`. |
| 5 | Prompt strategy for fair benchmarking (Phase 3 dependency) | Benchmark the **current** arena prompt first (Phase 2) to get a true baseline; only then A/B a PokerBench-aligned prompt. |
