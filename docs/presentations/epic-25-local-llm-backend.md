# Demo: EPIC-25 — Local-LLM Backend & Multi-Model Agents

> One Claude monolith became a shared `LlmBackend` trait plus two thin per-provider crates. The audience sees two LLM agents — Claude and Ollama — at the same table, each emitting its own `gen_ai.*` spans, with no copy-pasted poker code between them.

## Audience & framing

Engineering peers reviewing EPIC-25 progress. The angle is **the cost and shape of the abstraction**: EPIC-23 shipped one LLM agent as a 740-line monolith; EPIC-25 split it into a shared library + two backends, and the dividend is a free, local, no-API-key second agent that lights up the same OTel spans as Claude. The visible punchline is two `gen_ai.system` values side-by-side in Jaeger from the same hand.

> Verified during smoke: workspace builds clean, OTel stack + service container + Ollama daemon already up on this machine, `llama2:latest` returns parseable text with token counts populated. **`llama3.1` is not yet pulled** — pull it before the demo (see Prerequisites). The currently-running `pkdealer_service` Docker image is v0.1.7 while the workspace agents are v0.1.11; gRPC proto is unchanged so this works, but the version banner will look mismatched.

---

## Prerequisites

- Repo at `/Users/christoph/src/github.com/ImperialBower/pkdealer` on `main`. EPIC-25 code is uncommitted (untracked dirs `crates/pkdealer_agent_llm/`, `crates/pkdealer_agent_ollama/`, modified `crates/pkdealer_agent_claude/`). Do **not** clean before demoing — that is the work.
- Rust toolchain present (`cargo --version`, 1.85+).
- `ANTHROPIC_API_KEY` exported in the shell that will run the Claude agent. Smoke check: `echo "len=${#ANTHROPIC_API_KEY}"` should print non-zero.
- Docker running with the compose stack up: `otel-collector`, `jaeger`, `prometheus`, `grafana`, and `pkdealer_service`. All five containers are currently up on this machine — leave them.
- Ollama installed (`/usr/local/bin/ollama`) and the daemon listening on `127.0.0.1:11434`.
- `llama3.1` model pulled. **One-time, ~5 GB download:**
  ```bash
  ollama pull llama3.1
  ```
  `llama2:latest` (already on disk) is a viable fallback but follows the strict-format instruction less reliably — more demos hit fallback decisions instead of model-chosen ones.
- Five terminal panes: claude bot | ollama bot | service logs (`docker logs -f pkdealer-pkdealer_service-1`) | scratch (Jaeger / cleanup) | optional Grafana.
- Browser tabs ready: `http://localhost:16686` (Jaeger) and `http://localhost:3001` (Grafana, optional).
- No host process listening on 50051, 11434, 4317, 16686 outside the docker stack / Ollama daemon.

---

## Setup (~ 3 minutes — most of it pre-pulled)

1. **Confirm the stack is up**
   ```bash
   docker ps --format '{{.Names}}\t{{.Status}}' | grep pkdealer-
   ```
   _Expected:_ Five containers, all `Up`.
   _Talking point:_ Same EPIC-22 collector / Jaeger / Prometheus / Grafana stack; the service container exposes 50051 directly.

2. **Confirm Ollama is reachable**
   ```bash
   curl -s http://localhost:11434/api/tags | python3 -m json.tool | head -10
   ```
   _Expected:_ JSON listing `llama3.1` (and any other pulled models) with size / digest.
   _Talking point:_ No auth header, no API key — Ollama is the "free agent" half of the story.

3. **Confirm the API key is set**
   ```bash
   echo "ANTHROPIC_API_KEY length: ${#ANTHROPIC_API_KEY}"
   ```
   _Expected:_ Non-zero length. Don't echo the key.
   _Talking point:_ Claude is the "paid agent" half — head-to-head proves the trait abstracts over both.

