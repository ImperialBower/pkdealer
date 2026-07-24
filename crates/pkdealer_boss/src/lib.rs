#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
// unwrap/expect are the idiomatic failure report in tests; the ban above is
// for shipping code only (see CLAUDE.md → Error Handling).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! # `pkdealer_boss`
//!
//! The **Boss** — a collusion detector that is *blind by construction*
//! (EPIC-70). It classifies colluding seat-pairs from public information
//! alone: seats, player UUIDs, per-street actions with amounts, blinds,
//! board, and chip deltas.
//!
//! ## The typed firewall
//!
//! The detection pipeline ([`signals`], [`detector`], [`report`]) accepts
//! only [`redacted::RedactedHand`], a type with **no field that can hold a
//! hole card or deck**. [`redacted::redact`] is the single choke point that
//! consumes a [`pkcore::hand_history::HandCollection`] and drops
//! `hole_cards` and `shuffled_deck` at the boundary. Only [`scorer`] (the
//! grading tier) and [`labels`] resolution may read the full collection —
//! their output never feeds back into detection inputs.

pub mod app;
pub mod calibrate;
pub mod detector;
pub mod error;
pub mod labels;
pub mod redacted;
pub mod report;
pub mod scorer;
pub mod signals;

#[cfg(test)]
pub(crate) mod fixtures;
