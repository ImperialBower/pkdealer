# Demo: EPIC-22 — OpenTelemetry instrumentation, end to end

> An autonomous poker hand played through a containerised gRPC service
> produces a trace tree in Jaeger, populates four metrics in Prometheus,
> and renders live in a Grafana dashboard — all wired through an
> OpenTelemetry collector, brought up with one `docker compose up`.

## Audience & framing

Default: technical reviewers / engineering peers familiar with the
observability stack and curious about how the pkdealer platform makes
itself inspectable. Surfaces the design choices (collector-in-the-middle,
W3C TraceContext on the gRPC server side, span lifecycle across
async-handler boundaries) without dwelling on Rust syntax.

Adjust if presenting to a less technical audience: skip the talking
points about `tracing::Layer` plumbing and the action-span parent
fallback; emphasise the three-tab visual story instead.

## Prerequisites

- Docker Desktop running (4 GB+ free memory).
- Repo at `~/src/github.com/ImperialBower/pkdealer`, on branch `epic-22`
  (or `main` if merged by demo time): `git checkout epic-22`.
- Host ports free: `50051`, `4317`, `8889`, `16686`, `9090`, `3001`.
  Check with `lsof -nP -iTCP -sTCP:LISTEN | grep -E '50051|4317|8889|16686|9090|3001'`
  — should print nothing.
- `grpcurl` installed (`brew install grpcurl` if not).
- Three browser tabs pre-opened (don't load yet — they show nothing
  before the demo):
  1. Jaeger UI: <http://localhost:16686>
  2. Prometheus: <http://localhost:9090>
  3. Grafana dashboard: <http://localhost:3001/d/pkdealer>
- Terminal with two panes:
  - Left: this runbook open, `cd` into the repo.
  - Right: reserved for the `docker compose logs -f pkdealer_service`
    stream during the demo so the audience sees the service narrating
    the hand.

> Smoke status: the demo path was verified end-to-end during EPIC-22
> Task 15 (Jaeger trace appeared, `pkdealer_hands_played_total = 1`,
> Grafana dashboard rendered six panels). For belt-and-suspenders,
> run a dress rehearsal once before the live demo — see "Dress
> rehearsal" at the bottom.

## Setup (~5 minutes)

1. **Clean slate (idempotent)**
   ```bash
   docker compose down
   ```
   _Expected:_ "Removing... done" for any stale containers, or silent if none.
   _Talking point:_ "Starting from zero so the demo isn't carrying yesterday's state."

2. **Bring up the full stack**
   ```bash
   docker compose up -d --build
   ```
   _Expected:_ five "Started" lines for `pkdealer_service`, `otel-collector`, `jaeger`, `prometheus`, `grafana`. ~4 min cold, ~30 s warm.
   _Talking point:_ "One file describes the whole observability stack — service plus collector plus three backends."

3. **Confirm all five services are up**
   ```bash
   docker compose ps
   ```
   _Expected:_ five rows, every `STATUS` reading "Up" or "running".
   _Talking point:_ "Service binds to 0.0.0.0 inside the container so the host can reach it."

4. **Probe each backend's HTTP endpoint**
   ```bash
   curl -sf http://localhost:16686/api/services > /dev/null && echo "jaeger ok"
   curl -sf http://localhost:9090/-/ready          > /dev/null && echo "prom ok"
   curl -sf http://localhost:3001/api/health       > /dev/null && echo "grafana ok"
   ```
   _Expected:_ three "ok" lines.
   _Talking point:_ "All three observability backends are healthy before we generate any telemetry."

5. **Tail the service logs in the right pane**
   ```bash
   docker compose logs -f pkdealer_service
   ```
   _Expected:_ "Poker Dealer Service v0.1.7" + "Starting gRPC server on 0.0.0.0:50051..."
   _Talking point:_ "Leaving this open so the audience sees the service narrate the hand in real time."

## The demo (~5–7 minutes)

### 1. Show the empty stack

1. **Open Jaeger** at <http://localhost:16686> in the prepared tab.
   _Expected:_ "Service" dropdown shows only `jaeger-all-in-one` — no `pkdealer_service` yet.
   _Talking point:_ "No traces yet — the service has nothing to report until a hand is played."

### 2. Drive a heads-up hand via grpcurl

The `pkdealer_client` `demo` example currently drives an in-process pkcore simulation rather than the gRPC service (flagged for follow-up). Use `grpcurl` directly for now; it's also more honest as a demo — the audience sees each RPC.

> The service exposes gRPC reflection (both v1 and v1alpha), so grpcurl
> can resolve message types over the wire — no proto-file flag needed.
> The fully-qualified service path is `pkdealer.dealer.v1.DealerService`.
> Optional warm-up: `grpcurl -plaintext localhost:50051 list` prints
> `grpc.reflection.v1.ServerReflection` and `pkdealer.dealer.v1.DealerService`.

