//! Game state snapshot visible to one agent at its seat.

use pkdealer_proto::dealer::Street;

/// The portion of the poker table state visible to a single seated agent.
///
/// Constructed from the service's `GetStatus` and `GetNextToAct` responses
/// each time it is the agent's turn to act.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::HandState;
///
/// let state = HandState {
///     seat: 1,
///     hole_cards: "Ah Kd".to_string(),
///     board: String::new(),
///     pot: 150,
///     to_call: 50,
///     my_chips: 9_950,
///     stacks: vec![(0, "alice".to_string(), 10_000), (1, "bob".to_string(), 9_950)],
///     big_blind: 100,
///     street: "preflop".to_string(),
///     action_history: vec![],
/// };
/// assert_eq!(state.seat, 1);
/// assert_eq!(state.to_call, 50);
/// assert_eq!(state.stacks.len(), 2);
/// ```
#[derive(Debug, Clone)]
pub struct HandState {
    /// This agent's seat number (0-based).
    pub seat: u8,
    /// Hole cards as a space-separated string, e.g. `"Ah Kd"`.
    pub hole_cards: String,
    /// Community cards, e.g. `"Qh Js Tc"`. Empty string before the flop.
    pub board: String,
    /// Total chips in the pot.
    pub pot: u32,
    /// Chips required to call the current bet. `0` means the agent can check.
    pub to_call: u32,
    /// This agent's current chip count.
    pub my_chips: u32,
    /// All seated players as `(seat, name, chips)` tuples.
    pub stacks: Vec<(u8, String, u32)>,
    /// The table's big blind amount.
    pub big_blind: u32,
    /// Current street: `"preflop"`, `"flop"`, `"turn"`, or `"river"`.
    pub street: String,
    /// Human-readable descriptions of actions taken this street, in order.
    pub action_history: Vec<String>,
}

/// Converts a protobuf [`Street`] variant to a lowercase street name string.
pub(crate) fn street_name(street: Street) -> &'static str {
    match street {
        Street::Preflop => "preflop",
        Street::Flop => "flop",
        Street::Turn => "turn",
        Street::River => "river",
        Street::Unspecified => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> HandState {
        HandState {
            seat: 2,
            hole_cards: "Ah Kd".to_string(),
            board: "Qh Js Tc".to_string(),
            pot: 300,
            to_call: 100,
            my_chips: 9_700,
            stacks: vec![
                (0, "alice".to_string(), 10_000),
                (2, "bob".to_string(), 9_700),
            ],
            big_blind: 100,
            street: "flop".to_string(),
            action_history: vec!["alice bets 100".to_string()],
        }
    }

    #[test]
    fn test_hand_state_construction_happy_path() {
        let state = sample_state();
        assert_eq!(state.seat, 2);
        assert_eq!(state.hole_cards, "Ah Kd");
        assert_eq!(state.board, "Qh Js Tc");
        assert_eq!(state.pot, 300);
        assert_eq!(state.to_call, 100);
        assert_eq!(state.my_chips, 9_700);
        assert_eq!(state.big_blind, 100);
        assert_eq!(state.street, "flop");
        assert_eq!(state.action_history.len(), 1);
    }

    #[test]
    fn test_hand_state_empty_board_preflop() {
        let state = HandState {
            board: String::new(),
            street: "preflop".to_string(),
            ..sample_state()
        };
        assert!(state.board.is_empty());
    }

    #[test]
    fn test_hand_state_clone_preserves_stacks() {
        let state = sample_state();
        let cloned = state.clone();
        assert_eq!(cloned.stacks.len(), state.stacks.len());
        assert_eq!(cloned.stacks[0].0, 0);
    }

    #[test]
    fn test_hand_state_no_call_check_scenario() {
        let state = HandState {
            to_call: 0,
            ..sample_state()
        };
        assert_eq!(state.to_call, 0);
    }

    #[test]
    fn test_street_name_preflop() {
        assert_eq!(street_name(Street::Preflop), "preflop");
    }

    #[test]
    fn test_street_name_flop() {
        assert_eq!(street_name(Street::Flop), "flop");
    }

    #[test]
    fn test_street_name_turn() {
        assert_eq!(street_name(Street::Turn), "turn");
    }

    #[test]
    fn test_street_name_river() {
        assert_eq!(street_name(Street::River), "river");
    }

    #[test]
    fn test_street_name_unspecified() {
        assert_eq!(street_name(Street::Unspecified), "unknown");
    }
}
