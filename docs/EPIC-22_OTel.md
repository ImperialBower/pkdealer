# EPIC-22: OpenTelemetry Instrumentation

## Status

| Component | Status |
|---|---|
| `tracing` + `tracing-subscriber` in `pkdealer_service` | Planned |
| `opentelemetry` + `opentelemetry-otlp` integration | Planned |
| `hand` span (deal → showdown) | Planned |
| `street` span (child of `hand`) | Planned |
| `action` span (child of `hand`) | Planned |
| `pkdealer.hands_played` counter metric | Planned |
| `pkdealer.pot_size` histogram metric | Planned |
| `pkdealer.action_duration_ms` histogram metric | Planned |
| `pkdealer.ai_decision_latency_ms` histogram metric | Planned |
| Trace context propagation in gRPC metadata | Planned |
| `docker-compose.yml` — Jaeger + Prometheus + Grafana | Planned |
| Grafana dashboard JSON (committed) | Planned |

---

## Context

Observability is the core "technical demonstration" value of the platform.
EPIC-22 makes every game event visible in Jaeger (traces) and Grafana
(metrics). A live demo can show two browser tabs: the spectator view (EPIC-21)
and Jaeger — together they tell the complete story of what the AI agents
decided and why each hand evolved the way it did.

The instrumentation is vendor-neutral: spans are emitted as OpenTelemetry
OTLP, which works with Jaeger, Grafana Tempo, Honeycomb, or any OTLP-compatible
backend without code changes.

---

## Design

### Crate dependencies (pkdealer_service)

```toml
[dependencies]
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
opentelemetry = { version = "0.26", features = ["metrics"] }
opentelemetry-otlp = { version = "0.26", features = ["grpc-tonic", "metrics"] }
opentelemetry_sdk = { version = "0.26", features = ["rt-tokio", "metrics"] }
tracing-opentelemetry = "0.27"
```

### Span hierarchy

```
hand (span)
│  attributes: hand_id, player_count, starting_pot, button_seat
│
├── street (span, one per street)
│   attributes: street_name ("preflop"|"flop"|"turn"|"river"), board_cards
│
└── action (span, one per player action)
    attributes: seat, player_name, action_type, amount, pot_after, next_to_act
```

**`hand` span lifecycle:** opened in `StartHand` handler, closed in `EndHand`
handler. Carried through via a per-hand `Span` stored in `TableState`.

```rust
struct TableState {
    session: PokerSession,
    token_to_seat: HashMap<Uuid, u8>,
    seat_to_token: HashMap<u8, Uuid>,
    current_hand_span: Option<tracing::Span>,   // ← new
}
```

**`action` span:** opened and closed inside the `act` handler. Child of the
current `hand` span via `tracing`'s context propagation.

### Metrics

| Metric | Type | Attributes | Description |
|--------|------|------------|-------------|
| `pkdealer.hands_played` | Counter | — | Incremented on every `end_hand` |
| `pkdealer.pot_size` | Histogram | — | Final pot size per hand |
| `pkdealer.action_duration_ms` | Histogram | `action_type`, `seat` | Time from `next_actor` prompt to `act` receipt |
| `pkdealer.ai_decision_latency_ms` | Histogram | `agent_type` | Emitted by agent clients (EPIC-23); tagged by model |

Metrics are exported via OTLP to Prometheus (via `opentelemetry-otlp`) and
scraped by the Prometheus container in the compose stack.

### Trace context propagation (gRPC)

Agent clients (EPIC-23) start a local span for each decision, inject the
`traceparent` header into gRPC metadata before calling `Act`, and the service
extracts it to make the action span a child of the agent's decision span.

```rust
// In the act handler (server side):
let parent_cx = opentelemetry::global::get_text_map_propagator(|p| {
    p.extract(&MetadataCarrier(request.metadata()))
});
let span = tracer.start_with_context("action", &parent_cx);
```

This wires the full trace: agent decision span → service action span → hand span.
In Jaeger, a single hand trace shows every AI decision nested under each action.

### Initialization

OTel is initialized in `main` before the gRPC server starts:

```rust
fn init_otel() -> SdkTracerProvider {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4317".to_owned()));
    opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(Config::default().with_resource(
            Resource::new(vec![KeyValue::new("service.name", "pkdealer_service")])
        ))
        .install_batch(opentelemetry_sdk::runtime::Tokio)
        .unwrap()
}
```

### Docker Compose stack

```yaml
# docker-compose.yml
services:
  pkdealer_service:
    build: ./crates/pkdealer_service
    environment:
      OTEL_EXPORTER_OTLP_ENDPOINT: http://jaeger:4317
    ports: ["50051:50051"]

  jaeger:
    image: jaegertracing/all-in-one:latest
    ports:
      - "16686:16686"   # Jaeger UI
      - "4317:4317"     # OTLP gRPC receiver

  prometheus:
    image: prom/prometheus:latest
    volumes: ["./prometheus.yml:/etc/prometheus/prometheus.yml"]
    ports: ["9090:9090"]

  grafana:
    image: grafana/grafana:latest
    volumes: ["./grafana/dashboards:/var/lib/grafana/dashboards"]
    ports: ["3001:3000"]
```

### Grafana dashboard

A committed `grafana/dashboards/pkdealer.json` shows:
- Active hand timeline (Jaeger trace embed or span duration histogram)
- Hands played per minute (counter rate)
- Pot size distribution (histogram heatmap)
- Action latency by type (percentile panel)
- AI decision latency by agent type (for EPIC-23)

---

## Work Items

1. Add OTel dependencies to `pkdealer_service/Cargo.toml`
2. Add `init_otel()` in `main.rs`; set up `tracing-subscriber` with OTel layer
3. Add `current_hand_span: Option<tracing::Span>` to `TableState`
4. Open `hand` span in `start_hand` handler; attach `hand_id`, `player_count`, `starting_pot`
5. Open/close `street` spans in the auto-advance loop (EPIC-20); attach `street_name`, `board_cards`
6. Open/close `action` spans in `act` handler; extract `traceparent` from gRPC metadata for parent context
7. Initialize metric instruments (`Counter`, `Histogram`) at startup; record in handlers
8. Write `docker-compose.yml` with Jaeger + Prometheus + Grafana
9. Write `prometheus.yml` scrape config for the service metrics endpoint
10. Commit `grafana/dashboards/pkdealer.json` with the dashboard definition
11. Document `OTEL_EXPORTER_OTLP_ENDPOINT` env var in service README and CLAUDE.md

---

## Configuration

| Variable | Default | Purpose |
|----------|---------|---------|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP gRPC collector |
| `OTEL_SERVICE_NAME` | `pkdealer_service` | Service name in traces |

---

## Verification

```bash
# Start full observability stack
docker compose up -d jaeger prometheus grafana

# Start service with OTel enabled
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 cargo run --bin pkdealer_service

# Run a few hands with bot agents
cargo run --bin pkdealer_agent_random -- --name alice &
cargo run --bin pkdealer_agent_random -- --name bob &

# Check traces
open http://localhost:16686   # Jaeger — search service "pkdealer_service"

# Check metrics
open http://localhost:9090    # Prometheus — query pkdealer_hands_played
open http://localhost:3001    # Grafana — pkdealer dashboard
```

A complete hand trace should show: `hand` span → `street` spans → `action`
spans. Pot size and hands-played metrics should appear in Prometheus within
one scrape interval (default 15s).
