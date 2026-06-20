# pkdealer Demo Runbook

A one-command launch of the full pkdealer platform: dealer service, six
agent containers (three rule bots + three LLMs), OpenTelemetry collector,
Jaeger, Prometheus, and
Grafana. The spectator UI runs separately from the
[pkspectator](https://github.com/ImperialBower/pkspectator) repo.

## Prerequisites

- **Docker Desktop** (or any Docker engine with Compose v2)
- **Ollama** running on the host:
  ```bash
  ollama serve            # in a dedicated terminal
  ollama pull llama3.1    # one-time, ~4.7 GB
  ollama pull mistral     # one-time, ~4.1 GB
  ollama pull gemma2      # one-time, ~5.4 GB
  ```
  If ollama isn't running, `./bin/aiarena` prints a warning and continues; the
  three LLM agent containers fail, the three rule bots still play.
- **pkspectator** checked out alongside this repo, e.g. `../pkspectator`.

## Launch

```bash
./bin/aiarena
```

The script builds images (first run takes a few minutes), starts the
stack, waits for the dealer to accept gRPC connections, and prints the
URLs.

In a second terminal:

```bash
cd ../pkspectator
cargo run
open http://localhost:3000
```

### All-bots variant (`./bin/botarena`)

For a pure rule-bot shootout — no LLMs, no ollama dependency — run
`./bin/botarena` instead. It seats a full 9-handed ring of every pkcore
archetype (`gto`, `tag`, `lag`, `tp`, `lp`, `maniac`, `abc`, `ssn`,
`joker`) against each other, sharing the same dealer + observability stack.
Both scripts drive a single `docker-compose.yml` and select their agents via
compose profiles (`aiarena` vs `botarena`), so don't run both at once — they
share the dealer's port. Tear one down (`docker compose down -v`) before
launching the other.

### Custom line-ups (`./bin/arena`)

`./bin/aiarena` and `./bin/botarena` are fixed rosters. To compose an arbitrary
table from the terminal — without editing `docker-compose.yml` — use
`./bin/arena` and pass a space-separated player list (EPIC-42). The player
registry lives in [`arena.toml`](arena.toml); run `./bin/arena --help` to list
the 14 known names and their types.

```bash
# Classic 6-seat mixed arena (equivalent to ./bin/aiarena):
./bin/arena gto lag tag llama mistral gemma

# Two GTO bots + a Claude agent + three rule archetypes:
./bin/arena gto:2 claude tag maniac lp        # `gto:2` == `gto gto`

# All-bot 9-seat ring (equivalent to ./bin/botarena):
./bin/arena gto lag tag tp lp maniac abc ssn joker

# Quick 3-seat debug table:
./bin/arena gto lag claude
```

`bin/arena` generates a one-off compose override in `/tmp` and brings up the
same infra + observability stack; the teardown command (which references the
override file) is printed at the end. Add `--dry-run` to generate and inspect
the override without starting anything. Notes:

- **At most 9 seats**; repeats get unique names (`gto gto` → `agent_gto_1`,
  `agent_gto_2`), each its own Jaeger service and `--name`.
- **`claude`** needs `ANTHROPIC_API_KEY` exported in your shell (live, billed);
  without it the container is still created but exits until the key is set.
- **`gwen`** (Gemini) is **not yet available** — its agent crate is EPIC-42
  Phase 3, deferred. Selecting it is rejected with a clear message.

## Token & cost simulation + PokerBench models (EPIC-44 / EPIC-43)

LLM seats record their token usage per decision; the service prices it against
`pricing.toml` and surfaces per-seat `input_tokens` / `output_tokens` /
`cost_micro_usd` on `SeatInfo`. The pricing is wired into the demo service in
`docker-compose.yml` (`PKDEALER_PRICING` + `PKDEALER_PRICE_AS`), so any arena run
with an LLM seat shows live **Tokens** and **Cost$** columns in pktui:

```bash
# bring up a table with an LLM seat (cost shows for LLM seats; bots stay blank):
./bin/arena gto lag gemma
# watch it live — Tokens + Cost$ columns update each decision:
cd ../pktui && cargo run -- spectate --endpoint http://127.0.0.1:50051
```

Local Ollama model ids are priced *as* a commercial model via `PKDEALER_PRICE_AS`
(default maps `gemma2→claude-opus-4-8`, `llama3.1→gpt-4.1`, `mistral→claude-haiku-4-5`).
Override per run, e.g. `PKDEALER_PRICE_AS="gemma2=gpt-4.1-nano" ./bin/arena gto gemma`.

**PokerBench-guided models.** `make pokerbench-models` bakes sampled
solver-optimal PokerBench decisions into each base model's system prompt and
runs `ollama create`, producing `pkpoker-gemma` / `pkpoker-llama` /
`pkpoker-mistral` / `pkpoker-qwen` (seated as `pkgemma` / `pkllama` /
`pkmistral` / `pkqwen`). It runs entirely on local Ollama — no GPU, no cloud —
and auto-downloads the dataset:

```bash
ollama serve && ollama pull gemma2 llama3.1 mistral qwen2.5:3b   # one-time prereqs
make pokerbench-models                                 # downloads data + creates models
./bin/arena pkgemma gto lag                            # seat the guided model
```

Because the PokerBench knowledge lives in the system prompt rather than the
weights, the same examples port to any base with no retraining. `pkqwen` builds
FROM the small `qwen2.5:3b` (~1.9 GB) instead of the 9B `gemma2` (~5.4 GB), so it
decides with much lower latency — seat it when the larger seats feel slow:

```bash
./bin/arena pkqwen gto lag tag                         # fast PokerBench seat
```

The few-shot system prompt adds ~1.5–2k input tokens per decision, so the
`pkpoker-*` seats show visibly higher Tokens/Cost than the bare `gemma` seat —
the price of in-context knowledge, shown live. Tune the example count with
`POKERBENCH_EXAMPLES=N`; inspect Modelfiles without creating via
`make pokerbench-models ARGS="--dry-run"`.

> Note: this is in-context guidance, not weight-level fine-tuning (a 16GB Mac
> can't train 8–9B models). Real fine-tuning would run on HuggingFace Jobs
> (cloud, paid); `make pokerbench-data` already produces the train sets it needs.

## What to show

Arrange three browser tabs side-by-side:

| Tab | URL | Pitch |
|-----|-----|-------|
| Spectator | http://localhost:3000 | "Six AI agents — three rule-based bots and three local LLMs (llama, mistral, gemma) — playing live. The table is reading the dealer's gRPC event stream." |
| Jaeger | http://localhost:16686 | "Every action, every LLM call, every gRPC hop is traced. Pick the `agent_llama` (or `agent_mistral` / `agent_gemma`) service and drill into a `gen_ai.completion` span — that's the model thinking." |
| Grafana | http://localhost:3001/d/pkdealer | "Hands per minute, pot-size distribution, action latency by phase. All metrics flow through the OTel collector — no Prometheus instrumentation in the application code." |

### Suggested narration beats

1. **Open spectator** — wait for a hand to start, point at one seat. "That's `gto` — pkcore rule-based bot driven by a profile YAML. Next to it, `lag` — loose-aggressive, same binary, different profile."
2. **Switch to Jaeger** — search service `pkdealer_service`. Open a hand trace; expand to show child action spans, then click into an `agent_llama` (or `agent_mistral` / `agent_gemma`) span. The `gen_ai` attributes (model, prompt, completion, token counts) are visible inline.
3. **Switch to Grafana** — the `pkdealer` dashboard. Walk through hands/min, latency p50/p95/p99, pot-size heatmap.

## Tear down

```bash
docker compose down -v
```

`-v` drops named volumes, leaving no state for the next demo.

## Extending

- **Adding Claude as an agent.** Append a `agent_claude` service to
  `docker-compose.yml` using `Dockerfile.agent` with
  `BIN_NAME: pkdealer_agent_claude`. Pass `ANTHROPIC_API_KEY` from
  `.env`. The binary CLI matches the other agents.
- **Different rules profiles.** Mount `./data/bots` into the agent
  container and pass `--profile /data/bots/<file>.yaml`. Built-in
  profile names (`gto`, `loose_aggressive`, `tight_aggressive`, etc.)
  are resolved without a mount.
- **Table pacing.** Agents pause so the action is watchable:
  `PKDEALER_ACTION_DELAY_SECS` (default `1`) before each action and
  `PKDEALER_HAND_END_DELAY_SECS` (default `5`) after every hand ends. Override
  per run, e.g. `PKDEALER_ACTION_DELAY_SECS=2 ./bin/aiarena`, or set
  `PKDEALER_ACTION_DELAY_SECS=0` for a full-speed table.
- **Different ollama models.** Each LLM seat reads its own override env var:
  `LLAMA_MODEL` (default `llama3.1`), `MISTRAL_MODEL` (default `mistral`),
  and `GEMMA_MODEL` (default `gemma2`). Set any of them in `.env` (e.g.
  `GEMMA_MODEL=phi3` to lighten the host); `ollama pull` it first.

## Troubleshooting

| Symptom | Likely cause |
|---------|---------------|
| `agent_llama` / `agent_mistral` / `agent_gemma` keeps restarting | `ollama serve` not running, or that model not pulled (`ollama pull llama3.1 mistral gemma2`) |
| Grafana shows no data | Wait ~30 s for first scrape; verify `up{job="otel-collector"}` in Prometheus |
| Spectator can't connect | Run from the pkspectator repo, not from inside compose; the dealer port (50051) is exposed on localhost |
| Container can't reach the host | On Linux, `extra_hosts: host.docker.internal:host-gateway` is already set; otherwise check firewall |
