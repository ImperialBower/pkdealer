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

