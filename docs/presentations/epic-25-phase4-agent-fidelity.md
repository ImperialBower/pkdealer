# Demo: EPIC-25 Phase 4 — Agent Fidelity

> Two AI bots play poker autonomously, and every recorded hand shows what each
> agent *wanted* to do versus what the table *actually applied* — model id, and
> every coercion — in the exported hand history.

## Audience & framing

Engineering / product. The angle: **provenance for AI-agent decisions**. Until
now a recorded hand was purely mechanical ("seat 0 raised 224"). Phase 4 adds an
optional, analysis-only `agent` block to each voluntary action that captures the
agent side of the story — raw model text, token usage, the model id, and the
parse / clamp / retry coercions — without changing replay. Pitch it as "git
blame for bot decisions."

Smoke-verified 2026-06-01: a 2-bot arena produced 348 agent blocks and 108
coercions (both clamp and retry paths) in ~15 seconds.

## Prerequisites

- Repo checked out at `ImperialBower/pkdealer`, branch with Phase 4 landed
  (proto `AgentFidelity`, service buffering/attach, agent population).
- `pkcore = 0.1.3` resolved (provides `attach_agent_fidelity`). `cargo tree -p pkcore`
  should show `0.1.3`.
- Rust toolchain per `rust-toolchain.toml`; no Docker, no LLM backend, no
  network needed for the core demo.
- Two terminal panes: **pane A** = service, **pane B** = bots + the YAML reveal.
  (A third pane is optional if you want bot logs separate from the reveal.)
- Scratch dir `/tmp/pkdealer-demo` is writable (the recorder writes here).

## Setup (~ 3 minutes)

1. **Clean the scratch dir and build the two binaries**
   ```bash
   rm -rf /tmp/pkdealer-demo && mkdir -p /tmp/pkdealer-demo
   cargo build -p pkdealer_service -p pkdealer_agent_random
   ```
   _Expected:_ `Finished \`dev\` profile` (~20s warm, a few minutes cold).
   _Talking point:_ "No LLM needed — random bots exercise every coercion path."

2. **Pane A — start the dealer with disk recording on**
   ```bash
   PKDEALER_ADDR=127.0.0.1:50051 \
   PKDEALER_RECORD_DIR=/tmp/pkdealer-demo \
   PKDEALER_SPECTATOR_TOKEN=spectator \
   OTEL_SDK_DISABLED=true \
   ./target/debug/pkdealer_service
   ```
   _Expected:_ `Starting gRPC server on 127.0.0.1:50051...`
   _Talking point:_ "`PKDEALER_RECORD_DIR` flushes the whole session to YAML after every hand."

## The demo (~ 6 minutes)

### 1. Seat two autonomous bots

In **pane B**, launch two random bots. They self-seat and race to start hands —
no orchestrator needed.

```bash
cd <repo root>
PKDEALER_ENDPOINT=http://127.0.0.1:50051 PKDEALER_ACTION_DELAY_SECS=1 \
  ./target/debug/pkdealer_agent_random --name alice &
PKDEALER_ENDPOINT=http://127.0.0.1:50051 PKDEALER_ACTION_DELAY_SECS=1 \
  ./target/debug/pkdealer_agent_random --name bob &
```
_Expected:_ each prints `[alice] seated at seat 0` / `[bob] seated at seat 1`, then `hand started` and a stream of decisions.
_Talking point:_ "Both bots call StartHand after every hand-end — that's the autonomous loop from EPIC-20."

### 2. Watch a coercion happen live

Point at pane B's bot output for a few hands.

```bash
# (just read the running bot logs)
```
_Expected:_ lines like `[alice] preflop ... → Raise(150)` and occasionally `act rejected (Insufficient increment Error) — falling back to Fold`.
_Talking point:_ "That rejection-and-fallback is the *retry* coercion — the runner records it, then unblocks the table."

### 3. The reveal — agent blocks in the recorded hand

Let it run ~15 seconds, then in pane B (or pane C) inspect the YAML.

```bash
F=$(ls -t /tmp/pkdealer-demo/session-*.yaml | head -1); echo "$F"
grep -c 'agent:' "$F"          # every voluntary action is annotated
grep -c 'was_coerced: true' "$F"
```
_Expected:_ hundreds of `agent:` blocks; a healthy fraction `was_coerced: true`.
_Talking point:_ "Every action carries a `model` id; ~1 in 3 was coerced."

### 4. Show intended-vs-applied on one action

