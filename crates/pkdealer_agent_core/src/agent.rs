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
        }
    }

    #[test]
    fn test_decision_debug_fold() {
        assert_eq!(format!("{:?}", Decision::Fold), "Fold");
    }

    #[test]
    fn test_decision_debug_bet() {
        assert_eq!(format!("{:?}", Decision::Bet(200)), "Bet(200)");
    }

    #[test]
    fn test_decision_debug_raise() {
        assert_eq!(format!("{:?}", Decision::Raise(500)), "Raise(500)");
    }

    #[test]
    fn test_decision_clone_preserves_amount() {
        let d = Decision::Raise(500);
        assert_eq!(d.clone(), Decision::Raise(500));
    }

    #[test]
    fn test_decision_equality_same_variant() {
        assert_eq!(Decision::Check, Decision::Check);
        assert_eq!(Decision::Bet(100), Decision::Bet(100));
    }

    #[test]
    fn test_decision_inequality_different_variant() {
        assert_ne!(Decision::Fold, Decision::Call);
    }

    #[test]
    fn test_decision_inequality_different_amount() {
        assert_ne!(Decision::Bet(100), Decision::Bet(200));
    }

    #[tokio::test]
    async fn test_poker_agent_stub_always_fold() {
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
    async fn test_poker_agent_stub_bet_pot() {
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
