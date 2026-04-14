# EPIC-24: Demo Packaging

## Status

| Component | Status |
|---|---|
| `docker-compose.yml` — full platform stack | Planned |
| `demo.sh` — one-command launcher | Planned |
| `prometheus.yml` — scrape config | Planned |
| Grafana dashboard JSON (committed) | Planned |
| Langfuse (self-hosted) in compose stack | Planned |
| `DEMO.md` — presenter guide | Planned |
| Dockerfiles for service + spectator + agents | Planned |

---

## Context

EPIC-24 makes the full platform runnable as a live demo with a single command:
`./demo.sh`. This is the "conference demo" milestone — a terminal, two browser
tabs (spectator + Jaeger), and optionally Langfuse open side-by-side tell the
complete story of AI agents playing poker with full observability.

Everything in this EPIC assumes EPIC-20 (autonomous game loop), EPIC-21
(spectator), EPIC-22 (OTel), and EPIC-23 (bot agents) are complete.

---

## Design

### `docker-compose.yml`

```yaml
version: "3.9"
services:
  pkdealer_service:
    build:
      context: .
      dockerfile: crates/pkdealer_service/Dockerfile
    environment:
      OTEL_EXPORTER_OTLP_ENDPOINT: http://jaeger:4317
      PKDEALER_SPECTATOR_TOKEN: spectator
    ports:
      - "50051:50051"
    healthcheck:
      test: ["CMD", "grpc_health_probe", "-addr=:50051"]
      interval: 5s
      timeout: 3s
      retries: 10

  pkdealer_spectator:
    build:
      context: .
      dockerfile: crates/pkdealer_spectator/Dockerfile
    environment:
      PKDEALER_ENDPOINT: http://pkdealer_service:50051
      PKDEALER_SPECTATOR_TOKEN: spectator
    ports:
      - "3000:3000"
    depends_on:
      pkdealer_service:
        condition: service_healthy

  agent_gto:
    build:
      context: .
      dockerfile: crates/pkdealer_agent_rules/Dockerfile
    command: ["--name", "gto", "--profile", "/data/bots/gto.yaml"]
    environment:
      PKDEALER_ENDPOINT: http://pkdealer_service:50051
      OTEL_EXPORTER_OTLP_ENDPOINT: http://jaeger:4317
    volumes:
      - ./data/bots:/data/bots:ro
    depends_on:
      pkdealer_service:
        condition: service_healthy

  agent_lag:
    build:
      context: .
      dockerfile: crates/pkdealer_agent_rules/Dockerfile
    command: ["--name", "lag", "--profile", "/data/bots/loose_aggressive.yaml"]
    environment:
      PKDEALER_ENDPOINT: http://pkdealer_service:50051
      OTEL_EXPORTER_OTLP_ENDPOINT: http://jaeger:4317
    volumes:
      - ./data/bots:/data/bots:ro
    depends_on:
      pkdealer_service:
        condition: service_healthy

  agent_claude:
    build:
      context: .
      dockerfile: crates/pkdealer_agent_claude/Dockerfile
    command: ["--name", "claude"]
    environment:
      PKDEALER_ENDPOINT: http://pkdealer_service:50051
      ANTHROPIC_API_KEY: ${ANTHROPIC_API_KEY}
      OTEL_EXPORTER_OTLP_ENDPOINT: http://jaeger:4317
    depends_on:
      pkdealer_service:
        condition: service_healthy

  jaeger:
    image: jaegertracing/all-in-one:latest
    ports:
      - "16686:16686"   # Jaeger UI
      - "4317:4317"     # OTLP gRPC

  prometheus:
    image: prom/prometheus:latest
    volumes:
      - ./prometheus.yml:/etc/prometheus/prometheus.yml:ro
    ports:
      - "9090:9090"

  grafana:
    image: grafana/grafana:latest
    volumes:
      - ./grafana/dashboards:/var/lib/grafana/dashboards:ro
      - ./grafana/provisioning:/etc/grafana/provisioning:ro
    ports:
      - "3001:3000"
    depends_on:
      - prometheus

  langfuse:
    image: langfuse/langfuse:latest
    environment:
      DATABASE_URL: postgresql://langfuse:langfuse@langfuse_db/langfuse
      NEXTAUTH_SECRET: demo-secret
      NEXTAUTH_URL: http://localhost:3002
    ports:
      - "3002:3000"
    depends_on:
      - langfuse_db

  langfuse_db:
    image: postgres:16
    environment:
      POSTGRES_DB: langfuse
      POSTGRES_USER: langfuse
      POSTGRES_PASSWORD: langfuse
    volumes:
      - langfuse_pg:/var/lib/postgresql/data

volumes:
  langfuse_pg:
```

