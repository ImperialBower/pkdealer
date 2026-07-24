//! Agent decision interface shared by all poker bot implementations.

use crate::hand_state::HandState;

/// The action an agent chooses to take on its turn.
///
/// Mirrors the proto `ActionType` enum but is defined independently so
/// callers do not need to import protobuf types directly.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::Decision;
///
/// let fold = Decision::Fold;
/// let bet = Decision::Bet(200);
/// let raise = Decision::Raise(500);
///
/// assert_eq!(fold, Decision::Fold);
/// assert_eq!(bet, Decision::Bet(200));
/// assert!(matches!(raise, Decision::Raise(_)));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Discard the hand and forfeit any chips already invested.
    Fold,
    /// Pass the action without putting chips in (only legal when `to_call == 0`).
    Check,
    /// Match the current bet to stay in the hand.
    Call,
    /// Open a new bet for the specified chip amount.
    Bet(u32),
    /// Increase a prior bet to the specified total amount.
    Raise(u32),
    /// Commit all remaining chips.
    AllIn,
}

/// Per-decision provenance: what an agent *produced* versus what the table will
/// *apply* (EPIC-25 Phase 4).
///
/// Surfaced alongside a [`Decision`] by [`PokerAgent::decide_with_fidelity`] and
/// mapped onto the `PlayerAction.agent` proto field by the runner. Every field
/// is optional; an empty value (all `None`) means "no provenance", which the
/// recorder treats as an un-annotated action.
///
/// The applied action lives in the surrounding [`Decision`]; this struct records
/// the agent-side story — raw model text, token usage, model id, whether the
/// action was coerced (parse fallback, legality clamp, or rejection retry), and
/// the originally [`intended_action`](Self::intended_action) when it differs.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::{AgentFidelity, Decision};
///
/// let fidelity = AgentFidelity {
///     raw_response: Some("raise to 250".to_string()),
///     was_coerced: Some(true),
///     intended_action: Some(Decision::Raise(250)),
///     model: Some("claude-sonnet".to_string()),
///     ..Default::default()
/// };
/// assert_eq!(fidelity.intended_action, Some(Decision::Raise(250)));
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentFidelity {
    /// Raw, unparsed model/agent response text (LLM agents). `None` for agents
    /// that produce a structured decision directly (rules/random).
    pub raw_response: Option<String>,
    /// True when the applied action differs from what the agent intended — an
    /// unparseable response, a bet/raise clamped to a legal size, or a
    /// server-rejected action replaced by a safe fallback.
    pub was_coerced: Option<bool>,
    /// The action the agent originally intended, when it differs from the
    /// applied [`Decision`].
    pub intended_action: Option<Decision>,
    /// Prompt/input tokens reported by the backend (LLM agents).
    pub input_tokens: Option<u32>,
    /// Completion/output tokens reported by the backend (LLM agents).
    pub output_tokens: Option<u32>,
    /// Model / agent identifier (e.g. `"claude-..."`, `"rules-v1"`).
    pub model: Option<String>,
    /// The prompt text sent to the model (LLM agents), captured at decision time
    /// so offline cost analysis can re-tokenize it against a target model's
    /// tokenizer (EPIC-44 Phase 3). `None` for structured agents.
    pub prompt: Option<String>,
}

/// Decision-making interface implemented by every agent type.
///
/// The `decide` method is called each time it is the agent's turn to act.
/// Implementors receive a [`HandState`] snapshot and return a [`Decision`].
/// The method is `async` to support I/O-bound agents such as LLM clients.
#[async_trait::async_trait]
pub trait PokerAgent: Send + Sync {
    /// Choose an action given the current hand state.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use pkdealer_agent_core::{Decision, HandState, PokerAgent};
    ///
    /// struct AlwaysFold;
    ///
    /// #[async_trait::async_trait]
    /// impl PokerAgent for AlwaysFold {
    ///     async fn decide(&self, _state: &HandState) -> Decision {
    ///         Decision::Fold
    ///     }
    /// }
    /// ```
    async fn decide(&self, state: &HandState) -> Decision;

    /// Choose an action *and* surface its [`AgentFidelity`] provenance.
    ///
    /// The default implementation returns the bare [`decide`](Self::decide)
    /// result with empty fidelity, so structured agents (rules/random) need no
    /// changes. LLM agents override this to surface raw response text, token
    /// usage, the model id, and parse-level coercions. The runner finalizes
    /// `was_coerced` / `intended_action` around its own legality clamp and
    /// rejection retries.
    async fn decide_with_fidelity(&self, state: &HandState) -> (Decision, AgentFidelity) {
        (self.decide(state).await, AgentFidelity::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> HandState {
        HandState {
            seat: 0,
            hole_cards: "2c 7d".to_string(),
            board: String::new(),
            pot: 150,
            to_call: 50,
            my_chips: 9_950,
            stacks: vec![],
            big_blind: 100,
            street: "preflop".to_string(),
            action_history: vec![],
            button_seat: None,
            hand_no: 0,
        }
    }

    #[test]
    fn decision_debug_fold() {
        assert_eq!(format!("{:?}", Decision::Fold), "Fold");
    }

    #[test]
    fn decision_debug_bet() {
        assert_eq!(format!("{:?}", Decision::Bet(200)), "Bet(200)");
    }

    #[test]
    fn decision_debug_raise() {
        assert_eq!(format!("{:?}", Decision::Raise(500)), "Raise(500)");
    }

    #[test]
    fn decision_clone_preserves_amount() {
        let d = Decision::Raise(500);
        assert_eq!(d.clone(), Decision::Raise(500));
    }

    #[test]
    fn decision_equality_same_variant() {
        assert_eq!(Decision::Check, Decision::Check);
        assert_eq!(Decision::Bet(100), Decision::Bet(100));
    }

    #[test]
    fn decision_inequality_different_variant() {
        assert_ne!(Decision::Fold, Decision::Call);
    }

    #[test]
    fn decision_inequality_different_amount() {
        assert_ne!(Decision::Bet(100), Decision::Bet(200));
    }

    #[tokio::test]
    async fn poker_agent_stub_always_fold() {
        struct AlwaysFold;

        #[async_trait::async_trait]
        impl PokerAgent for AlwaysFold {
            async fn decide(&self, _state: &HandState) -> Decision {
                Decision::Fold
            }
        }

        let agent = AlwaysFold;
        assert_eq!(agent.decide(&sample_state()).await, Decision::Fold);
    }

    #[tokio::test]
    async fn poker_agent_stub_bet_pot() {
        struct BetPot;

        #[async_trait::async_trait]
        impl PokerAgent for BetPot {
            async fn decide(&self, state: &HandState) -> Decision {
                Decision::Bet(state.pot)
            }
        }

        let agent = BetPot;
        let state = sample_state();
        assert_eq!(agent.decide(&state).await, Decision::Bet(150));
    }
}