2. **Seat Alice**
   ```bash
   grpcurl -plaintext \
     -d '{"name":"Alice","chips":10000}' \
     localhost:50051 pkdealer.dealer.v1.DealerService/SeatPlayer
   ```
   _Expected:_ JSON with `seatNumber: 0` and a UUID `playerToken`.
   _Talking point:_ "Each seat returns a token — that's the auth handle for that player's `Act` calls."
   _Save the returned token as `ALICE`._

3. **Seat Bob** (same shape, change the name)
   ```bash
   grpcurl -plaintext \
     -d '{"name":"Bob","chips":10000}' \
     localhost:50051 pkdealer.dealer.v1.DealerService/SeatPlayer
   ```
   _Expected:_ JSON with `seatNumber: 1` and a UUID `playerToken`.
   _Save as `BOB`._

4. **Start the hand**
   ```bash
   grpcurl -plaintext \
     -d '{}' \
     localhost:50051 pkdealer.dealer.v1.DealerService/StartHand
   ```
   _Expected:_ JSON with `status` containing two seats, board empty, pot=150 (small blind 50 + big blind 100).
   _Talking point:_ "Blinds posted; hole cards dealt. In the service log you can see the `hand` span open."

5. **Heads-up check-down — 8 `Act` calls** (alternate seats, action types
   chosen so the hand reaches showdown without anyone folding)
   ```bash
   # Replace <TOKEN> with the next-to-act seat's token; replace <SEAT> with 0 or 1.
   # action_type: 1=BET 2=CALL 3=CHECK 4=RAISE 5=ALL_IN 6=FOLD
   grpcurl -plaintext \
     -H "x-player-token: <TOKEN>" \
     -d '{"action":{"seat":<SEAT>,"actionType":2,"amount":0}}' \
     localhost:50051 pkdealer.dealer.v1.DealerService/Act
   ```
   Sequence: SB calls (action_type 2), BB checks (3), then 6× check (3) through flop / turn / river.

   _Expected last call:_ JSON with `handComplete: true` and a `handResult` block.
   _Talking point (during the loop):_ "Each Act extracts `traceparent` from gRPC metadata — when EPIC-23 agents send one, the action span nests under their decision span."

6. **Watch the service log narrate the hand** (right pane)
   _Expected:_ four "Street advanced." lines (preflop → flop → turn → river) followed by one "Hand ended." line.
   _Talking point:_ "Auto-advance happens inside `Act` — the service runs streets through to showdown with no client orchestration."

### 3. The trace

7. **Refresh Jaeger's service dropdown**
   _Expected:_ `pkdealer_service` now appears.
   _Talking point:_ "OTLP spans pushed to the collector, which forwarded to Jaeger."

8. **Click "Find Traces", select the most recent**
   _Expected:_ a trace named `start_hand` or `hand` ~milliseconds long, containing a nested tree.
   _Talking point:_ "One trace per hand."

9. **Expand the trace**
   _Expected:_ `hand` at the top → four `street` spans → ~8 `action` spans nested under them.
   _Talking point:_ "The hand span lives on `TableState` across separate gRPC calls — that's the design twist. Without it each Act would be a root span."
   _Point at:_ one `action` span → expand attributes → show `seat`, `action_type`, `amount`, `pot_after`, `linked_hand_trace`.

### 4. The metric

10. **Open Prometheus** at <http://localhost:9090>.

11. **Query the hands-played counter**
    ```
    pkdealer_hands_played_total
    ```
    _Expected:_ one row with value `1`, labelled `service_name="pkdealer_service"`.
    _Talking point:_ "Pushed to the collector via OTLP, scraped by Prometheus from the collector's Prometheus exporter at port 8889 every 15 seconds."

12. **Query the pot-size histogram**
    ```
    pkdealer_pot_size_bucket
    ```
    _Expected:_ several `le` buckets with one observation each — the final pot landed in the appropriate bucket.
    _Talking point:_ "Histogram bucket boundaries are SDK defaults. Tuning lives in Grafana, not in the code."

### 5. The dashboard

13. **Open Grafana** at <http://localhost:3001/d/pkdealer>.
    _Expected:_ a "pkdealer" dashboard with six panels — Hands played (per min), Pot size distribution, Action latency p50/p95/p99, Actions by type, AI decision latency (EPIC-23 placeholder, no data), and a Jaeger link panel.
    _Talking point:_ "Datasources are file-provisioned, so the dashboard works on first boot — no Grafana clicking to set up."

14. **Click the Jaeger link** in the bottom panel.
    _Expected:_ Jaeger search page pre-filtered to `pkdealer_service`.
    _Talking point:_ "Closes the loop — dashboard alert leads directly to the trace that explains it."

## What to highlight verbally

