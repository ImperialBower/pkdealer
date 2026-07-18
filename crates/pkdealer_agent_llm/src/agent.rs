//! The generic [`LlmPokerAgent`] wrapper.
//!
//! [`LlmPokerAgent<B>`] composes any [`LlmBackend`] with the shared
//! [`build_prompt`] / [`parse_action_opt`]
//! pipeline to produce a value that satisfies
//! [`pkdealer_agent_core::PokerAgent`]. Backend authors only need to wire up
//! HTTP — the poker-side concerns are handled here.

use async_trait::async_trait;
use pkdealer_agent_core::{AgentFidelity, Decision, HandState, PokerAgent};
use tracing::Instrument as _;

use crate::backend::LlmBackend;
use crate::parse::parse_action_opt;
use crate::prompt::{build_prompt, pot_odds};

/// Generic poker agent driven by any [`LlmBackend`].
///
/// On each turn, the agent:
///
/// 1. Builds a prompt from the [`HandState`] via [`build_prompt`].
/// 2. Calls `backend.complete(&prompt)` inside an `llm.decision` span carrying
///    `gen_ai.*` OpenTelemetry semantic-convention attributes.
/// 3. Parses the response with [`parse_action_opt`] and records the chosen
///    action on the span.
/// 4. On backend error, returns [`fallback_decision`] (`Check` when there is
///    nothing to call, `Fold` otherwise) so the agent never stalls a hand.
///
/// # Examples
///
/// ```rust
/// use async_trait::async_trait;
/// use pkdealer_agent_core::{Decision, HandState, PokerAgent};
/// use pkdealer_agent_llm::{LlmBackend, LlmError, LlmPokerAgent, LlmResponse};
///
/// struct AlwaysFolds;
///
/// #[async_trait]
/// impl LlmBackend for AlwaysFolds {
///     async fn complete(&self, _prompt: &str) -> Result<LlmResponse, LlmError> {
///         Ok(LlmResponse { text: "fold".into(), input_tokens: 0, output_tokens: 0 })
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() {
/// let agent = LlmPokerAgent::new(AlwaysFolds);
/// let state = HandState {
///     seat: 0, hole_cards: "2c 7d".into(), board: String::new(),
///     pot: 100, to_call: 50, my_chips: 9_950, stacks: vec![],
///     big_blind: 100, street: "preflop".into(), action_history: vec![],
///     button_seat: None,
/// };
/// assert_eq!(agent.decide(&state).await, Decision::Fold);
/// # }
/// ```
pub struct LlmPokerAgent<B> {
    /// The backend that handles transport and provider-specific concerns.
    pub backend: B,
    /// Identifier reported as `gen_ai.system` on each decision span.
    pub system: &'static str,
    /// Model identifier reported as `gen_ai.request.model` on each span.
    pub model: String,
}

impl<B> LlmPokerAgent<B> {
    /// Construct an agent without provider/model identifiers. The span
    /// attributes `gen_ai.system` and `gen_ai.request.model` will be empty.
    /// Prefer [`LlmPokerAgent::with_model`] when those identifiers are known.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            system: "",
            model: String::new(),
        }
    }

    /// Construct an agent that tags every decision span with the given
    /// provider and model identifiers.
    pub fn with_model(backend: B, system: &'static str, model: impl Into<String>) -> Self {
        Self {
            backend,
            system,
            model: model.into(),
        }
    }
}

#[async_trait]
impl<B: LlmBackend> PokerAgent for LlmPokerAgent<B> {
    async fn decide(&self, state: &HandState) -> Decision {
        self.decide_with_fidelity(state).await.0
    }

    async fn decide_with_fidelity(&self, state: &HandState) -> (Decision, AgentFidelity) {
        let prompt = build_prompt(state);
        let pot_odds_val = pot_odds(state);
        let to_call = state.to_call;
        let street = state.street.clone();
        let model = (!self.model.is_empty()).then(|| self.model.clone());

        let span = tracing::info_span!(
            "llm.decision",
            gen_ai.system = self.system,
            gen_ai.request.model = self.model.as_str(),
            poker.street = street.as_str(),
            poker.pot = state.pot,
            poker.pot_odds = pot_odds_val,
            gen_ai.usage.input_tokens = tracing::field::Empty,
            gen_ai.usage.output_tokens = tracing::field::Empty,
            poker.action_chosen = tracing::field::Empty,
        );

        let result = self
            .backend
            .complete(&prompt)
            .instrument(span.clone())
            .await;

        match result {
            Ok(response) => {
                span.record("gen_ai.usage.input_tokens", response.input_tokens);
                span.record("gen_ai.usage.output_tokens", response.output_tokens);
                // `None` ⇒ the model produced something unparseable; we apply a
                // safe fallback and flag the coercion while keeping the raw text.
                let parsed = parse_action_opt(&response.text);
                let decision = parsed.clone().unwrap_or_else(|| fallback_decision(to_call));
                span.record("poker.action_chosen", tracing::field::debug(&decision));
                eprintln!(
                    "[{system}] {street} → {text:?} → {decision:?}  (in={in_tok} out={out_tok})",
                    system = if self.system.is_empty() {
                        "llm"
                    } else {
                        self.system
                    },
                    text = response.text,
                    in_tok = response.input_tokens,
                    out_tok = response.output_tokens,
                );
                let fidelity = AgentFidelity {
                    raw_response: Some(response.text),
                    was_coerced: Some(parsed.is_none()),
                    // Intended-vs-applied is left to the runner's legality clamp;
                    // a clean parse already equals the applied action here.
                    intended_action: None,
                    input_tokens: Some(response.input_tokens),
                    output_tokens: Some(response.output_tokens),
                    model,
                    // EPIC-44 Phase 3: capture the exact prompt sent to the model so
                    // offline analysis can re-tokenize it against a target tokenizer.
                    prompt: Some(prompt),
                };
                (decision, fidelity)
            }
            Err(e) => {
                eprintln!(
                    "[{system}] backend error ({street}): {e}",
                    system = if self.system.is_empty() {
                        "llm"
                    } else {
                        self.system
                    }
                );
                let fidelity = AgentFidelity {
                    was_coerced: Some(true),
                    model,
                    ..Default::default()
                };
                (fallback_decision(to_call), fidelity)
            }
        }
    }
}

