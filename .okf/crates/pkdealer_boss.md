---
type: Rust Crate
title: pkdealer_boss
description: The Boss — blind collusion detection over redacted pkdealer arena sessions (EPIC-70).
resource: https://github.com/ImperialBower/pkdealer/tree/main/crates/pkdealer_boss
tags: [binary, library, collusion, detection, sprt, epic-70]
timestamp: 2026-07-23T12:00:00Z
---

Offline collusion detector (EPIC-70 Phase 2) that reads recorded arena sessions
(YAML from the EPIC-25 disk sink, or JSON from the `ExportSession` RPC — see
[Dealer gRPC API](/interfaces/dealer-grpc-api.md)) and classifies colluding
seat-pairs from **public information alone**.

Its detection pipeline (`signals`, `detector`, `report`) accepts only the
`RedactedHand` type, which has no field that can hold a hole card — the
`redact()` choke point drops `hole_cards` and the deck at the boundary, so "the
Boss cannot peek" is enforced by the type system. Per-pair evidence accumulates
as a Wald **SPRT** log-likelihood ratio over three signals (chip-flow
asymmetry, soft-play index, whipsaw count), and a pair is flagged the first
hand it crosses the upper bound with a `Confidence` sample-size floor met.

A separate, import-isolated `scorer` module (the only card-aware code) grades a
run against a `GroundTruthLabels` sidecar: hands-to-detection, false-positive
rate, and an EV-sacrifice oracle upper bound. See
[EPIC docs](/references/epic-docs.md) for the design spec.
