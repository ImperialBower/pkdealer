//! Prompt construction and pot-odds calculation shared by LLM backends.

use pkdealer_agent_core::HandState;

/// Formats a [`HandState`] into a natural-language prompt suitable for any
/// chat-completion LLM (Claude, Ollama-served models, …).
///
/// The prompt is deliberately single-turn and self-contained: it bakes the
/// system framing, the table snapshot, and the action grammar into one user
/// message so backends that lack a separate "system" role need no special
/// handling.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::HandState;
/// use pkdealer_agent_llm::build_prompt;
///
/// let state = HandState {
///     seat: 0,
///     hole_cards: "Ah Kd".to_string(),
///     board: String::new(),
///     pot: 200,
///     to_call: 100,
///     my_chips: 9_900,
///     stacks: vec![(0, "alice".to_string(), 9_900)],
///     big_blind: 100,
///     street: "preflop".to_string(),
///     action_history: vec![],
/// };
/// let prompt = build_prompt(&state);
/// assert!(prompt.contains("Ah Kd"));
/// assert!(prompt.contains("preflop"));
/// ```
#[must_use]
pub fn build_prompt(state: &HandState) -> String {
    let board = if state.board.is_empty() {
        "(no community cards yet)".to_string()
    } else {
        state.board.clone()
    };

    let stacks_str = state
        .stacks
        .iter()
        .map(|(seat, name, chips)| format!("seat {seat} {name}: {chips}"))
        .collect::<Vec<_>>()
        .join(", ");

    let history = if state.action_history.is_empty() {
        "(no actions yet this street)".to_string()
    } else {
        state.action_history.join("\n")
    };

    format!(
        "You are a professional poker player at a No-Limit Hold'em table.\n\n\
         Your hand: {hole_cards}\n\
         Board: {board} ({street})\n\
         Pot: {pot} chips  |  To call: {to_call} chips  |  Your stack: {my_chips} chips\n\
         Big blind: {big_blind}\n\n\
         Seat stacks: {stacks_str}\n\n\
         Action history this street:\n\
         {history}\n\n\
         Choose ONE action: fold, check, call, bet <amount>, raise <amount>\n\
         Respond with only the action, nothing else.",
        hole_cards = state.hole_cards,
        board = board,
        street = state.street,
        pot = state.pot,
        to_call = state.to_call,
        my_chips = state.my_chips,
        big_blind = state.big_blind,
        stacks_str = stacks_str,
        history = history,
    )
}

/// Computes pot odds as a fraction: `to_call / (pot + to_call)`.
///
/// Returns `0.0` when there is no bet to call. The result is included in
/// the per-decision OpenTelemetry span so downstream analysis can correlate
/// agent choices with the chip-call price they were offered.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::HandState;
/// use pkdealer_agent_llm::pot_odds;
///
/// let state = HandState {
///     seat: 0,
///     hole_cards: String::new(),
///     board: String::new(),
///     pot: 300,
///     to_call: 100,
///     my_chips: 9_900,
///     stacks: vec![],
///     big_blind: 100,
///     street: "flop".to_string(),
///     action_history: vec![],
/// };
/// let odds = pot_odds(&state);
/// assert!((odds - 0.25).abs() < 1e-9);
/// ```
#[must_use]
pub fn pot_odds(state: &HandState) -> f64 {
    if state.to_call == 0 {
        return 0.0;
    }
    let total = f64::from(state.pot) + f64::from(state.to_call);
    if total == 0.0 {
        return 0.0;
    }
    f64::from(state.to_call) / total
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn sample_state() -> HandState {
        HandState {
            seat: 1,
            hole_cards: "Ah Kd".to_string(),
            board: "Qh Js Tc".to_string(),
            pot: 300,
            to_call: 100,
            my_chips: 9_700,
            stacks: vec![
                (0, "alice".to_string(), 10_000),
                (1, "bob".to_string(), 9_700),
            ],
            big_blind: 100,
            street: "flop".to_string(),
            action_history: vec!["alice bets 100".to_string()],
        }
    }

    #[test]
    fn build_prompt_contains_hole_cards() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("Ah Kd"));
    }

    #[test]
    fn build_prompt_contains_board() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("Qh Js Tc"));
    }

    #[test]
    fn build_prompt_contains_street() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("flop"));
    }

    #[test]
    fn build_prompt_contains_pot_and_to_call() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("300"));
        assert!(prompt.contains("100"));
    }

    #[test]
    fn build_prompt_contains_stacks() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("alice"));
        assert!(prompt.contains("bob"));
    }

    #[test]
    fn build_prompt_contains_action_history() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("alice bets 100"));
    }

    #[test]
    fn build_prompt_empty_board_shows_placeholder() {
        let state = HandState {
            board: String::new(),
            street: "preflop".to_string(),
            ..sample_state()
        };
        let prompt = build_prompt(&state);
        assert!(prompt.contains("no community cards yet"));
    }

    #[test]
    fn build_prompt_empty_history_shows_placeholder() {
        let state = HandState {
            action_history: vec![],
            ..sample_state()
        };
        let prompt = build_prompt(&state);
        assert!(prompt.contains("no actions yet this street"));
    }

    #[test]
    fn build_prompt_contains_instruction() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("fold, check, call, bet"));
    }

    #[test]
    fn pot_odds_with_call() {
        let state = HandState {
            pot: 300,
            to_call: 100,
            ..sample_state()
        };
        assert!((pot_odds(&state) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn pot_odds_no_call_returns_zero() {
        let state = HandState {
            to_call: 0,
            ..sample_state()
        };
        assert_eq!(pot_odds(&state), 0.0);
    }

    #[test]
    fn pot_odds_call_equals_pot() {
        let state = HandState {
            pot: 100,
            to_call: 100,
            ..sample_state()
        };
        assert!((pot_odds(&state) - 0.5).abs() < 1e-9);
    }
}
