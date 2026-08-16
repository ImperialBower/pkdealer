#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
// unwrap/expect are the idiomatic failure report in tests; the ban above is for
// shipping code only (see CLAUDE.md → Error Handling).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! # `pkdealer_agent_boss` — the live blind collusion detector (EPIC-70 Phase 4)
//!
//! An observer process that sits in the arena like any other agent but never
//! takes a seat. It polls the dealer's `ExportSession` on the completed-hand
//! watermark cadence, [`redacts`](app::ingest) each export at ingest, runs the
//! blind [`pkdealer_boss`] SPRT detector, and emits per-pair verdicts as
//! structured logs plus OpenTelemetry ([`otel`]).
//!
//! It shares the *offline* Boss's detection library verbatim — the only
//! additions here are the live gRPC poll loop and the `OTel` instrument set. For a
//! provably-blind path prefer the offline `pkdealer_boss` analyzer, which needs
//! no spectator token; see [`app`] for the trust-boundary discussion.
//!
//! ## Validation status
//!
//! Authored but **not run against a live arena** in the session that created it
//! (that needs a multi-container `docker compose` stack). The pure decision
//! pieces are unit-tested; the poll loop is not exercised end-to-end.

pub mod app;
pub mod otel;
