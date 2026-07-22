---
type: Rust Crate
title: pkdealer_proto
description: Shared protobuf definitions and tonic-generated Rust types for pkdealer.
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_proto
tags: [library, protobuf, grpc]
timestamp: 2026-07-22T13:10:00Z
---

Library crate holding the single protobuf schema (`proto/dealer.proto`) and
the Rust types tonic generates from it. Every other crate in the workspace —
the [service](/crates/pkdealer_service.md), the
[client](/crates/pkdealer_client.md), and all agents via
[pkdealer_agent_core](/crates/pkdealer_agent_core.md) — depends on this crate
for the wire contract described in
[Dealer gRPC API](/interfaces/dealer-grpc-api.md).

Code generation runs in `build.rs` via `tonic-build` using
`protoc-bin-vendored`, so no system `protoc` installation is required.
