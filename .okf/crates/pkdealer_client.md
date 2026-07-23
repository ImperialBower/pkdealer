---
type: Rust Crate
title: pkdealer_client
description: gRPC client binary that connects to the dealer service; also hosts the 9-player demo binary.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_client
tags: [binary, client, grpc]
timestamp: 2026-07-22T13:10:00Z
---

gRPC client for the [dealer service](/crates/pkdealer_service.md). Ships a
second binary, `demo`, used by `make demo` to run the 9-player client demo
against a running service.

# Configuration

| Variable | Default | Description |
|---|---|---|
| `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | Service endpoint to connect to |
| `PKDEALER_CLIENT_ID` | `pkdealer-client` | Client identifier sent in ping requests |
