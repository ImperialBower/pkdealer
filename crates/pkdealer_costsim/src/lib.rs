//! Offline token-accounting and notional-cost analysis over recorded pkdealer
//! arena sessions (EPIC-44 Phase 0).
//!
//! This crate is a pure *consumer* of the EPIC-25 recording sink: it reads a
//! [`pkcore::hand_history::HandCollection`] (the YAML written after every hand),
//! sums each seat's LLM token usage from the per-action
//! [`pkcore::hand_history::AgentFidelity`] provenance, and joins those counts
//! against a notional [`pricing::Pricing`] table to produce a costed
//! leaderboard. The live arena service is never touched.
//!
//! Because cost is a pure function of `(model, input_tokens, output_tokens)` —
//! all already in the hand log — the same recorded session can be re-priced
//! under several pricing scenarios without replaying a hand.

pub mod app;
/// Notional pricing and cost computation, re-exported from the shared
/// [`pkdealer_pricing`] crate so existing `pkdealer_costsim::pricing::…` paths
/// keep working after the extraction (EPIC-44).
pub use pkdealer_pricing as pricing;
pub mod report;
