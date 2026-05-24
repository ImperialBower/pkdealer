# Demo: EPIC-23 — Bot Agent Clients

> Three independent bot binaries — random, rules-based, and Claude — sit down at one table over gRPC and play continuous No-Limit Hold'em with no human in the loop.

## Audience & framing

Engineering peers or a technical lead reviewing the EPIC-23 deliverable end-to-end. The angle is **one trait, three strategies**: the same `PokerAgent` interface powers a 20-line random bot, a YAML-driven rules engine, and a remote LLM, with progressively richer observability for each.

Deeper sub-demos already exist if the audience wants to drill in:
- `docs/presentations/epic-23a-random-bot-agents.md` — random agent + transport mechanics
- `docs/presentations/epic-23-claude-agent.md` — Claude agent + `gen_ai.*` Jaeger spans

---

## Prerequisites

- Repo checked out on branch `epic-23b` (which bundles 23a/23b/23c — see naming note below)
- Rust toolchain present (`cargo --version`)
- Docker running (for the OTel stack — Jaeger is the visual payoff)
- `ANTHROPIC_API_KEY` exported in the shell that will run the Claude agent
- Five terminal panes: service | random bot | rules bot | claude bot | scratch (for Jaeger / commands)
- Browser tab ready for `http://localhost:16686`
- No other process listening on ports 50051, 9090, 3001, 16686, 4317

> Naming note: the EPIC doc calls the sub-features 23a (random), 23b (rules), 23c (Claude). The active branch is `epic-23b` but contains all three. The runbook treats EPIC-23 as the whole deliverable.

---

## Setup (~ 5 minutes)

1. **Build the four binaries the demo touches**
   ```bash
   cargo build --bin pkdealer_service \
               --bin pkdealer_agent_random \
               --bin pkdealer_agent_rules \
               --bin pkdealer_agent_claude
   ```
   _Expected:_ `Finished` with no errors.
   _Talking point:_ One workspace, four binaries — three of those agents differ only in their `decide()` implementation.

2. **Bring up the OTel stack**
   ```bash
   docker compose up -d otel-collector jaeger prometheus grafana
   ```
   _Expected:_ Four containers running.
   _Talking point:_ Same EPIC-22 stack; agents export OTLP gRPC to the collector on `localhost:4317`.

3. **Confirm Jaeger is reachable**
   ```bash
   open http://localhost:16686
   ```
   _Expected:_ Jaeger UI loads; the service dropdown is empty until agents emit spans.
   _Talking point:_ This is where Claude's decisions will appear as `llm.decision` spans later.

