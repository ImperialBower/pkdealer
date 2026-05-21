# EPIC-22 — OpenTelemetry Instrumentation (Design)

**Status:** design — awaiting implementation plan
**Date:** 2026-05-20
**Owner:** pkdealer
**Epic doc:** [`docs/EPIC-22_OTel.md`](../../EPIC-22_OTel.md)

---

## Goal

Make every game event in `pkdealer_service` observable via OpenTelemetry —
hand-level traces in Jaeger, four service metrics in Prometheus, a
hand-authored Grafana dashboard, and W3C TraceContext propagation across
gRPC so future agent clients (EPIC-23) nest naturally under service spans.

This epic is the foundation for the platform's observability story. It does
**not** ship a containerised service (EPIC-24), agent decision spans
(EPIC-23), or Langfuse integration (EPIC-23 / EPIC-24).

---

## Scope decisions (locked during brainstorming)

| Decision | Choice | Rationale |
|---|---|---|
| Slice | Full EPIC-22 in one design (instrumentation + compose + dashboard) | Cohesive value — the dashboard validates the metrics, and one design keeps cross-cutting concerns visible. |
| Metrics path | OTel Collector in the middle | One OTLP export path in-process; collector fans out to Jaeger (traces) and Prometheus exporter (metrics). |
| gRPC propagation | Server-side W3C TraceContext extraction wired now | Service is ready for EPIC-23 agents with zero further service changes; falls back to a fresh root when no header is present (current state). |

---

## Architecture

```
 ┌──────────────────────────┐         OTLP gRPC :4317        ┌────────────────────┐
 │ agent (EPIC-23, future)  │ ───────────────────────────▶   │                    │
 │ injects `traceparent`    │                                │   otel-collector   │
 └──────────────┬───────────┘                                │                    │
                │ gRPC Act (metadata: traceparent)           │  receivers: otlp   │
                ▼                                            │  exporters:        │
 ┌──────────────────────────┐                                │   • prometheus     │
 │ pkdealer_service         │                                │     :8889/metrics  │
 │  • W3C TraceContext      │  push traces + metrics OTLP    │   • otlp/jaeger    │
 │    propagator extracts   │ ─────────────────────────────▶ └──┬───────────┬─────┘
 │    parent context        │                                   │           │
 │  • hand / street / action│                                   ▼           ▼
 │    spans                 │                            ┌──────────┐  ┌─────────┐
 │  • 4 metric instruments  │                            │ jaeger   │  │promtheus│
 └──────────────────────────┘                            │ :16686   │  │ :9090   │
                                                                            │
                                                                            ▼
                                                                       ┌─────────┐
                                                                       │ grafana │
                                                                       │ :3001   │
                                                                       └─────────┘
```

`pkdealer_service` runs on the host (via `cargo run`) and pushes OTLP over
gRPC to `otel-collector` in compose. Backend containers are
`otel-collector`, `jaeger`, `prometheus`, `grafana`. No service container in
this epic — that's EPIC-24's job.

---

## Crate dependencies

`crates/pkdealer_service/Cargo.toml`:

```toml
tracing                            = "0.1"
tracing-subscriber                 = { version = "0.3", features = ["env-filter", "fmt"] }
tracing-opentelemetry              = "0.31"
opentelemetry                      = "0.30"
opentelemetry_sdk                  = { version = "0.30", features = ["rt-tokio"] }
opentelemetry-otlp                 = { version = "0.30", features = ["grpc-tonic", "metrics", "trace"] }
opentelemetry-semantic-conventions = "0.30"

[dev-dependencies]
tracing-test = "0.2"
```

Versions are the current `0.30.x` family. The implementation plan locks
exact patch versions at execution time.

---

## Initialization (`pkdealer_service/src/otel.rs`, new module)

