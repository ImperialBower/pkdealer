# Demo: EPIC-25 — Eight-Player Multi-Model Shootout

> Six small local LLMs (Llama 3.2 3B, Qwen 2.5 3B, Phi-3.5, Gemma 2 2B, Llama 3.2 1B, Mistral 7B) seated against two sound rule-based bots (`gto`, `tight_aggressive`) at the same nine-max table. The audience sees eight independent agents — six talking to a single Ollama daemon, two pure-Rust — playing real hands while Jaeger streams a `gen_ai.system=ollama` span per LLM decision next to deterministic `pkdealer_agent_rules` decisions, all under one autonomous game loop.

## Audience & framing

Engineering peers (or curious onlookers) interested in *what falls out of the EPIC-25 abstraction once you push on it*. The angle is **the trait pays for itself the second time you re-use it, and the sixth**: one `LlmBackend` impl + six CLI flags = six model personalities at the same table, no per-model crate. The bots are the control group — the audience can see by eye whether the models behave like poker players or like coin-flippers.

**Bot pick justification (raise this verbally, in case anyone asks):** of `pkdealer_agent_rules`'s nine built-in profiles, `gto` is the math-driven optimum and `tight_aggressive` (TAG) is the textbook winning style in NLHE literature. Swap to `abc` or `tight_passive` if your audience prefers a more "human" baseline.

> Verified during prep: all six recommended models are already pulled on this machine. `cargo check` was not re-run for the runbook; the prior EPIC-25 presentation confirms the workspace builds. **Docker daemon was not running at prep time** — the OTel stack is down. The runbook offers two paths: (A) start Docker for the full Jaeger story, or (B) run with `OTEL_SDK_DISABLED=true` for a service-only demo. Pick before the audience arrives.

---

## Prerequisites

- Repo at `/Users/christoph/src/github.com/ImperialBower/pkdealer`, branch `main` (or `epic-25` if you want the in-flight version).
- Rust 1.85+ (`cargo --version`).
- Ollama daemon listening on `127.0.0.1:11434`.
  - Smoke: `curl -s http://localhost:11434/api/tags | python3 -m json.tool | head`
  - All six models must appear: `llama3.2:1b`, `llama3.2:3b`, `qwen2.5:3b`, `phi3.5:latest`, `gemma2:2b`, `mistral:latest`. Total disk ~13 GB.
- **RAM budget**: 6 models × KV cache. Set `OLLAMA_MAX_LOADED_MODELS=6` and `OLLAMA_NUM_PARALLEL=1` *before* starting `ollama serve` — without this, Ollama defaults to keeping ~3 models loaded and the other three swap on every decision, turning a 60-second hand into a 5-minute hand.
- **If using Jaeger** (path A): Docker daemon running; `docker compose up -d` in repo root brings up `otel-collector`, `jaeger`, `prometheus`, `grafana`, `pkdealer_service`.
- **If not** (path B): nothing extra. `pkdealer_service` runs as `cargo run -p pkdealer_service`.
- Terminal layout: 11 panes is excessive for live presentation. Use **tmux with a 4×3 grid** (12 panes, one unused) or run the eight agents under a single helper script (provided in Setup step 5) and keep three visible panes: service logs, agent log multiplexer, Jaeger/browser.
- Browser tabs: `http://localhost:16686` (Jaeger, path A only). Optional: `http://localhost:3001` (Grafana).
- No host process bound to ports `50051`, `11434`, `4317`, `16686`.

---

## Setup (~ 4 minutes, path A — or ~ 90 seconds, path B)

1. **Configure Ollama for multi-model residency**
   ```bash
   pkill -f "ollama serve" 2>/dev/null
   OLLAMA_MAX_LOADED_MODELS=6 OLLAMA_NUM_PARALLEL=1 ollama serve &
   sleep 2
   curl -s http://localhost:11434/api/tags | python3 -c "import sys,json; print('models:', len(json.load(sys.stdin)['models']))"
   ```
   _Expected:_ `models: 7` (or more — only six are needed).
   _Talking point:_ One daemon, six in-memory models, sequential requests per model — Ollama's process pool keeps all six warm so we don't pay load latency mid-hand.

