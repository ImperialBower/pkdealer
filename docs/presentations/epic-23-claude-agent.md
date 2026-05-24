# Demo: EPIC-23 — Claude LLM Agent (`pkdealer_agent_claude`)

> A bot that asks Claude what to do every street, then plays the action — with every decision visible as a `gen_ai.*` span in Jaeger.

## Audience & framing

Engineering peers or a technical lead reviewing EPIC-23 progress. The angle is **observability of an LLM in the loop**: the random/rules agents proved the transport works; this demo shows what changes when the decision-maker is a remote model and every call is fully traced.

> Note on naming: the user's invocation said "EPIC-23b", but `docs/EPIC-23_Bot_Agents.md` labels Claude as sub-EPIC **23c** (23a = random, 23b = rules). This branch (`epic-23b`) bundles both. The runbook below targets the Claude binary regardless.

---

## Prerequisites

- Repo checked out on branch `epic-23b`
- Rust toolchain present (`cargo --version`)
- `ANTHROPIC_API_KEY` exported in the shell that will run the Claude agent
- Docker running (for the OTel stack — Jaeger is where the payoff lives)
- Four terminal panes: service | random bot | claude bot | scratch (for Jaeger / cleanup)
- Browser tab ready for `http://localhost:16686`
- No other process listening on ports 50051, 9090, 3001, 16686, 4317

---

## Setup (~ 4 minutes)

1. **Build the three binaries the demo touches**
   ```bash
   cargo build --bin pkdealer_service --bin pkdealer_agent_random --bin pkdealer_agent_claude
   ```
   _Expected:_ `Finished` with no errors.
   _Talking point:_ Three binaries share one workspace; the Claude agent is ~740 lines on top of the shared runner.

2. **Bring up the OTel stack**
   ```bash
   docker compose up -d otel-collector jaeger prometheus grafana
   ```
   _Expected:_ Four containers running (`otel-collector`, `jaeger`, `prometheus`, `grafana`).
   _Talking point:_ Same EPIC-22 stack — the agent emits OTLP over gRPC to the collector on `localhost:4317`.

3. **Confirm Jaeger is reachable**
   ```bash
   open http://localhost:16686
   ```
   _Expected:_ Jaeger UI loads; service list is empty until the agent emits its first span.
   _Talking point:_ This is where the audience will see Claude's decisions appear.