4. **Build the agents (binaries only)**
   ```bash
   cargo build -p pkdealer_agent_ollama -p pkdealer_agent_claude
   ```
   _Expected:_ `Finished `dev` profile [...] target(s)` — first build ~30 s, warm build instant.
   _Talking point:_ Both crates pull `pkdealer_agent_llm` for prompt / parse / OTel — the only crate-local code is HTTP transport for one provider.

5. **Open the Jaeger UI in the scratch pane**
   ```bash
   open http://localhost:16686
   ```
   _Expected:_ Jaeger loads; service dropdown likely shows `pkdealer_service` from prior runs. New entries `pkdealer_agent_ollama` and `pkdealer_agent_claude` will appear once the agents emit their first spans.
   _Talking point:_ This is where the payoff lands — same hand, two different `gen_ai.system` values.

---

## The demo (~ 8 minutes)

### 1. Show the abstraction in code (1 min)

In a scratch pane, open three files side-by-side:

```bash
$EDITOR crates/pkdealer_agent_llm/src/backend.rs \
        crates/pkdealer_agent_claude/src/lib.rs \
        crates/pkdealer_agent_ollama/src/lib.rs
```
_Expected:_ `LlmBackend` trait (one async method); `ClaudeBackend` and `OllamaBackend` each implementing it.
_Talking point:_ One trait, one method. Adding a third backend is a struct + 60 lines of HTTP.

Point at the `LlmPokerAgent::with_model(backend, "ollama", model)` call in `agent.rs`:
```bash
grep -n "with_model\|gen_ai\." crates/pkdealer_agent_llm/src/agent.rs | head -10
```
_Expected:_ The `with_model` constructor + the `gen_ai.system` and `gen_ai.request.model` span attributes.
_Talking point:_ Those two strings are the entire mechanism — the OTel system / model split per backend.

### 2. Seat the Ollama agent

In **pane "ollama bot"**:
```bash
cargo run -p pkdealer_agent_ollama -- --name llama --model llama3.1 --seat 0
```
_Expected:_ `[llama] host=http://localhost:11434 model=llama3.1`, then a seat-taken log line in the service pane.
_Talking point:_ Local LLM. No tokens billed. The agent will idle until a second seat fills.

### 3. Seat the Claude agent

In **pane "claude bot"**:
```bash
cargo run -p pkdealer_agent_claude -- --name claude --seat 1
```
_Expected:_ `[claude] model=claude-sonnet-4-6` (or current default) and a second seat-taken in the service pane. The autonomous-loop service starts dealing.
_Talking point:_ Same workspace, same `LlmPokerAgent`, different `LlmBackend`. From the service's point of view they're indistinguishable.

### 4. Watch one full hand play

Eyes on the **service logs pane** for 60–90 s:
```bash
docker logs -f pkdealer-pkdealer_service-1
```
_Expected:_ Pre-flop / flop / turn / river action lines, alternating between `llama` and `claude`, ending in `hand_complete`.
_Talking point:_ Llama takes longer per decision (cold local inference) than Claude — point at the gap. That's the trade you're showing: free vs. fast.

### 5. Open Jaeger and find the hand

In the scratch browser tab:

- Service dropdown → pick **`pkdealer_agent_ollama`** → Find Traces.
  _Expected:_ One or more traces named `llm.decision`, each ~hundreds of ms to a few seconds.
  _Talking point:_ One span per Ollama decision. Look at `gen_ai.system=ollama`, `gen_ai.request.model=llama3.1`, and the token counts pulled from `prompt_eval_count` / `eval_count`.

- Switch service to **`pkdealer_agent_claude`** → Find Traces.
  _Expected:_ Same shape, but `gen_ai.system=anthropic`, `gen_ai.request.model=claude-sonnet-4-6`, and faster duration.
  _Talking point:_ Identical span shape from two different providers — the abstraction earned its keep here.

### 6. Open one nested trace