- **One OTLP export path in the service; fanout in the collector.** Rust code doesn't know or care that Jaeger and Prometheus exist — it pushes one stream to `otel-collector:4317`. Swapping in Honeycomb or Grafana Tempo later is a collector-config change, not a code change.
- **W3C TraceContext propagator is wired on the server side TODAY** even though no agent injects `traceparent` yet. When EPIC-23 lands, the agent's decision span and the service's action span will nest with zero further service changes.
- **The `hand` span survives across separate gRPC calls** by living on `TableState` (behind the same mutex that guards the game state). That's the design tension worth showing: tonic handlers are request-scoped, but game progression is stateful. The span field is the bridge.
- **`ai_decision_latency_ms` is declared but reserved.** The instrument exists; the dashboard panel exists; the service never records into it. EPIC-23 agents will. We picked the metric name today so EPIC-23's PR is purely additive — no dashboard churn.
- **`OTEL_SDK_DISABLED=true` skips OTel init entirely.** Important for tests/CI: with it set, no global subscriber is installed, so test-local subscribers work cleanly. Earlier the disabled path installed a fmt subscriber as a side effect — that broke parallel test isolation and we removed it.

## Likely questions & answers

**Why a collector in the middle? Why not push to Prometheus directly?**
Prometheus is pull-based; pushing to it requires the experimental remote-write API. The OTel Collector exposes a Prometheus exporter that Prometheus can scrape — same model as scraping any other target. We also get a swap-point for filtering/sampling/routing later, and one OTLP path in-process keeps service code simple.

**What happens to the in-process `hand` → `action` tree when an EPIC-23 agent injects `traceparent`?**
The action span becomes a child of the agent's decision span (parented via remote context), so the agent → service nesting becomes the primary trace. We preserve the in-process hand context as a `linked_hand_trace` field on the action span — a workaround for `tracing-opentelemetry`'s lack of an exposed span-link API at the pinned SDK version. A proper span-link can replace it when the SDK matures.

**Why grpcurl instead of `cargo run --example demo -p pkdealer_client`?**
The `demo` example currently drives an in-process pkcore simulation rather than the containerised gRPC service. That's a discrepancy with how it was introduced in the recent commit history; it's flagged as a follow-up. grpcurl is also more honest as a demo — the audience sees every RPC by name.

**How much delay between the hand ending and the metric showing up in Prometheus?**
Up to one scrape interval — currently 15 seconds. Trace export is push-based via the collector's batched OTLP exporter, so traces appear in Jaeger almost immediately (sub-second). Metrics lag a bit because of the pull model. The Grafana stat panel shows hands-per-minute as a `rate()`, so the very first hand needs the second scrape interval before the rate has signal.

**What's NOT in this epic?**
Langfuse, `gen_ai.*` semantic conventions, a published container image, sampling configuration, multi-table or per-table metric labels, and any agent-side telemetry. All of that is EPIC-23 (agent observability) and EPIC-24 (demo packaging). EPIC-22 ships the foundation; the next two epics build on it.

## Cleanup

```bash
docker compose down
```
_Expected:_ "Removing... done" for all five containers. No volumes configured, nothing else to undo.

If you opened any terminal panes specifically for this demo, close them. The browser tabs are harmless to leave open.

To return to the working branch (if you switched to `epic-22` for the demo):
```bash
git checkout <your-branch>
```

## Troubleshooting

- **`pkdealer_service` not appearing in Jaeger after a hand.** The service may have started before the collector accepted OTLP. `depends_on` in compose is start-order-only, not health-aware. Fix: `docker compose restart pkdealer_service`, then re-drive a hand.
- **Grafana panels showing "No data".** Prometheus needs one scrape interval (~15 s) to register the first sample, and the `rate()` quantile panels need two. Wait 30 s after the hand completes and refresh.
- **`docker compose up` fails with "image not found: jaegertracing/all-in-one:1.62".** This shouldn't happen — compose uses `:latest` because the `1.62` tag doesn't exist on Docker Hub. If you see this, pull the latest of the file: `git checkout epic-22 -- docker-compose.yml`.
- **`grpcurl: command not found`.** `brew install grpcurl`.
- **`server does not support the reflection API`.** Means you're running an older build of the service image without `tonic-reflection`. `docker compose up -d --build pkdealer_service` to pick up the current code. As a one-off fallback, you can also point grpcurl at the proto: `-import-path proto -proto dealer.proto`.

## Dress rehearsal

Run this 15 minutes before the live demo to catch image drift, network issues, or stale state:

```bash
git checkout epic-22 && \
docker compose down && \
docker compose up -d --build && \
sleep 5 && \
curl -sf http://localhost:16686/api/services -o /dev/null && echo "jaeger ok" && \
curl -sf http://localhost:9090/-/ready          -o /dev/null && echo "prom ok"  && \
curl -sf http://localhost:3001/api/health       -o /dev/null && echo "grafana ok"
```

If all three probes print "ok", you're ready. Then `docker compose down` and bring it back up at demo time.
