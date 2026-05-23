#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
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
pub mod error;
pub mod hand_state;
pub mod runner;

pub use agent::{Decision, PokerAgent};
pub use error::AgentError;
pub use hand_state::HandState;
pub use runner::{AgentConfig, run_agent};