```bash
grep -n -B4 -A3 'was_coerced: true' "$F" | head -24
```
_Expected:_ blocks such as —
```yaml
        action: raise
        amount: 224.0          # what the table APPLIED
        agent:
          was_coerced: true
          intended_action: raise
          intended_amount: 180.0   # what the bot WANTED
```
and the preflop-bet clamp:
```yaml
        action: check
        agent:
          was_coerced: true
          intended_action: bet
          intended_amount: 100.0
```
_Talking point:_ "Applied size stays in `amount`; the original intent lives in the `agent` block — both halves preserved."

### 5. Show a clean (un-coerced) action carries just the model id

```bash
grep -n -B4 -A2 'model: alice' "$F" | head -8
```
_Expected:_ an `action: call` / `agent: { model: alice }` with no coercion fields.
_Talking point:_ "Structured agents that aren't coerced record just who acted — no noise."

### 6. (Optional) LLM provenance — raw text + tokens

Only if Ollama is running on `:11434` (or swap in `pkdealer_agent_claude` with
`ANTHROPIC_API_KEY`). Replace one random bot with an LLM bot:

```bash
PKDEALER_ENDPOINT=http://127.0.0.1:50051 \
  ./target/debug/pkdealer_agent_ollama --name llm --model llama3.2
```
_Expected:_ that bot's actions gain `raw_response:`, `input_tokens:`, `output_tokens:` in the YAML.
_Talking point:_ "Same block — the LLM just fills more of it: the raw text and token cost behind each move."
_(Not exercised in the core smoke; LLM provenance fields are covered by the agent_llm unit tests and the service e2e test.)_

## What to highlight verbally

- **Analysis-only, replay-safe.** `HandHistory::replay()` ignores the `agent`
  block entirely — proven by a replay-invariance test. Recording provenance
  never changes game truth.
- **Three coercion sources, one flag.** Parse-fallback (LLM), legality clamp
  (sub-floor raise / illegal preflop bet), and rejection-retry all set
  `was_coerced` and preserve `intended_action`. The runner is where intent vs.
  applied is finally known.
- **Back-compatible by construction.** Absent metadata emits no `agent:` key —
  a hand with no agents is byte-identical to a pre-Phase-4 recording. Manual
  (human) hands stay clean; only agent acts annotate.
- **Strict positional alignment.** The service buffers one entry per applied
  voluntary action in arrival order, so a positional zip lands each block on the
  right action; a seat-checked guard and a drift warning catch any skew.
- **What it unlocks:** eval/training datasets that distinguish "the model chose
  this" from "the table forced this" — essential for judging agent quality.

## Likely questions & answers

- **Q: Does this slow the table or change outcomes?**
  A: No. It's recorded after the action is applied and ignored by replay; game
  logic never reads it.

- **Q: What if a bot has no model/LLM, like the random one?**
  A: The runner stamps the agent's `--name` as the `model` id, so every arena
  action still has provenance even without an LLM.

- **Q: Why is `amount` 224 but `intended_amount` 180?**
  A: The bot raised below the legal minimum; the runner clamped it up to stay
  valid and recorded the original 180 as the intent.

- **Q: How does it line the metadata up with the right action?**
  A: One buffered entry per successfully-applied voluntary action, in arrival
  order — exactly the order pkcore replays — zipped on with a seat check.

- **Q: Does a human at a mixed table get empty `agent: {}` blocks?**
  A: No — the hand-end hook strips empty placeholders, so agent-less actions
  stay clean.

## Cleanup

```bash
pkill -f 'target/debug/pkdealer_agent_random'
pkill -f 'target/debug/pkdealer_service'
rm -rf /tmp/pkdealer-demo            # optional: drop the recorded YAML
```
_Expected:_ `pgrep -fl 'pkdealer_service|pkdealer_agent_random'` prints nothing.
No branches or env vars were changed; only `/tmp/pkdealer-demo` was written.

## Troubleshooting

- **`act rejected (Insufficient increment Error) — falling back to Fold`** in bot
  logs is **expected**, not a failure — it's a random bot under-raising and the
  runner's retry coercion kicking in. It's the live source of `was_coerced: true`
  retry blocks; call it out as a feature.
- **No `session-*.yaml` appears.** A hand must fully complete (showdown or all
  fold) before the first flush. Give it ~10s; confirm both bots seated (need ≥2
  funded players for `StartHand` to succeed).
- **`Address already in use` on start.** A previous service is still bound to
  50051: `pkill -f target/debug/pkdealer_service` then restart, or change
  `PKDEALER_ADDR`.
- **Bots exit immediately.** Check `PKDEALER_ENDPOINT` matches the service
  `PKDEALER_ADDR` (`http://127.0.0.1:50051`).