```rust
pub struct OtelGuards {
    tracer_provider: SdkTracerProvider,
    meter_provider:  SdkMeterProvider,
}

impl Drop for OtelGuards {
    fn drop(&mut self) {
        let _ = self.tracer_provider.shutdown();
        let _ = self.meter_provider.shutdown();
    }
}

pub fn init_otel() -> Result<Option<OtelGuards>, Box<dyn Error>> {
    if std::env::var("OTEL_SDK_DISABLED").as_deref() == Ok("true") {
        // No-op: install plain fmt subscriber, return None
        return Ok(None);
    }

    // 1. Install global W3C TraceContext propagator.
    global::set_text_map_propagator(TraceContextPropagator::new());

    // 2. Build OTLP tonic exporter.
    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:4317".to_owned());
    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .unwrap_or_else(|_| "pkdealer_service".to_owned());
    let resource = Resource::builder()
        .with_service_name(service_name)
        .build();

    // 3. Tracer provider + tracing-opentelemetry layer.
    let span_exporter = SpanExporter::builder().with_tonic()
        .with_endpoint(&endpoint).build()?;
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter)
        .build();
    let tracer = tracer_provider.tracer("pkdealer_service");

    // 4. Meter provider with periodic OTLP push.
    let metric_exporter = MetricExporter::builder().with_tonic()
        .with_endpoint(&endpoint).build()?;
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(metric_exporter)
        .build();
    global::set_meter_provider(meter_provider.clone());

    // 5. tracing-subscriber: env-filter + fmt + OTel layer.
    Registry::default()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_|
            EnvFilter::new("pkdealer_service=info,info")))
        .with(fmt::layer())
        .with(OpenTelemetryLayer::new(tracer))
        .init();

    Ok(Some(OtelGuards { tracer_provider, meter_provider }))
}
```

If `init_otel` returns `Err`, `main` logs a warning and continues with a
plain `fmt` subscriber so local dev without a collector still works.

### Config

| Var | Default | Purpose |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP gRPC target |
| `OTEL_SERVICE_NAME` | `pkdealer_service` | `service.name` resource attr |
| `RUST_LOG` | `pkdealer_service=info,info` | tracing filter |
| `OTEL_SDK_DISABLED` | unset | If `true`, skip OTel init entirely (tests/CI) |

---

## Span instrumentation

### `TableState` additions (`crates/pkdealer_service/src/main.rs:103`)

```rust
struct TableState {
    session: PokerSession,
    token_to_seat: HashMap<Uuid, u8>,
    seat_to_token: HashMap<u8, Uuid>,
    current_hand_span:   Option<tracing::Span>,
    current_street_span: Option<tracing::Span>,
    hand_started_at:     Option<std::time::Instant>,
    last_prompt_at:      Option<std::time::Instant>,
}
```

### `hand` span — opened in `start_hand` handler

Attributes set at open: `hand_id` (fresh UUID), `player_count`,
`starting_pot`, `button_seat`.

Closed inside the `act` handler when `SessionStep::HandComplete` fires
(main.rs:712). At close, final attributes `winning_seats` and `final_pot`
are recorded via `Span::record`.

### `street` span — opened on `SessionStep::StreetAdvanced` (main.rs:702)

Parent is the current `hand` span. Attributes: `street_name` (mapped from
`GamePhase`) and `board_cards` (rendered board string). The previous
street's span is dropped before the new one opens.

### `action` span — opened in `act` handler

Parent-selection rule (must be explicit; `tracing`'s default parent is
the currently-entered span, but the `act` handler is *not* entered
inside `current_hand_span` — that span lives on `TableState`, not on the
async task stack):

1. Extract `parent_cx` from the incoming `traceparent` via
   `MetadataExtractor`.
2. If `parent_cx` carries a valid span context (agent present):
   call `action_span.set_parent(parent_cx)`, and **add a link** to the
   in-process `hand` span so Jaeger can cross-reference both traces.
3. Otherwise (no agent / current state): explicitly parent the action
   span under `current_street_span` (which is itself parented to
   `current_hand_span`).

Pseudocode:

```rust
let parent_cx = global::get_text_map_propagator(|p| {
    p.extract(&MetadataExtractor(request.metadata()))
});

let action_span = if parent_cx.span().span_context().is_valid() {
    // Agent path: parent = agent's decision span; link = local hand span.
    let span = tracing::info_span!("action", /* fields = Empty */);
    span.set_parent(parent_cx);
    if let Some(hand) = &state.current_hand_span {
        span.add_link(hand.context().span().span_context().clone());
    }
    span
} else {
    // Service-internal path: parent = current_street_span.
    let parent = state.current_street_span.as_ref();
    tracing::info_span!(parent: parent, "action", /* fields = Empty */)
};
let _enter = action_span.enter();
```

When an agent (EPIC-23) sends `traceparent`, the action span becomes a
child of the agent's decision span. The added link to the `hand` span
preserves the in-process view in Jaeger as a "related trace" indicator.