4. **Confirm the API key is set**
   ```bash
   echo "ANTHROPIC_API_KEY length: ${#ANTHROPIC_API_KEY}"
   ```
   _Expected:_ A non-zero length (don't echo the key itself).
   _Talking point:_ The Claude agent exits immediately if the key is missing — fail fast.

5. **Glance at the available bot profiles**
   ```bash
   ls data/bots/
   ```
   _Expected:_ Eight YAMLs — `abc`, `gto`, `loose_aggressive`, `loose_passive`, `maniac`, `short_stack_ninja`, `tight_aggressive`, `tight_passive`.
   _Talking point:_ The rules agent picks a personality from this directory at launch.

---

## The demo (~ 10 minutes)

### 1. Start the service

In **pane 1**:
```bash
cargo run --bin pkdealer_service
```
_Expected:_ `Dealer service listening on 0.0.0.0:50051`
_Talking point:_ Same autonomous-loop service from EPIC-20 — it deals as soon as two players are seated.

> Always start from a fresh service process. Ghost players from previous runs will occupy lower seats and stall post-flop action.

---

### 2. Seat the random agent

In **pane 2**:
```bash
cargo run --bin pkdealer_agent_random -- --name rando --chips 10000
```
_Expected:_ Service log shows `rando` seated at seat 0; no hands yet.
_Talking point:_ Random agent — uniform legal moves. This is the baseline every other agent is measured against.

---

### 3. Seat the rules agent — hands start

In **pane 3**:
```bash
cargo run --bin pkdealer_agent_rules -- --name gto --profile gto --chips 10000
```
_Expected:_ Service log shows `gto` seated at seat 1, then hands begin dealing.
_Talking point:_ The rules agent loaded `data/bots/gto.yaml`; decisions now come from `pkcore::RuleBasedDecider` rather than dice.

Let two or three hands play. Point at pane 3 for the rules log, pane 1 for the service summary.

---

### 4. Add the Claude agent to the same table

In **pane 4**:
```bash
ANTHROPIC_API_KEY="$ANTHROPIC_API_KEY" \
cargo run --bin pkdealer_agent_claude -- \
  --name claude --chips 10000 --max-tokens 16
```
_Expected:_ `[claude] model=claude-sonnet-4-6 max_tokens=16`, then per-decision lines like `[claude] preflop → "raise 300" → Raise(300)  (in=187 out=4)`.
_Talking point:_ Three independent processes, one table — the service can't tell them apart.

---

### 5. Watch a three-handed hand resolve (point at pane 1)

Let one full hand play out across all three agents.

_Expected:_ Three decision streams interleave; the service emits `HandStarted`, per-action events, then `HandEnded` with the chip delta.
_Talking point:_ Chip conservation across all three — sum of stacks stays at 30 000 throughout.

---

### 6. Swap the rules agent's personality (point at pane 3)

Ctrl-C the rules agent in pane 3, then:
```bash
cargo run --bin pkdealer_agent_rules -- --name gto --profile maniac --chips 10000 --client-secret <token-from-startup-log>
```

(If seat resume isn't set up, just re-seat under a new name: `--name maniac` with no `--client-secret`.)

_Expected:_ Same binary, very different play — more bets, more raises, more bluffs.
_Talking point:_ One YAML swap turns a tight-aggressive bot into a maniac. The `decide()` code path didn't change.

---

### 7. Inspect Claude's decisions in Jaeger (pane 5 / browser)

In the Jaeger UI:
- **Service:** `pkdealer_agent_claude`
- **Operation:** `llm.decision`
- Click **Find Traces**, open the most recent one.

_Expected:_ A span with `gen_ai.system = "anthropic"`, `gen_ai.request.model`, `gen_ai.usage.input_tokens` / `output_tokens`, `poker.street`, `poker.pot_odds`, `poker.action_chosen`.
_Talking point:_ Standard OTel GenAI semantic conventions — any GenAI-aware dashboard reads them without configuration.

---

### 8. Show the Claude fallback path (optional, ~30s)

Ctrl-C the Claude agent. Re-run with a junk key:
```bash
ANTHROPIC_API_KEY=sk-bogus cargo run --bin pkdealer_agent_claude -- --name claude
```
_Expected:_ `[claude] API error (preflop): Anthropic API 401: ...`; the agent still acts (Check/Fold) and the table keeps going.
_Talking point:_ A remote model failing should never lock the table — the trace captures the error, gameplay continues.

---

## What to highlight verbally

- **`PokerAgent::decide` is the only thing each agent customises.** Connect, seat, event-loop, gRPC, chip accounting — all shared. The random agent is ~20 lines on top of that surface; Claude is ~80.
- **Three flavours of decision-making on identical telemetry.** The random agent has none, the rules agent emits standard tracing spans, Claude adds `gen_ai.*` SemConv attributes. All three drop into the same Jaeger.
- **YAML profiles let non-engineers tune the rules bot.** `data/bots/*.yaml` is the surface area for opening a personality knob to product or product-research without touching Rust.
- **Trace context flows from the agent into the service.** A Claude `llm.decision` span becomes the parent of the service's `act` span — one trace covers prompt → response → server resolution.
- **EPIC-23 unblocks EPIC-24.** The follow-on demo EPIC depends on having credible non-human opponents that play full hands without supervision; that infrastructure landed here.

---

## Likely questions & answers

**Q: Why three agents instead of one configurable one?**
A: They have different blast radii. Random needs no dependencies; rules pulls in `pkcore`'s simulation crate; Claude pulls in `reqwest` and emits paid API calls. Keeping them as separate binaries means each can be deployed independently, with the dependencies it actually needs.

**Q: Can I run more than three agents at one table?**
A: Yes — the service supports up to nine seats (matching `--seat 0..=8`). Each agent is a separate process, so the only practical limit is local CPU and (for Claude) API rate limits.

**Q: How is hole-card visibility handled across agents?**
A: Each agent's `StreamEvents` request includes its own player token; the service redacts hole cards from any event going to a different seat. There's no shared state for an agent to "peek" at — the gRPC contract enforces it.

**Q: Why use `pkcore::RuleBasedDecider` instead of writing the rules in the agent crate?**
A: `pkcore` already runs that decider locally in `SimTable` for offline simulation. Reusing it ensures the bot plays the same way over gRPC as it does in a local sim — same input, same output.

**Q: What about the Langfuse scoring (23d)?**
A: Deferred. The OTel trace IDs are already exported, so when Langfuse scoring lands the per-decision quality dataset can be reconstructed retroactively from existing traces.

---

## Cleanup

```bash
# Ctrl-C in panes 1–4 (service + three agents)
docker compose down
unset ANTHROPIC_API_KEY   # only if it was set just for the demo
# Branch state is already epic-23b; no checkout needed.
```

---

## Troubleshooting

**Service starts, agents connect, but no hands deal after two are seated.**
Stale seats from a previous run. `pkill -f pkdealer_agent_` then restart the service. The service does not evict on agent disconnect.

**`ANTHROPIC_API_KEY is not set or empty` then exit 1.**
The key didn't make it into the Claude agent's environment. Either `export ANTHROPIC_API_KEY=sk-...` in the same shell or prefix the command inline.

**Rules agent panics with `Failed to load profile`.**
Either the short name was wrong (use `gto`, `tp`, `tag`, `lp`, `lag`, `maniac`, `ssn`, `abc`) or the YAML path doesn't exist. Confirm with `ls data/bots/`.

**Jaeger UI lists `pkdealer_service` but not `pkdealer_agent_claude`.**
The Claude agent never reached an API call (likely seated but never had a decision yet, or failed silently on OTel init). Check pane 4 for an `OTel exporter init failed:` line and confirm the collector is healthy with `docker compose logs otel-collector`.

**`Anthropic API 401` on every Claude decision.**
Key invalid or revoked. The agent will keep playing via fallback (Check / Fold), but the demo loses its `gen_ai.usage` attribute story. Replace the key and restart.
