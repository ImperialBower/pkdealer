//! Game state snapshot visible to one agent at its seat.

use pkdealer_proto::dealer::{PlayerState, Street};

/// One seated player's public state within a [`HandState`].
///
/// Richer than the old `(seat, name, chips)` tuple: it also carries the
/// player's chips committed this street (`bet`) and whether they are still
/// contesting the pot (`is_active`). The active flag lets a downstream decider
/// count *live* opponents for multi-way equity instead of every occupied seat.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::SeatSnapshot;
///
/// let seat = SeatSnapshot {
///     seat: 0,
///     name: "alice".to_string(),
///     chips: 9_900,
///     bet: 100,
///     is_active: true,
/// };
/// assert!(seat.is_active);
/// assert_eq!(seat.bet, 100);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeatSnapshot {
    /// Zero-based seat number.
    pub seat: u8,
    /// Player handle / display name.
    pub name: String,
    /// Chips remaining in the player's stack (not committed this round).
    pub chips: u32,
    /// Chips wagered on the current betting round only; `0` at street start.
    pub bet: u32,
    /// `true` while the player is still contesting the pot — i.e. not folded,
    /// busted out, or merely seated waiting for the next hand. All-in players
    /// are active (they can still win the pot).
    pub is_active: bool,
}

/// Returns `true` when a [`PlayerState`] means the player is still contesting
/// the current pot.
///
/// Folded, eliminated, waiting-for-next-hand, and unspecified seats are
/// inactive; everyone with live or committed chips — including all-in — is
/// active.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::seat_state_is_active;
/// use pkdealer_proto::dealer::PlayerState;
///
/// assert!(seat_state_is_active(PlayerState::Called));
/// assert!(seat_state_is_active(PlayerState::AllIn));
/// assert!(!seat_state_is_active(PlayerState::Folded));
/// assert!(!seat_state_is_active(PlayerState::Ready));
/// ```
#[must_use]
pub fn seat_state_is_active(state: PlayerState) -> bool {
    matches!(
        state,
        PlayerState::YetToAct
            | PlayerState::Checked
            | PlayerState::Called
            | PlayerState::Bet
            | PlayerState::Raised
            | PlayerState::AllIn
            | PlayerState::Blind
    )
}

/// The portion of the poker table state visible to a single seated agent.
///
/// Constructed from the service's `GetStatus` and `GetNextToAct` responses
/// each time it is the agent's turn to act.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::{HandState, SeatSnapshot};
///
/// let state = HandState {
///     seat: 1,
///     hole_cards: "Ah Kd".to_string(),
///     board: String::new(),
///     pot: 150,
///     to_call: 50,
///     my_chips: 9_950,
///     stacks: vec![
///         SeatSnapshot { seat: 0, name: "alice".to_string(), chips: 10_000, bet: 0, is_active: true },
///         SeatSnapshot { seat: 1, name: "bob".to_string(), chips: 9_950, bet: 50, is_active: true },
///     ],
///     big_blind: 100,
///     street: "preflop".to_string(),
///     action_history: vec![],
///     button_seat: Some(0),
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
    /// All seated players and their public per-seat state.
    pub stacks: Vec<SeatSnapshot>,
    /// The table's big blind amount.
    pub big_blind: u32,
    /// Current street: `"preflop"`, `"flop"`, `"turn"`, or `"river"`.
    pub street: String,
    /// Human-readable descriptions of actions taken this street, in order.
    pub action_history: Vec<String>,
    /// Seat number of the dealer button this hand, when known. `None` when the
    /// table has not assigned a button (e.g. no hand in progress). Drives
    /// position-aware decisions.
    pub button_seat: Option<u8>,
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
                SeatSnapshot {
                    seat: 0,
                    name: "alice".to_string(),
                    chips: 10_000,
                    bet: 100,
                    is_active: true,
                },
                SeatSnapshot {
                    seat: 2,
                    name: "bob".to_string(),
                    chips: 9_700,
                    bet: 0,
                    is_active: true,
                },
            ],
            big_blind: 100,
            street: "flop".to_string(),
            action_history: vec!["alice bets 100".to_string()],
            button_seat: Some(0),
        }
    }

    #[test]
    fn hand_state_construction_happy_path() {
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
    fn hand_state_empty_board_preflop() {
        let state = HandState {
            board: String::new(),
            street: "preflop".to_string(),
            ..sample_state()
        };
        assert!(state.board.is_empty());
    }

    #[test]
    fn hand_state_clone_preserves_stacks() {
        let state = sample_state();
        let cloned = state.clone();
        assert_eq!(cloned.stacks.len(), state.stacks.len());
        assert_eq!(cloned.stacks[0].seat, 0);
        assert_eq!(cloned.button_seat, Some(0));
    }

    #[test]
    fn seat_snapshot_fields() {
        let s = SeatSnapshot {
            seat: 3,
            name: "carol".to_string(),
            chips: 5_000,
            bet: 250,
            is_active: false,
        };
        assert_eq!(s.seat, 3);
        assert_eq!(s.name, "carol");
        assert_eq!(s.chips, 5_000);
        assert_eq!(s.bet, 250);
        assert!(!s.is_active);
    }

    #[test]
    fn seat_state_is_active_contesting_states() {
        for state in [
            PlayerState::YetToAct,
            PlayerState::Checked,
            PlayerState::Called,
            PlayerState::Bet,
            PlayerState::Raised,
            PlayerState::AllIn,
            PlayerState::Blind,
        ] {
            assert!(seat_state_is_active(state), "{state:?} should be active");
        }
    }

    #[test]
    fn seat_state_is_active_inactive_states() {
        for state in [
            PlayerState::Unspecified,
            PlayerState::Ready,
            PlayerState::Folded,
            PlayerState::Out,
        ] {
            assert!(!seat_state_is_active(state), "{state:?} should be inactive");
        }
    }

    #[test]
    fn hand_state_no_call_check_scenario() {
        let state = HandState {
            to_call: 0,
            ..sample_state()
        };
        assert_eq!(state.to_call, 0);
    }

    #[test]
    fn street_name_preflop() {
        assert_eq!(street_name(Street::Preflop), "preflop");
    }

    #[test]
    fn street_name_flop() {
        assert_eq!(street_name(Street::Flop), "flop");
    }

    #[test]
    fn street_name_turn() {
        assert_eq!(street_name(Street::Turn), "turn");
    }

    #[test]
    fn street_name_river() {
        assert_eq!(street_name(Street::River), "river");
    }

    #[test]
    fn street_name_unspecified() {
        assert_eq!(street_name(Street::Unspecified), "unknown");
    }
}