### `MetadataExtractor`

~15-line newtype around `tonic::metadata::MetadataMap` implementing
`opentelemetry::propagation::Extractor`. Standard boilerplate.

---

## Metrics

`Arc<Metrics>` stored on `DealerService`:

```rust
struct Metrics {
    hands_played:           Counter<u64>,
    pot_size:               Histogram<u64>,
    action_duration_ms:     Histogram<f64>,
    ai_decision_latency_ms: Histogram<f64>,   // EPIC-23 agents emit; service does not
}
```

| Metric | Where recorded | Attributes |
|---|---|---|
| `pkdealer.hands_played` | `act` handler on `HandComplete` | — |
| `pkdealer.pot_size` | same site, before payout | — |
| `pkdealer.action_duration_ms` | top of `act` handler, `now() - last_prompt_at` | `action_type`, `seat` |
| `pkdealer.ai_decision_latency_ms` | **not emitted by service** — reserved for EPIC-23 | `agent_type` |

`last_prompt_at` is updated whenever the auto-advance loop emits a
`NextActor` event. Histogram bucket boundaries use SDK defaults
(exponential); tuning lives in collector config or Grafana, not in code.
No SDK View configuration in v1.

---

## Compose stack

New top-level files in pkdealer/:

```
pkdealer/
├── docker-compose.yml                            (new)
└── ops/                                          (new)
    ├── otel-collector.yaml
    ├── prometheus.yml
    └── grafana/
        ├── provisioning/
        │   ├── datasources/datasources.yml
        │   └── dashboards/dashboards.yml
        └── dashboards/
            └── pkdealer.json
```

### `docker-compose.yml`

```yaml
services:
  otel-collector:
    image: otel/opentelemetry-collector-contrib:0.115.1
    command: ["--config=/etc/otel-collector.yaml"]
    volumes: ["./ops/otel-collector.yaml:/etc/otel-collector.yaml:ro"]
    ports:
      - "4317:4317"   # OTLP gRPC in
      - "8889:8889"   # Prometheus exporter out

  jaeger:
    image: jaegertracing/all-in-one:1.62
    environment:
      COLLECTOR_OTLP_ENABLED: "true"
    ports:
      - "16686:16686" # UI
      - "14317:4317"  # internal-only OTLP (collector → jaeger)

  prometheus:
    image: prom/prometheus:v2.55.1
    volumes: ["./ops/prometheus.yml:/etc/prometheus/prometheus.yml:ro"]
    ports: ["9090:9090"]

  grafana:
    image: grafana/grafana:11.3.1
    environment:
      GF_AUTH_ANONYMOUS_ENABLED: "true"
      GF_AUTH_ANONYMOUS_ORG_ROLE: "Admin"
    volumes:
      - ./ops/grafana/provisioning:/etc/grafana/provisioning:ro
      - ./ops/grafana/dashboards:/var/lib/grafana/dashboards:ro
    ports: ["3001:3000"]
```

### `ops/otel-collector.yaml`

```yaml
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317

exporters:
  otlp/jaeger:
    endpoint: jaeger:4317
    tls: { insecure: true }
  prometheus:
    endpoint: 0.0.0.0:8889
    resource_to_telemetry_conversion: { enabled: true }

processors:
  batch: {}

service:
  pipelines:
    traces:
      receivers:  [otlp]
      processors: [batch]
      exporters:  [otlp/jaeger]
    metrics:
      receivers:  [otlp]
      processors: [batch]
      exporters:  [prometheus]
```

### `ops/prometheus.yml`

```yaml
global: { scrape_interval: 15s }
scrape_configs:
  - job_name: otel-collector
    static_configs: [{ targets: ["otel-collector:8889"] }]
```

### Grafana dashboard (`ops/grafana/dashboards/pkdealer.json`)

Hand-authored JSON (not UI-exported — smaller diff, reviewable). Panels:

1. **Hands played** — `rate(pkdealer_hands_played_total[1m])` stat
2. **Pot size distribution** — `pkdealer_pot_size_bucket` heatmap
3. **Action latency p50/p95/p99** — `histogram_quantile` over
   `pkdealer_action_duration_ms_bucket`
4. **Actions by type** — `sum by (action_type)
   (rate(pkdealer_action_duration_ms_count[1m]))` stacked area