/// The safe action chosen when a backend errors or returns garbage.
///
/// `Check` when nothing is owed, `Fold` otherwise — the cheapest legal move
/// in both cases.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::Decision;
/// use pkdealer_agent_llm::fallback_decision;
///
/// assert_eq!(fallback_decision(0), Decision::Check);
/// assert_eq!(fallback_decision(50), Decision::Fold);
/// ```
#[must_use]
pub fn fallback_decision(to_call: u32) -> Decision {
    if to_call == 0 {
        Decision::Check
    } else {
        Decision::Fold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{LlmError, LlmResponse};

    struct FixedBackend(&'static str);

    #[async_trait]
    impl LlmBackend for FixedBackend {
        async fn complete(&self, _prompt: &str) -> Result<LlmResponse, LlmError> {
            Ok(LlmResponse {
                text: self.0.to_string(),
                input_tokens: 10,
                output_tokens: 2,
            })
        }
    }

    struct FailingBackend;

    #[async_trait]
    impl LlmBackend for FailingBackend {
        async fn complete(&self, _prompt: &str) -> Result<LlmResponse, LlmError> {
            Err(LlmError::new("simulated outage"))
        }
    }

    fn sample_state(to_call: u32) -> HandState {
        HandState {
            seat: 0,
            hole_cards: "2c 7d".to_string(),
            board: String::new(),
            pot: 150,
            to_call,
            my_chips: 9_950,
            stacks: vec![],
            big_blind: 100,
            street: "preflop".to_string(),
            action_history: vec![],
            button_seat: None,
        }
    }

    #[tokio::test]
    async fn decide_returns_parsed_action_on_success() {
        let agent = LlmPokerAgent::with_model(FixedBackend("call"), "test", "fake-model");
        assert_eq!(agent.decide(&sample_state(50)).await, Decision::Call);
    }

    #[tokio::test]
    async fn decide_with_fidelity_clean_parse_surfaces_provenance() {
        let agent = LlmPokerAgent::with_model(FixedBackend("call"), "test", "fake-model");
        let (decision, f) = agent.decide_with_fidelity(&sample_state(50)).await;
        assert_eq!(decision, Decision::Call);
        assert_eq!(f.raw_response.as_deref(), Some("call"));
        assert_eq!(f.was_coerced, Some(false));
        assert_eq!(f.intended_action, None);
        assert_eq!(f.input_tokens, Some(10));
        assert_eq!(f.output_tokens, Some(2));
        assert_eq!(f.model.as_deref(), Some("fake-model"));
    }

    #[tokio::test]
    async fn decide_with_fidelity_unparseable_marks_coerced_keeps_text() {
        let agent = LlmPokerAgent::with_model(FixedBackend("hmm maybe"), "test", "m");
        let (decision, f) = agent.decide_with_fidelity(&sample_state(50)).await;
        assert_eq!(decision, Decision::Fold); // safe fallback facing a bet
        assert_eq!(f.was_coerced, Some(true));
        assert_eq!(f.raw_response.as_deref(), Some("hmm maybe"));
        assert_eq!(f.intended_action, None);
    }

    #[tokio::test]
    async fn decide_with_fidelity_backend_error_has_no_text() {
        let agent = LlmPokerAgent::with_model(FailingBackend, "test", "m");
        let (decision, f) = agent.decide_with_fidelity(&sample_state(0)).await;
        assert_eq!(decision, Decision::Check);
        assert_eq!(f.was_coerced, Some(true));
        assert_eq!(f.raw_response, None);
        assert_eq!(f.input_tokens, None);
        assert_eq!(f.model.as_deref(), Some("m"));
    }

    #[tokio::test]
    async fn decide_with_fidelity_empty_model_is_none() {
        let agent = LlmPokerAgent::new(FixedBackend("fold"));
        let (_decision, f) = agent.decide_with_fidelity(&sample_state(50)).await;
        assert_eq!(f.model, None);
    }

    #[tokio::test]
    async fn decide_falls_back_to_check_when_backend_fails_and_nothing_to_call() {
        let agent = LlmPokerAgent::with_model(FailingBackend, "test", "fake");
        assert_eq!(agent.decide(&sample_state(0)).await, Decision::Check);
    }

    #[tokio::test]
    async fn decide_falls_back_to_fold_when_backend_fails_facing_bet() {
        let agent = LlmPokerAgent::with_model(FailingBackend, "test", "fake");
        assert_eq!(agent.decide(&sample_state(100)).await, Decision::Fold);
    }

    #[test]
    fn fallback_decision_no_bet() {
        assert_eq!(fallback_decision(0), Decision::Check);
    }

    #[test]
    fn fallback_decision_facing_bet() {
        assert_eq!(fallback_decision(50), Decision::Fold);
    }

    #[test]
    fn new_constructor_leaves_identifiers_empty() {
        let agent = LlmPokerAgent::new(FixedBackend("fold"));
        assert_eq!(agent.system, "");
        assert!(agent.model.is_empty());
    }
}
