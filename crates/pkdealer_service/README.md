# Poker Dealer Service

A gRPC service implementation for poker dealer functionality.

## Overview

This service provides a gRPC interface for poker game operations including:

- **Card Management**: Shuffling and dealing cards
- **Game State**: Managing poker game state and player positions
- **Rule Enforcement**: Validating game actions and enforcing poker rules
- **Session Management**: Handling multiple concurrent game sessions

## Building

```bash
# From workspace root
cargo build --package pkdealer_service

# Or from this directory
cargo build
```

## Running

```bash
# From workspace root
cargo run --bin pkdealer_service

# Or from this directory
cargo run
```

## Testing

```bash
# Run all tests
cargo test --package pkdealer_service

# Run with output
cargo test --package pkdealer_service -- --nocapture

# Note: Binary crates don't have separate library doc tests
# Doc tests in function comments are checked during regular test runs
```

## Configuration

Configuration options will be loaded from:
- Environment variables
- Configuration file (`config.toml`)
- Command-line arguments

### Environment Variables

- `PKDEALER_PORT` - Service port (default: 50051)
- `PKDEALER_HOST` - Bind address (default: 0.0.0.0)
- `PKDEALER_LOG_LEVEL` - Logging level (default: info)
- `PKDEALER_REBUY_AMOUNT` - Default chips granted when a `Rebuy` request specifies `chips == 0` (default: 10000)
- `PKDEALER_REBUY_ON_BUST_ENABLED` - When `true`, auto-reloads any seat that finished a hand with `chips == 0`, and allows the `Rebuy` RPC for seats with `chips == 0` (default: false)
- `PKDEALER_TOPUP_ENABLED` - When `true`, allows the `Rebuy` RPC for seats that still have chips; mid-hand top-ups are always rejected (default: false)
- `PKDEALER_BLIND_SCHEDULE_ENABLED` - When `true`, escalates blinds on the fixed 12-level schedule every `PKDEALER_HANDS_PER_LEVEL` hands and recycles stacks at the top (default: false)
- `PKDEALER_HANDS_PER_LEVEL` - Hands per blind level when the schedule is enabled (default: 20)

### Tournament blind schedule

When `PKDEALER_BLIND_SCHEDULE_ENABLED=true`, the service escalates blinds on a
fixed 12-level schedule (50/100 up to 3,000/6,000 — the same values as
pkarena0-web), advancing one level every `PKDEALER_HANDS_PER_LEVEL` hands
(default 20). The top level is not terminal: after it plays out its hands the
table recycles — every stack above `PKDEALER_REBUY_AMOUNT` is capped back down
to it (smaller stacks are left alone), blinds drop to 50/100, and escalation
starts over. A full cycle is `12 × PKDEALER_HANDS_PER_LEVEL` hands (240 by
default).

The flag is off by default, so plain `cargo run` and the test suite keep the
fixed 50/100 blinds. The `aiarena` and `botarena` demos enable it via
`docker-compose.yml`. Stack caps touch only the chip stack, so the per-seat
profit/loss metric stays cumulative across cycles (it steps down at each
reset).

## API

The service exposes the following gRPC methods:

- `ShuffleDeck()` - Create and shuffle a new deck
- `DealCards()` - Deal specified number of cards
- `CreateSession()` - Create a new game session
- `GetGameState()` - Query current game state

See the proto definitions for detailed API documentation.

## Development

### Prerequisites

- Rust 1.85.0 or later
- Protocol Buffers compiler (protoc)

### Code Style

This project follows the guidelines in `.github/copilot-instructions.md`:
- All public functions must have doc tests
- All public functions must have unit tests
- No `unwrap()` or `panic!()` in library code
- Comprehensive error handling

## Observability

`pkdealer_service` is instrumented with OpenTelemetry: three span kinds —
`hand`, `street`, `action` — and four metrics — `pkdealer.hands_played`,
`pkdealer.pot_size`, `pkdealer.action_duration_ms`, and
`pkdealer.ai_decision_latency_ms` (reserved for EPIC-23 agent clients).

### Quickstart (full compose stack)

From the repo root:

```bash
docker compose up -d --build
# drive a hand against the containerised service on localhost:50051
# (e.g. via grpcurl or any gRPC client)

open http://localhost:16686   # Jaeger
open http://localhost:9090    # Prometheus
open http://localhost:3001    # Grafana → "pkdealer" dashboard
```

### Host dev (faster iteration on the service)

```bash
docker compose up -d otel-collector jaeger prometheus grafana
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317 \
  cargo run --bin pkdealer_service
```

### Env vars

| Var | Default | Purpose |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTLP gRPC target |
| `OTEL_SERVICE_NAME` | `pkdealer_service` | `service.name` resource attribute |
| `OTEL_SDK_DISABLED` | unset | If `true`, skips OTel init entirely (useful in tests/CI) |
| `RUST_LOG` | `pkdealer_service=info,info` | tracing filter |

## License

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](../../LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
* MIT license ([LICENSE-MIT](../../LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.