### `demo.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail

# Require ANTHROPIC_API_KEY if Claude agent is enabled (set to empty to skip)
: "${ANTHROPIC_API_KEY:?Set ANTHROPIC_API_KEY or unset the claude agent in compose}"

echo "Starting pkdealer demo stack..."
docker compose up -d

echo "Waiting for service to be healthy..."
until docker compose exec pkdealer_service grpc_health_probe -addr=:50051 &>/dev/null; do
  sleep 1
done

echo ""
echo "Demo is live!"
echo "  Spectator:  http://localhost:3000"
echo "  Jaeger:     http://localhost:16686"
echo "  Grafana:    http://localhost:3001"
echo "  Langfuse:   http://localhost:3002"
echo ""
echo "Press Ctrl+C to stop."

# Open browser tabs (macOS)
if command -v open &>/dev/null; then
  open http://localhost:3000
  open http://localhost:16686
fi

docker compose logs -f pkdealer_service pkdealer_spectator
```

### Dockerfiles

Each crate gets a multi-stage Dockerfile following the same pattern:

```dockerfile
# Build stage
FROM rust:1.85-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin pkdealer_service

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/pkdealer_service /usr/local/bin/
EXPOSE 50051
ENTRYPOINT ["pkdealer_service"]
```

One Dockerfile per binary, sharing the same build context (workspace root).

### Grafana provisioning

Committed files under `grafana/`:

```
grafana/
├── provisioning/
│   ├── datasources/
│   │   └── prometheus.yml    — auto-wire Prometheus datasource
│   └── dashboards/
│       └── pkdealer.yml      — auto-load dashboard from file
└── dashboards/
    └── pkdealer.json         — dashboard definition
```

Dashboard panels:
- **Hands played** — counter rate over time
- **Pot size distribution** — histogram heatmap
- **Action latency** — p50/p95/p99 by action type
- **AI decision latency** — p50/p95/p99 by agent type (Claude vs rule-based)
- **Active hand** — duration gauge

### `prometheus.yml`

```yaml
global:
  scrape_interval: 15s

scrape_configs:
  - job_name: pkdealer
    static_configs:
      - targets: ["pkdealer_service:9090"]
```

The service exposes a Prometheus metrics endpoint on port 9090 (Axum or
`opentelemetry-prometheus` exporter) alongside the gRPC port 50051.

---

## Work Items

1. Write `Dockerfile` for `pkdealer_service`
2. Write `Dockerfile` for `pkdealer_spectator`
3. Write `Dockerfile` for `pkdealer_agent_rules`
4. Write `Dockerfile` for `pkdealer_agent_claude`
5. Write `docker-compose.yml` with all services, health checks, and env wiring
6. Write `prometheus.yml` scrape config
7. Write Grafana provisioning files and dashboard JSON
8. Write `demo.sh` launcher
9. Test full compose stack locally end-to-end (all containers healthy, game running, traces in Jaeger, metrics in Grafana)
10. Write `DEMO.md` presenter guide (what to click, what to say, expected output)
11. Add `.env.example` with all required environment variables documented

---

## Verification

```bash
# Full demo start
export ANTHROPIC_API_KEY=sk-...
./demo.sh

# Verify all containers are healthy
docker compose ps

# Confirm game is running (at least 1 hand played)
docker compose logs pkdealer_service | grep "HandEnded"

# Jaeger: search service "pkdealer_service" — should show hand + action spans
open http://localhost:16686

# Grafana: pkdealer dashboard — should show hands/min > 0
open http://localhost:3001

# Spectator: cards, board, pot updating in real time
open http://localhost:3000

# Teardown
docker compose down -v
```