2. **Bring up the OTel + service stack — path A**
   ```bash
   docker compose up -d
   docker compose ps --format '{{.Service}}\t{{.Status}}'
   ```
   _Expected:_ Five services, all `running`.
   _Talking point:_ Same EPIC-22 stack — collector at 4317, Jaeger at 16686, the service at 50051.

   **Or — path B (no Docker):**
   ```bash
   OTEL_SDK_DISABLED=true cargo run -p pkdealer_service &
   sleep 3
   ```
   _Expected:_ `pkdealer_service listening on 127.0.0.1:50051`.
   _Talking point:_ No traces today — we'll watch chip flow in logs instead.

3. **Build every agent binary once**
   ```bash
   cargo build -p pkdealer_agent_ollama -p pkdealer_agent_rules
   ```
   _Expected:_ `Finished `dev` profile [...]`.
   _Talking point:_ Two binaries cover all eight players — the multi-model story is entirely a `--model` flag.

4. **Smoke each model once (no service, no seating — just confirm Ollama answers)**
   ```bash
   for m in llama3.2:3b qwen2.5:3b phi3.5 gemma2:2b llama3.2:1b mistral; do
     echo -n "$m: "
     curl -s http://localhost:11434/api/chat -d "{\"model\":\"$m\",\"stream\":false,\"messages\":[{\"role\":\"user\",\"content\":\"reply with only the word: call\"}]}" \
       | python3 -c "import sys,json; print(json.load(sys.stdin)['message']['content'].strip()[:40])"
   done
   ```
   _Expected:_ Each model prints `call` or close to it (Phi may add punctuation; Llama 1B may chatter).
   _Talking point:_ The smallest model already shows the format-following lottery — that's why the parser at `parse.rs:30` has a deterministic fallback.

5. **Create a helper script that seats all eight agents**

   The runbook expects a `demo/shootout.sh` file. If it doesn't exist yet, create it now (one-time, ~30 s):

   ```bash
   cat > demo/shootout.sh <<'EOF'
   #!/usr/bin/env bash
   # Seats eight players: six Ollama models + two sound rules bots.
   set -euo pipefail
   ROOT="$(cd "$(dirname "$0")/.." && pwd)"
   cd "$ROOT"

   LOG=demo/shootout-logs
   mkdir -p "$LOG"

   echo "Launching 8 agents (logs in $LOG/) …"

   # 6 Ollama agents — seats 0..5
   for pair in "0:llama3.2:3b:llama3" "1:qwen2.5:3b:qwen" "2:phi3.5:phi" \
               "3:gemma2:2b:gemma" "4:llama3.2:1b:lil-llama" "5:mistral:mistral"; do
     seat="${pair%%:*}"
     rest="${pair#*:}"
     model="${rest%:*}"
     name="${rest##*:}"
     cargo run --quiet -p pkdealer_agent_ollama -- \
       --seat "$seat" --model "$model" --name "$name" \
       > "$LOG/seat${seat}-${name}.log" 2>&1 &
     echo "  seat $seat: $name ($model) pid=$!"
   done

   # 2 Rules bots — seats 6, 7
   cargo run --quiet -p pkdealer_agent_rules -- \
     --seat 6 --profile gto --name gto-bot \
     > "$LOG/seat6-gto-bot.log" 2>&1 &
   echo "  seat 6: gto-bot (gto) pid=$!"

   cargo run --quiet -p pkdealer_agent_rules -- \
     --seat 7 --profile tag --name tag-bot \
     > "$LOG/seat7-tag-bot.log" 2>&1 &
   echo "  seat 7: tag-bot (tight_aggressive) pid=$!"

   echo "All eight agents launched. Tail with:  tail -F $LOG/seat*.log"
   wait
   EOF
   chmod +x demo/shootout.sh
   ```
   _Expected:_ `demo/shootout.sh` exists, executable.
   _Talking point:_ Production demos would use a process supervisor; for stage, a backgrounded loop is enough — the autonomous game loop (EPIC-20) handles street advance and hand end.