5. **AI decision latency by agent** — placeholder until EPIC-23
6. **Trace lookup** — text link to
   `http://localhost:16686/search?service=pkdealer_service`

Datasources are provisioned (Prometheus at `http://prometheus:9090`,
Jaeger at `http://jaeger:16686/jaeger/ui`) so Grafana works on first
boot.

---

## Testing

| Test | File | Asserts |
|---|---|---|
| `init_otel_with_disabled_flag_is_noop` | `crates/pkdealer_service/src/otel.rs` | `OTEL_SDK_DISABLED=true` returns `Ok(None)` and spans still construct |
| `metadata_extractor_round_trips_traceparent` | same | `MetadataExtractor` extracts a known `trace_id` from injected header |
| `action_span_inherits_agent_context` | `crates/pkdealer_service/tests/otel_propagation.rs` | Build `Request<ActRequest>` with `traceparent`; capture span via `tracing-test`; assert parent matches |
| `hand_span_spans_full_hand_lifecycle` | same | Drive a 2-player hand; capture spans; assert one `hand` containing N `action` spans |

`tracing-test` provides in-process span capture. No real OTLP exporter
is used in tests.

---

## Verification (manual)

```bash
docker compose up -d
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
  cargo run --bin pkdealer_service
cargo run --example demo -p pkdealer_client

open http://localhost:16686   # Jaeger: service "pkdealer_service"
open http://localhost:9090    # Prometheus: pkdealer_hands_played_total
open http://localhost:3001    # Grafana: "pkdealer" dashboard
```

Complete hand trace shows `hand` → `street` × N → `action` × N. Metrics
appear in Prometheus within one scrape interval (≤15s).

---

## Documentation updates

- `crates/pkdealer_service/README.md` — new "Observability" section
- `CLAUDE.md` — append short "Observability" note pointing at `ops/`
- `DEVLOG.md` — append EPIC-22 section mirroring EPIC-20/21 structure
- `docs/EPIC-22_OTel.md` — flip all "Planned" → "Complete"; correct
  stale OTel SDK version reference; replace direct-Prometheus
  assumption with the collector pipeline

---

## Work breakdown

1. Add OTel deps to `crates/pkdealer_service/Cargo.toml`
2. Create `crates/pkdealer_service/src/otel.rs` — `init_otel`,
   `OtelGuards`, `MetadataExtractor`
3. Wire `init_otel` into `main`; install fallback fmt subscriber on error
4. Add `current_hand_span`, `current_street_span`, `hand_started_at`,
   `last_prompt_at` fields to `TableState`
5. Open `hand` span in `start_hand`; close on `HandComplete` in `act`
6. Open/close `street` span on `StreetAdvanced` in `act`'s auto-advance
   loop
7. Build `Arc<Metrics>` at startup; record `hands_played`, `pot_size`,
   `action_duration_ms` at the documented sites
8. Add `action` span in `act` handler; extract `traceparent` via
   `MetadataExtractor`; link to current `hand` span
9. Write `docker-compose.yml`, `ops/otel-collector.yaml`,
   `ops/prometheus.yml`
10. Write `ops/grafana/dashboards/pkdealer.json` and provisioning files
11. Write tests listed above
12. Update `crates/pkdealer_service/README.md`, `CLAUDE.md`, `DEVLOG.md`,
    `docs/EPIC-22_OTel.md`

---

## Out of scope

- `pkdealer_service` Docker image (EPIC-24)
- Langfuse / `gen_ai.*` semantic conventions (agent-emitted; EPIC-23)
- Sampling configuration (defaults until volume warrants tuning)
- Multi-table or per-table metric labels (single-table today)
- Log export to OTLP (stdout `fmt` layer is enough for v1)

---

## Open risks

- **OTel Rust SDK version churn.** The 0.x line has reshaped its API
  several times. Implementation plan must lock exact patch versions and
  run a smoke build before merging. If the public API at implementation
  time differs from this design's pseudocode, the *shape* is the
  authoritative part — keep the same span hierarchy, propagator, and
  metric instruments even if the calls look different.
- **Action-span detachment under agent context.** When EPIC-23 ships,
  `action` spans nest under agent spans, not under the in-process
  `hand` span. Span-links preserve the cross-reference, but Jaeger's
  default view still presents two separate traces. Acceptable; revisit
  if it impedes debugging.
