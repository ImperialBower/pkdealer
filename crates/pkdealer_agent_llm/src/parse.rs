//! Free-text response parsing into [`Decision`] values.

use pkdealer_agent_core::Decision;

/// Parses an LLM text response into a [`Decision`].
///
/// Recognizes `fold`, `check`, `call`, `all in`, `bet <n>`, `raise <n>`,
/// and `raise to <n>` (case-insensitive, surrounding whitespace allowed).
/// Falls back to `Check` when `to_call == 0` or `Fold` when facing a bet,
/// so the caller can treat the function as total: it always returns a
/// legal action even on garbage input.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::Decision;
/// use pkdealer_agent_llm::parse_action;
///
/// assert_eq!(parse_action("fold", 100), Decision::Fold);
/// assert_eq!(parse_action("check", 0), Decision::Check);
/// assert_eq!(parse_action("call", 50), Decision::Call);
/// assert_eq!(parse_action("bet 300", 0), Decision::Bet(300));
/// assert_eq!(parse_action("raise 600", 100), Decision::Raise(600));
/// assert_eq!(parse_action("raise to 800", 100), Decision::Raise(800));
/// // Unrecognized → safe fallback
/// assert_eq!(parse_action("???", 0), Decision::Check);
/// assert_eq!(parse_action("???", 50), Decision::Fold);
/// ```
#[must_use]
pub fn parse_action(response: &str, to_call: u32) -> Decision {
    match parse_action_opt(response) {
        Some(decision) => decision,
        None if to_call == 0 => Decision::Check,
        None => Decision::Fold,
    }
}

/// Parses an LLM text response into a [`Decision`], or `None` when the text is
/// not a recognized action.
///
/// Unlike [`parse_action`], this does **not** substitute a safe fallback — a
/// `None` return is the signal that the model produced something unparseable,
/// which the caller records as a coercion (`was_coerced`) while keeping the raw
/// text. Recognizes the same forms as [`parse_action`].
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::Decision;
/// use pkdealer_agent_llm::parse_action_opt;
///
/// assert_eq!(parse_action_opt("raise to 800"), Some(Decision::Raise(800)));
/// assert_eq!(parse_action_opt("???"), None);
/// ```
#[must_use]
pub fn parse_action_opt(response: &str) -> Option<Decision> {
    let lower = response.trim().to_lowercase();

    if lower == "fold" {
        return Some(Decision::Fold);
    }
    if lower == "check" {
        return Some(Decision::Check);
    }
    if lower == "call" {
        return Some(Decision::Call);
    }
    if matches!(lower.as_str(), "all in" | "all-in" | "allin") {
        return Some(Decision::AllIn);
    }

    if let Some(rest) = lower.strip_prefix("bet ") {
        if let Ok(n) = rest.trim().parse::<u32>() {
            return Some(Decision::Bet(n));
        }
    }

    if let Some(rest) = lower.strip_prefix("raise ") {
        let rest = rest.trim().strip_prefix("to ").unwrap_or(rest.trim());
        if let Ok(n) = rest.trim().parse::<u32>() {
            return Some(Decision::Raise(n));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_variants() {
        assert_eq!(parse_action("fold", 100), Decision::Fold);
        assert_eq!(parse_action("FOLD", 100), Decision::Fold);
        assert_eq!(parse_action("  fold  ", 100), Decision::Fold);
    }

    #[test]
    fn check_variants() {
        assert_eq!(parse_action("check", 0), Decision::Check);
        assert_eq!(parse_action("CHECK", 0), Decision::Check);
    }

    #[test]
    fn call_variants() {
        assert_eq!(parse_action("call", 50), Decision::Call);
        assert_eq!(parse_action("Call", 50), Decision::Call);
    }

    #[test]
    fn all_in_variants() {
        assert_eq!(parse_action("all in", 100), Decision::AllIn);
        assert_eq!(parse_action("all-in", 100), Decision::AllIn);
        assert_eq!(parse_action("allin", 100), Decision::AllIn);
        assert_eq!(parse_action("All In", 100), Decision::AllIn);
    }

    #[test]
    fn bet_with_amount() {
        assert_eq!(parse_action("bet 300", 0), Decision::Bet(300));
        assert_eq!(parse_action("BET 500", 0), Decision::Bet(500));
    }

    #[test]
    fn raise_with_amount() {
        assert_eq!(parse_action("raise 600", 100), Decision::Raise(600));
        assert_eq!(parse_action("RAISE 1000", 100), Decision::Raise(1_000));
    }

    #[test]
    fn raise_to_with_amount() {
        assert_eq!(parse_action("raise to 800", 100), Decision::Raise(800));
        assert_eq!(parse_action("Raise To 1200", 100), Decision::Raise(1_200));
    }

    #[test]
    fn fallback_no_call() {
        assert_eq!(parse_action("???", 0), Decision::Check);
        assert_eq!(parse_action("", 0), Decision::Check);
    }

    #[test]
    fn fallback_with_call() {
        assert_eq!(parse_action("???", 50), Decision::Fold);
        assert_eq!(parse_action("invalid response", 100), Decision::Fold);
    }

    #[test]
    fn bet_with_non_numeric_falls_back() {
        assert_eq!(parse_action("bet many", 0), Decision::Check);
        assert_eq!(parse_action("bet many", 50), Decision::Fold);
    }

    #[test]
    fn parse_action_opt_recognizes_actions() {
        assert_eq!(parse_action_opt("call"), Some(Decision::Call));
        assert_eq!(parse_action_opt("raise to 800"), Some(Decision::Raise(800)));
        assert_eq!(parse_action_opt("bet 300"), Some(Decision::Bet(300)));
        assert_eq!(parse_action_opt("all-in"), Some(Decision::AllIn));
    }

    #[test]
    fn parse_action_opt_returns_none_for_unrecognized() {
        assert_eq!(parse_action_opt("???"), None);
        assert_eq!(parse_action_opt("bet many"), None);
        assert_eq!(parse_action_opt(""), None);
    }
}