4. **Confirm the API key is set**
   ```bash
   echo "ANTHROPIC_API_KEY length: ${#ANTHROPIC_API_KEY}"
   ```
   _Expected:_ A non-zero length (don't echo the key itself).
   _Talking point:_ The agent exits immediately if the key is missing — fail fast.

---

## The demo (~ 7 minutes)

### 1. Start the service

In **pane 1**:
```bash
cargo run --bin pkdealer_service
```
_Expected:_ `Dealer service listening on 0.0.0.0:50051`
_Talking point:_ Same autonomous-loop service from EPIC-20 — it deals as soon as a second player sits down.

> If the OTel stack containers were just started, give the collector ~5 seconds before the first agent connects so the OTLP gRPC endpoint is fully up.

---

### 2. Seat a random sparring partner

In **pane 2**:
```bash
cargo run --bin pkdealer_agent_random -- --name rando --chips 10000
```
_Expected:_ Service log shows `rando` seated; no hands yet (one player).
_Talking point:_ Random agent is the baseline — every Claude decision will be measured against an opponent making uniform-legal moves.

---

### 3. Seat the Claude agent — hands start immediately

In **pane 3**:
```bash
ANTHROPIC_API_KEY="$ANTHROPIC_API_KEY" \
cargo run --bin pkdealer_agent_claude -- \
  --name claude --chips 10000 --max-tokens 16
```
_Expected:_ One startup line `[claude] model=claude-sonnet-4-6 max_tokens=16`, then per-decision lines like `[claude] preflop → "raise 300" → Raise(300)  (in=187 out=4)`.
_Talking point:_ Each `→` line shows raw model text, the parsed `Decision`, and Anthropic's reported token counts.

---

### 4. Watch a hand play out (point at pane 3)

Let one full hand resolve. Each street produces one Claude line; the random agent answers immediately in pane 2.

_Expected:_ Decisions across `preflop`/`flop`/`turn`/`river`; pot conservation visible in pane 1's `HandEnded` log.
_Talking point:_ The transport, state reconstruction, and chip accounting are identical to the random agent — only `decide()` is different.

---

### 5. Inspect a decision span in Jaeger (pane 4 / browser)

In the Jaeger UI:
- **Service:** `pkdealer_agent_claude`
- **Operation:** `llm.decision`
- Click **Find Traces**, open the most recent one.

_Expected:_ A span named `llm.decision` with these attributes populated:
- `gen_ai.system = "anthropic"`
- `gen_ai.request.model = "claude-sonnet-4-6"`
- `gen_ai.request.max_tokens = 16`
- `gen_ai.usage.input_tokens` / `gen_ai.usage.output_tokens`
- `poker.street`, `poker.pot`, `poker.pot_odds`, `poker.action_chosen`

_Talking point:_ The `gen_ai.*` names are OpenTelemetry semantic conventions — any GenAI-aware dashboard reads them without configuration.

---

### 6. Show a deliberately bad model choice (optional, ~30s)

Kill the Claude agent (Ctrl-C in pane 3) and re-run with an obviously-too-small token budget:

```bash
cargo run --bin pkdealer_agent_claude -- --name claude --max-tokens 2
```
_Expected:_ Truncated/unrecognized responses → agent falls back to `Check` (or `Fold` facing a bet); service keeps running.
_Talking point:_ The parser has a safe fallback — no panic, no crash, the hand keeps moving.

---

### 7. Show the fallback path on API error (optional)

Kill the agent. Re-run with a junk key:
```bash
ANTHROPIC_API_KEY=sk-bogus cargo run --bin pkdealer_agent_claude -- --name claude
```
_Expected:_ `[claude] API error (preflop): Anthropic API 401: ...`; the agent still acts (Check/Fold) and the table continues.
_Talking point:_ A remote model failing should not lock the table — observability captures the error, gameplay continues.

---

## What to highlight verbally

- **The `decide()` method is the entire surface area.** Everything outside it — seating, event filtering, gRPC, chip accounting — is shared with the random and rules agents.
- **`gen_ai.*` is not custom telemetry.** It's the OTel SemConv namespace; the same attributes Anthropic, OpenAI, and Bedrock dashboards expect. We didn't invent a schema.
- **Pot odds are attached to the span, not just the prompt.** When you replay a decision in Jaeger, you can see what the *math* said even though the model only saw the prompt.
- **Fallback behaviour is deliberate.** An API timeout or a 1-token response can't break the table; the parser collapses anything unrecognized to the safest legal action.
- **Trace context is propagated into `Act`.** The action span on the service side is a child of `llm.decision`, so one trace covers prompt → response → server-side resolution.

---

## Likely questions & answers

**Q: How much does running this cost?**
A: With `--max-tokens 16` and Sonnet 4.6, an average hand is ~4 decisions × ~200 input + ~5 output tokens — fractions of a cent per hand. The token counts are on every span, so cost is auditable.

**Q: Why is the prompt so short — no hand history across hands?**
A: Intentional. Each `decide()` is stateless: the hand state struct carries everything Claude needs for *this* decision. Long-horizon strategy would require either fine-tuning or a memory layer, neither of which is in scope here.

**Q: What happens on a malformed response, like "I would raise to about 300 chips"?**
A: The parser only accepts canonical forms (`fold` / `check` / `call` / `bet N` / `raise N` / `raise to N` / `all in`). Anything else falls through to Check (no bet) or Fold (facing one). It's logged, so prompt-engineering regressions show up in the trace history.

**Q: Could we swap in a local model (Ollama, llama.cpp) instead?**
A: Yes — the only Anthropic-specific code is `call_api()`. Replace that with an OpenAI-compatible HTTP call and the rest of the agent is unchanged. The `gen_ai.system` attribute on the span makes the swap visible in dashboards.

**Q: Why `--max-tokens 16` by default?**
A: The expected response is one short action like `raise 300`. Sixteen tokens is enough headroom for that; lower budgets save cost but raise the fallback rate, which you can monitor via the span.

---

## Cleanup

```bash
# Ctrl-C in panes 1, 2, 3 (service + both agents)
docker compose down
unset ANTHROPIC_API_KEY    # optional — only if it was set just for the demo
# Branch state already on epic-23b; no checkout needed.
```

---

## Troubleshooting

**`ANTHROPIC_API_KEY is not set or empty` then exit 1.**
The key didn't make it into the agent's environment. Either export it in the same shell (`export ANTHROPIC_API_KEY=sk-...`) or prefix the command inline (`ANTHROPIC_API_KEY=sk-... cargo run ...`).

**Agent connects but no `llm.decision` spans appear in Jaeger.**
Two common causes: (1) `OTEL_SDK_DISABLED=true` was left set from a prior session — `unset OTEL_SDK_DISABLED`; (2) the collector wasn't ready when the agent started — restart the agent after confirming `docker ps` shows `otel-collector` healthy.

**`Anthropic API 401` on every decision.**
Key is invalid or revoked. Agent will keep playing via the fallback (Check/Fold). Replace the key and restart.

**Ghost players occupy seats 0/1, Claude seats at 2 and hands stall.**
A prior agent process didn't exit cleanly. `pkill -f pkdealer_agent_` then restart the service before re-seating.

**Jaeger UI lists no services.**
Refresh the search panel (Jaeger caches the service list briefly); if still empty after the agent has logged at least one decision, check `docker compose logs otel-collector` for export errors.
