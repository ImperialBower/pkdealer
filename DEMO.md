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
