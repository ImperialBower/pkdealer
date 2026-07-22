---
type: Rust Crate
title: pkdealer_service
description: The gRPC dealer server binary — implements DealerService and manages the poker table.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_service
tags: [binary, service, grpc, otel]
timestamp: 2026-07-22T13:10:00Z
---

Binary crate implementing `DealerService` (see
[Dealer gRPC API](/interfaces/dealer-grpc-api.md)): seating, dealing,
action processing, street advancement, showdown resolution, event streaming,
and session export. It is OpenTelemetry-instrumented — see the
[observability runbook](/runbooks/observability.md).

# Configuration

| Variable | Default | Description |
|---|---|---|
| `PKDEALER_ADDR` | `127.0.0.1:50051` | Address the service listens on |
| `OTEL_SDK_DISABLED` | unset | Set `true` to run without a collector (tests, bare `cargo run`) |

The crate's own `README.md` documents the full environment-variable set and
observability quickstart.

# Examples

```sh
cargo run -p pkdealer_service                       # default bind
PKDEALER_ADDR=0.0.0.0:9090 cargo run -p pkdealer_service
cargo test -p pkdealer_service --test e2e_ping      # end-to-end ping test
```