---

## The demo (~ 6–10 minutes)

### 1. Seat the table

1. **Run the shootout script**
   ```bash
   ./demo/shootout.sh
   ```
   _Expected:_ Eight lines, each showing `seat N: <name> (<model>) pid=…`.
   _Talking point:_ Eight players in one command — six talk to Ollama, two are pure Rust deciders. The service doesn't know or care.

2. **Confirm all eight are seated**
   ```bash
   grep -h "took seat" demo/shootout-logs/seat*.log | sort
   ```
   _Expected:_ Eight lines, one per seat 0–7.
   _Talking point:_ Each agent calls `TakeSeat`; the service's autonomous loop will start the hand once seven blinds-eligible players are present.

### 2. Watch a hand play

3. **Tail the service log (path A)**
   ```bash
   docker logs -f $(docker compose ps -q pkdealer_service) | grep -E "HandStarted|StreetAdvanced|action|HandEnded"
   ```
   **Or (path B):** the service is in the background of this shell — bring it forward or scroll its pane.

   _Expected:_ `HandStarted` → many `action` lines → `StreetAdvanced` (flop, turn, river) → `HandEnded` with a winner.
   _Talking point:_ One autonomous game loop, eight independent decision-makers. The service has no model awareness — just legal-action enforcement.

4. **Watch one model's decisions in isolation**
   ```bash
   tail -F demo/shootout-logs/seat0-llama3.log
   ```
   _Expected:_ One line per turn — `decision: Call` / `Raise(400)` etc., with the prompt and raw response logged.
   _Talking point:_ This is the LLM "thinking out loud" — useful when explaining why a model folded a hand the audience thinks looks good.

5. **Diff personalities by tailing two seats side-by-side**
   ```bash
   # in a fresh pane
   tail -F demo/shootout-logs/seat4-lil-llama.log demo/shootout-logs/seat6-gto-bot.log
   ```
   _Expected:_ The 1B model chatters, occasionally produces malformed actions (parser falls back to fold/check); the `gto` bot emits clean decisions every time.
   _Talking point:_ The smallest model is essentially a structured-output stress test. The bot is the control — same table state, deterministic answer.

### 3. Show the trace story (path A only)

6. **Open Jaeger and find a hand**
   - Browser: `http://localhost:16686`
   - Service: `pkdealer_service`
   - Operation: `hand`
   - Click any recent trace.

   _Expected:_ A `hand` span with nested `action` spans; each `action` from an LLM seat has a child `llm.decision` span with `gen_ai.system=ollama` and `gen_ai.request.model=<the model>`.
   _Talking point:_ Six different `gen_ai.request.model` values in one trace — that's the whole EPIC-25 punchline rendered visually.

7. **Filter to one model's spans**
   - Jaeger search → Tags: `gen_ai.request.model=qwen2.5:3b`
   - Find decisions across hands.

   _Talking point:_ Per-model decision latency, token counts, fallback rate — all queryable. Adding the seventh model wouldn't change this query.

### 4. The honest moment (optional, recommended)

8. **Show which model is "winning"**
   ```bash
   grep -h "HandEnded" demo/shootout-logs/seat*.log 2>/dev/null \
     || docker logs $(docker compose ps -q pkdealer_service) 2>&1 | grep "HandEnded" | tail -20
   ```
   _Expected:_ Hand-end summaries with chip deltas per seat.
   _Talking point:_ Don't oversell — twenty hands is variance, not skill. But the *direction* is real: the bots will usually be even or up, the 1B model will usually be down.

---

## What to highlight verbally