Click into any single `llm.decision` span and expand the trace tree.
_Expected:_ The `llm.decision` span nests under (or is causally linked to) the service-side `action` span via the EPIC-22 trace context propagation.
_Talking point:_ One Trace ID covers gRPC ingress on the service AND the LLM call on the agent. That cross-process correlation is what makes "why did this bot fold?" a one-click query.

---

## What to highlight verbally

- One trait, two backends, zero duplicated poker logic — the abstraction passes its first real test the moment a second backend ships.
- Adding `pkdealer_agent_openai` next is a struct plus a `complete()` impl — not a new crate from scratch.
- `gen_ai.*` semconv attributes are what make Jaeger work for both providers without special-casing.
- Cost story: live demo of a hand used to cost real Anthropic tokens; with Ollama in the mix the per-hand demo cost is electricity.
- Mock-HTTP tests live with each backend (`ClaudeBackend::with_base_url`, `OllamaBackend` against a TCP fixture) — CI exercises the wire format without any external service running.

---

## Likely questions & answers

**Q: Why not just hide the backend behind an env var instead of two crates?**
A: Each provider needs its own dependencies, CLI flags, and OTel `service.name`. Separate binaries keep the dependency graph honest and the OTel resource attributes correct. The shared crate is the abstraction; the binaries are just wiring.

**Q: How do you parse "raise" amounts from a model that writes prose?**
A: `parse_action` matches the trimmed lowercased response as a literal — `fold`, `check`, `call`, `raise <n>`, or `raise to <n>`. Anything else hits a safe fallback (`Check` if no bet to call, otherwise `Fold`). The fallback is also OTel-visible, so we can measure how often each model goes off-script.

**Q: Why is the Ollama decision slow?**
A: First-token latency on local CPU/GPU. `llama3.1` cold-starts in seconds; cached, it's faster but still slower than Claude over the wire. That's the trade — no API spend, slower hands.

**Q: Did you have to change `pkdealer_agent_core` or the service?**
A: No. The trait lives in a new shared crate that only the LLM-backed agents depend on. Random and rules agents stay LLM-free; the service is untouched. The refactor is additive at the workspace level.

**Q: What's next?**
A: Live smoke against `ollama serve` is the last open work item in EPIC-25. After that: a third backend (OpenAI or Gemini) to prove the trait shape generalizes, then a comparison harness that runs one `HandState` through every backend and logs the chosen actions side-by-side.

---

## Cleanup

The compose stack and Ollama daemon were already running before the demo — leave them. Just stop the foreground agents:

```bash
# In each agent pane:
Ctrl-C
```

If you started a fresh stack just for the demo and want it down:
```bash
docker compose down
pkill -f "ollama serve"
```

The EPIC-25 code remains uncommitted on `main`. When ready:
```bash
git status
# (review, then branch + commit per CLAUDE.md global rules)
```

---

## Troubleshooting

- **`llama3.1` not found / "model not pulled"** — run `ollama pull llama3.1` (one-time, ~5 GB). Or pass `--model llama2` to fall back to what's on disk; expect more fallback decisions.
- **Ollama agent decisions are all `Check`/`Fold`** — the model is writing prose instead of the strict format. Confirmed against `llama2`: response was `"I would respond by checking..."` which fails the literal parse and falls through to the `to_call==0 → Check` branch. Switch to `llama3.1`, which obeys "respond with EXACTLY one of: ..." more reliably.
- **Jaeger shows no traces** — the OTel exporter prints `OTel exporter init failed` and continues without tracing if the collector isn't reachable. Confirm `docker ps` shows `pkdealer-otel-collector-1` Up and port 4317 is listening (`lsof -iTCP:4317`).
- **Service banner reads `v0.1.7` but the agents are `v0.1.11`** — the Docker `pkdealer_service` image is older than the workspace. gRPC proto hasn't changed since v0.1.7 so this works, but if behavior is off, rebuild the image: `docker compose up -d --build pkdealer_service`.
- **`ANTHROPIC_API_KEY length: 0`** — set it in the shell *before* `cargo run` (clap reads it via the `env` attribute, not from `.env`).
