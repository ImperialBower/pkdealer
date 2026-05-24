#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! # `pkdealer_agent_llm`
//!
//! Shared building blocks for LLM-backed poker agents.
//!
//! Each LLM-backed agent crate (`pkdealer_agent_claude`, `pkdealer_agent_ollama`,
//! …) provides its own [`LlmBackend`] implementation that owns HTTP transport,
//! authentication, and request/response shape for one specific model provider.
//! The poker-side concerns — turning a [`pkdealer_agent_core::HandState`] into
//! a prompt, parsing a free-text response into a
//! [`pkdealer_agent_core::Decision`], and choosing a safe fallback on backend
//! error — live here and are reused by every backend.
//!
//! The generic [`LlmPokerAgent`] composes a [`LlmBackend`] with the shared
//! prompt-builder and response-parser to satisfy the
//! [`pkdealer_agent_core::PokerAgent`] trait.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use async_trait::async_trait;
//! use pkdealer_agent_core::{AgentConfig, run_agent};
//! use pkdealer_agent_llm::{LlmBackend, LlmError, LlmPokerAgent, LlmResponse};
//!
//! struct EchoBackend;
//!
//! #[async_trait]
//! impl LlmBackend for EchoBackend {
//!     async fn complete(&self, _prompt: &str) -> Result<LlmResponse, LlmError> {
//!         Ok(LlmResponse {
//!             text: "check".to_string(),
//!             input_tokens: 1,
//!             output_tokens: 1,
//!         })
//!     }
//! }
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let agent = LlmPokerAgent::new(EchoBackend);
//! let config = AgentConfig {
//!     endpoint: "http://127.0.0.1:50051".to_string(),
//!     name: "echo".to_string(),
//!     seat: None,
//!     chips: 10_000,
//!     client_secret: String::new(),
//! };
//! run_agent(agent, config).await?;
//! # Ok(())
//! # }
//! ```

pub mod agent;
pub mod backend;
pub mod parse;
pub mod prompt;

pub use agent::{LlmPokerAgent, fallback_decision};
pub use backend::{LlmBackend, LlmError, LlmResponse};
pub use parse::parse_action;
pub use prompt::{build_prompt, pot_odds};