- **The trait paid for itself.** Six models in one demo cost six CLI flags, not six crates. That's the EPIC-25 thesis stated by the price tag, not the slide deck.
- **Same prompt, six personalities.** Identical `build_prompt` output goes to all six LLMs — divergence is *purely* model behavior. That's a free A/B harness for any future prompt edit.
- **The parser is a load-bearing safety net.** With a 1.3 GB model at the table, malformed outputs happen. `parse_action`'s fallback at `parse.rs:59-63` is why the table doesn't deadlock when a model hallucinates `"I think we should fold."` instead of `"fold"`.
- **The bots are the control group.** Deterministic deciders make it obvious when the LLMs are gambling vs. reasoning. Without them, the audience has no baseline.
- **OTel + multi-backend = comparable telemetry for free.** `gen_ai.system` is a standard semconv attribute. The Jaeger search "`gen_ai.request.model=mistral`" works because of the trait-level instrumentation in `pkdealer_agent_llm`, not per-backend code.

---

## Likely questions & answers

**Q: Why not put all six models behind one agent process?**
A: Each agent process owns one seat at the table; one model per process is the cleanest mapping. Multiplexing six models into one process is doable (one `LlmBackend` per turn) but buys nothing for a live demo and complicates the trace story.

**Q: Are these actually playing poker, or just emitting legal actions?**
A: Both. The prompt includes hole cards, board, pot, to-call, and stack; the parser only checks the action *grammar*, not whether it's good poker. So the 7B and 3B models often play recognizable lines; the 1B model is closer to "structured noise that the parser laundered into a legal call."

**Q: Why no Claude or OpenAI in this demo?**
A: Cost and reproducibility. This runbook is meant to run anywhere with Ollama installed and no API keys. The previous EPIC-25 presentation covers the Claude side-by-side story.

**Q: What about DeepSeek-R1, which is also on this machine?**
A: It's an R1 reasoning model — emits `<think>...</think>` blocks before the answer, which the strict-prefix parser in `parse.rs` discards. R1 would fold-or-check every hand. Including it would be a worked example of why output format matters more than model size for agent design — fair to mention if a question opens the door.

**Q: How many hands will play in the demo window?**
A: With six models warm and `OLLAMA_NUM_PARALLEL=1`, expect ~30–60 seconds per hand. In an 8-minute demo, plan for 5–10 hands. If the 1B model is on a folding streak, hands go faster (fewer streets to act on).

---

## Cleanup

```bash
# Kill all agents
pkill -f pkdealer_agent_ollama
pkill -f pkdealer_agent_rules

# Path A: stop the stack
docker compose down

# Path B: stop the service
pkill -f pkdealer_service

# Reset Ollama to its normal single-model mode (kill the demo daemon, let macOS Ollama.app respawn)
pkill -f "ollama serve"

# Logs
rm -rf demo/shootout-logs
```

If you intend to re-run the demo within an hour, skip the Ollama and Docker steps — they're idempotent on restart.

---

## Troubleshooting

- **Hands take five minutes each, not one.** Ollama is swapping models. Confirm `OLLAMA_MAX_LOADED_MODELS=6` is set in the daemon's environment (`ps eww $(pgrep -f "ollama serve")` to inspect). On macOS the GUI Ollama.app ignores the shell — kill it and `ollama serve` from the shell with the env var set.
- **An agent silently exits at startup.** Tail its log: `tail -50 demo/shootout-logs/seat<N>-*.log`. Most common: `TakeSeat` denied because that seat is occupied by a stale agent from a prior run. Fix with `pkill -f pkdealer_agent`.
- **Jaeger shows zero `llm.decision` spans.** OTel collector not up, or `OTEL_SDK_DISABLED=true` is set in one of the agent shells. Confirm `docker compose ps otel-collector` and that the launch script's environment is clean.
- **`gemma2:2b` produces empty action text.** Gemma occasionally returns just whitespace for short prompts; the parser falls back to check/fold. If it persists for a whole demo, swap `gemma2:2b` → `gemma2:9b` (~5 GB) in `shootout.sh` line 13.
- **1B model never wins a hand.** Working as intended. That's the talking point, not the bug.
