---
type: Rust Crate
title: pkdealer_agent_boss
description: Live blind collusion detector — polls ExportSession, redacts at ingest, emits per-pair SPRT verdicts over OpenTelemetry (EPIC-70 Phase 4).
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_agent_boss
tags: [binary, library, collusion, detection, sprt, observer, opentelemetry, epic-70]
timestamp: 2026-08-30T12:00:00Z
---

The **live** counterpart to the offline [pkdealer_boss](/crates/pkdealer_boss.md)
analyzer. It joins the arena like any other process but never takes a seat: it
polls the dealer's `ExportSession` RPC (see
[Dealer gRPC API](/interfaces/dealer-grpc-api.md)) on the completed-hand
watermark cadence, redacts every export at ingest, runs the blind SPRT
detector, and emits per-pair verdicts as structured logs plus OTel spans and
metrics (see [Observability](/runbooks/observability.md)).

Detection logic is *not* duplicated — the crate depends on `pkdealer_boss` and
shares its `RedactedHand` choke point verbatim. The only additions here are the
gRPC poll loop (`app`) and the OTel instrument set (`otel`).

# Modules

| Module | Role |
|--------|------|
| `app` | Poll loop, ingest-time redaction, trust-boundary discussion |
| `otel` | Tracer/meter setup and the per-pair verdict instruments |

# Trust boundary

The live path needs a spectator token to call `ExportSession`, so it is blind
by construction but not blind by *deployment*. Where provable blindness matters,
prefer the offline `pkdealer_boss` analyzer, which reads recorded sessions and
needs no token at all.

**Validation status:** authored but not yet exercised against a live arena; the
decision pieces are unit-tested, the poll loop is not covered end to end. See
[EPIC docs](/references/epic-docs.md) for EPIC-70.
