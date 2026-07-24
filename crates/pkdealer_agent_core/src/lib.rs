#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
// unwrap/expect are the idiomatic failure report in tests; the ban above is
// for shipping code only (see CLAUDE.md → Error Handling).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! Shared infrastructure for pkdealer bot agent binaries.
//!
//! This crate provides the common building blocks used by all three agent
//! binaries in EPIC-23:
//!
//! - [`HandState`] — the portion of table state visible to one seated agent
//! - [`Decision`] — the action an agent can choose to take
//! - [`PokerAgent`] — the async trait all agents implement
//! - [`AgentConfig`] — connection and seat parameters
//! - [`run_agent`] — connects, seats, and drives the event loop for any agent
//!
//! # Pacing
//!
//! [`run_agent`] paces the table so a spectator can follow play. Two pauses,
//! both read once at startup and applied by every agent built on this crate:
//!
//! | Variable | Default | Effect |
//! |----------|---------|--------|
//! | `PKDEALER_ACTION_DELAY_SECS`   | `1` | Pause before this seat submits each action. Only the acting agent waits, so it spaces consecutive actions across the table. Set `0` to disable. |
//! | `PKDEALER_HAND_END_DELAY_SECS` | `5` | Pause after every hand ends — showdown or fold-win alike — before the next hand starts, so viewers can see how it resolved. Set `0` to disable. |
//!
//! Values are (possibly fractional) seconds; an unparseable, negative, or
//! non-finite value falls back to the default.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use pkdealer_agent_core::{AgentConfig, Decision, HandState, PokerAgent, run_agent};
//!
//! struct AlwaysFold;
//!
//! #[async_trait::async_trait]
//! impl PokerAgent for AlwaysFold {
//!     async fn decide(&self, _state: &HandState) -> Decision {
//!         Decision::Fold
//!     }
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = AgentConfig {
//!     endpoint: "http://127.0.0.1:50051".to_string(),
//!     name: "folder".to_string(),
//!     seat: None,
//!     chips: 10_000,
//!     client_secret: String::new(),
//! };
//! run_agent(AlwaysFold, config).await?;
//! # Ok(())
//! # }
//! ```

pub mod agent;
#[cfg(feature = "collusion")]
pub mod backchannel;
pub mod error;
pub mod hand_state;
pub mod runner;

pub use agent::{AgentFidelity, Decision, PokerAgent};
pub use error::AgentError;
pub use hand_state::{HandState, SeatSnapshot, seat_state_is_active};
pub use runner::{AgentConfig, run_agent};
