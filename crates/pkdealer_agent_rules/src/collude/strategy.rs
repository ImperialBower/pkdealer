//! Collusion styles and the pure decision adjustments that realize them.
//!
//! Each style is a pure function of `(base_decision, snapshot, partner_seat,
//! partner_hole)`: given what the honest decider chose and the partner's
//! (leaked) hole cards, it returns the action a colluder plays instead. The
//! channel that delivered `partner_hole` is irrelevant here — Vectors A and B
//! feed identical inputs, which is exactly why the Boss catches the behavior,
//! not the channel.

use pkcore::arrays::HandRanker;
use pkcore::arrays::five::Five;
use pkcore::arrays::seven::Seven;
use pkcore::arrays::six::Six;
use pkcore::arrays::two::Two;
use pkcore::bot::player_action::PlayerAction;
use pkcore::bot::table_snapshot::{SeatInfo, TableSnapshot};
use pkcore::cards::Cards;
use std::str::FromStr;

/// Ways a colluding pair exploits shared hole cards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollusionStyle {
    /// Never bet/raise into the partner heads-up; check/call down.
    Soft,
    /// Re-raise behind the partner's raise to squeeze third parties out.
    Whipsaw,
    /// Fold the weaker team hand to concentrate chips on the partner.
    Dump,
}

/// Applies a collusion style on top of the honest decider's `base` action.
///
/// Pure function of `(style, base, snapshot, partner_seat, partner_hole)` —
/// the channel that delivered `partner_hole` is irrelevant (A/B equivalence).
/// When the collusion condition does not hold (e.g. the partner is not the
/// lone opponent for soft-play), the honest `base` action is returned
/// unchanged.
///
/// # Examples
///
/// ```text
/// // Soft-play: heads-up with the partner, an aggressive base action
/// // degrades to a check/call. (Requires --features collusion; this crate is
/// // a binary, so the example is illustrative rather than a run doc test.)
/// let adjusted = apply_style(CollusionStyle::Soft, PlayerAction::Bet(400), &snap, 1, &partner_hole);
/// assert_eq!(adjusted, PlayerAction::Check);
/// ```
#[must_use]
pub fn apply_style(
    style: CollusionStyle,
    base: PlayerAction,
    snap: &TableSnapshot<'_>,
    partner_seat: u8,
    partner_hole: &Cards,
) -> PlayerAction {
    match style {
        CollusionStyle::Soft => soft_play(base, snap, partner_seat),
        CollusionStyle::Whipsaw => whipsaw(base, snap, partner_seat),
        CollusionStyle::Dump => chip_dump(base, snap, partner_seat, partner_hole),
    }
}

/// Live opponents (contesting the pot, not the hero).
fn live_opponents<'a>(snap: &'a TableSnapshot<'_>) -> impl Iterator<Item = &'a SeatInfo> {
    snap.stacks
        .iter()
        .filter(|s| s.is_active && s.seat != snap.seat)
}

/// `SoftPlay`: never bet or raise into a pot where the partner is the only
/// other live player — check/call down instead.
fn soft_play(base: PlayerAction, snap: &TableSnapshot<'_>, partner_seat: u8) -> PlayerAction {
    let mut live = live_opponents(snap);
    let heads_up_with_partner =
        matches!((live.next(), live.next()), (Some(s), None) if s.seat == partner_seat);
    if !heads_up_with_partner {
        return base;
    }
    match base {
        PlayerAction::Bet(_) | PlayerAction::Raise(_) | PlayerAction::AllIn => {
            if snap.to_call > 0 {
                PlayerAction::Call
            } else {
                PlayerAction::Check
            }
        }
        other => other,
    }
}

/// Whipsaw: when the partner leads the betting this street and a third
/// party is still live, escalate a passive base action into a re-raise to
/// squeeze the victim out.
fn whipsaw(base: PlayerAction, snap: &TableSnapshot<'_>, partner_seat: u8) -> PlayerAction {
    let Some(partner) = snap
        .stacks
        .iter()
        .find(|s| s.seat == partner_seat && s.is_active)
    else {
        return base;
    };
    let top_bet = snap.stacks.iter().map(|s| s.bet).max().unwrap_or(0);
    let partner_leads = partner.bet > 0 && partner.bet == top_bet && partner.bet > snap.big_blind;
    let third_party_live = live_opponents(snap).any(|s| s.seat != partner_seat);
    if partner_leads && third_party_live && matches!(base, PlayerAction::Check | PlayerAction::Call)
    {
        return PlayerAction::Raise(snap.current_bet + snap.min_raise);
    }
    base
}

/// `ChipDump`: when the partner is committed with the stronger team hand,
/// fold (or check when folding is free money) to concentrate the team's
/// equity on the partner.
fn chip_dump(
    base: PlayerAction,
    snap: &TableSnapshot<'_>,
    partner_seat: u8,
    partner_hole: &Cards,
) -> PlayerAction {
    let Some(partner) = snap
        .stacks
        .iter()
        .find(|s| s.seat == partner_seat && s.is_active)
    else {
        return base;
    };
    let committed = partner.bet > 0 || (partner.chips == 0 && partner.is_active);
    if !committed {
        return base;
    }
    let (Some(hero), Some(villain)) = (
        strength(&snap.hole_cards, &snap.board),
        strength(partner_hole, &snap.board),
    ) else {
        return base;
    };
    if villain.beats(&hero) {
        if snap.to_call > 0 {
            PlayerAction::Fold
        } else {
            PlayerAction::Check
        }
    } else {
        base
    }
}

/// Comparable hand strength. Postflop uses pkcore's Cactus-Kev rank value
/// (LOWER wins); preflop uses a crude deterministic proxy (HIGHER wins,
/// pairs above unpaired hands). The two forms never compare across streets —
/// both team hands always share the same board.
enum Strength {
    /// Cactus-Kev `HandRankValue` — lower is stronger.
    Postflop(u16),
    /// Crude preflop proxy — higher is stronger.
    Preflop(u32),
}

impl Strength {
    fn beats(&self, other: &Strength) -> bool {
        match (self, other) {
            (Strength::Postflop(a), Strength::Postflop(b)) => a < b,
            (Strength::Preflop(a), Strength::Preflop(b)) => a > b,
            _ => false,
        }
    }
}

/// Evaluates a hand's [`Strength`] for the current street. Returns `None`
/// when the hole cards are missing or the board is an unsupported length.
fn strength(hole: &Cards, board: &Cards) -> Option<Strength> {
    if hole.len() < 2 {
        return None;
    }
    let joined = format!("{hole} {board}");
    match board.len() {
        0 => preflop_score(hole).map(Strength::Preflop),
        3 => Five::from_str(&joined)
            .ok()
            .map(|h| Strength::Postflop(h.hand_rank_value())),
        4 => Six::from_str(&joined)
            .ok()
            .map(|h| Strength::Postflop(h.hand_rank_value())),
        5 => Seven::from_str(&joined)
            .ok()
            .map(|h| Strength::Postflop(h.hand_rank_value())),
        _ => None,
    }
}

/// Crude preflop ordering: any pair beats any unpaired hand; within each
/// class, higher rank bits win; suited breaks ties. NOT equity-accurate
/// (22 outranks AKs here) — sufficient for deterministic dump decisions,
/// documented as a simulation constraint.
fn preflop_score(hole: &Cards) -> Option<u32> {
    let two = Two::from_str(&hole.to_string()).ok()?;
    let base = two.rank_binary();
    Some(if two.is_pair() {
        1_000_000 + base
    } else {
        base * 2 + u32::from(two.is_suited())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hand_state_to_snapshot;
    use pkcore::Forgiving;
    use pkcore::bot::player_action::PlayerAction;
    use pkdealer_agent_core::{HandState, SeatSnapshot};

    fn seat(seat: u8, name: &str, chips: u32, bet: u32, active: bool) -> SeatSnapshot {
        SeatSnapshot {
            seat,
            name: name.to_string(),
            chips,
            bet,
            is_active: active,
            player_id: None,
        }
    }

    /// Hero at seat 0 with `hole`; opponents as given.
    fn state(hole: &str, board: &str, to_call: u32, others: Vec<SeatSnapshot>) -> HandState {
        let mut stacks = vec![seat(0, "mallory_1", 10_000, 0, true)];
        stacks.extend(others);
        HandState {
            seat: 0,
            hole_cards: hole.to_string(),
            board: board.to_string(),
            pot: 600,
            to_call,
            my_chips: 10_000,
            stacks,
            big_blind: 100,
            street: if board.is_empty() {
                "preflop".into()
            } else {
                "flop".into()
            },
            action_history: vec![],
            button_seat: Some(0),
            hand_no: 7,
        }
    }

    fn cards(s: &str) -> Cards {
        Cards::forgiving_from_str(s)
    }

    #[test]
    fn soft_play_never_raises_partner_heads_up() {
        // Partner (seat 1) is the only live opponent; a raising hand checks back.
        let s = state(
            "Ah Kd",
            "Ac Kc 2d",
            0,
            vec![seat(1, "trudy_1", 9_000, 0, true)],
        );
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(
            CollusionStyle::Soft,
            PlayerAction::Bet(400),
            &snap,
            1,
            &cards("Qs Qc"),
        );
        assert_eq!(adjusted, PlayerAction::Check);
    }

    #[test]
    fn soft_play_calls_when_facing_partner_bet() {
        let s = state(
            "Ah Kd",
            "Ac Kc 2d",
            300,
            vec![seat(1, "trudy_1", 9_000, 300, true)],
        );
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(
            CollusionStyle::Soft,
            PlayerAction::Raise(900),
            &snap,
            1,
            &cards("Qs Qc"),
        );
        assert_eq!(adjusted, PlayerAction::Call);
    }

    #[test]
    fn colluder_softplays_partner_only() {
        // Same made hand, but the live opponent is NOT the partner → base stands.
        let s = state(
            "Ah Kd",
            "Ac Kc 2d",
            0,
            vec![
                seat(1, "trudy_1", 9_000, 0, false), // partner folded
                seat(2, "gto_1", 9_000, 0, true),
            ],
        );
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(
            CollusionStyle::Soft,
            PlayerAction::Bet(400),
            &snap,
            1,
            &cards("Qs Qc"),
        );
        assert_eq!(adjusted, PlayerAction::Bet(400));
    }

    #[test]
    fn whipsaw_squeezes_third_party() {
        // Partner leads the street, a victim is still live, base would call →
        // re-raise to squeeze.
        let s = state(
            "9h 8h",
            "",
            300,
            vec![
                seat(1, "trudy_1", 9_700, 300, true), // partner raised to 300
                seat(2, "gto_1", 9_900, 100, true),   // victim in the middle
            ],
        );
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(
            CollusionStyle::Whipsaw,
            PlayerAction::Call,
            &snap,
            1,
            &cards("As Ac"),
        );
        assert!(
            matches!(adjusted, PlayerAction::Raise(_)),
            "got {adjusted:?}"
        );
    }

    #[test]
    fn whipsaw_without_third_party_leaves_base() {
        let s = state("9h 8h", "", 300, vec![seat(1, "trudy_1", 9_700, 300, true)]);
        let snap = hand_state_to_snapshot(&s);
        assert_eq!(
            apply_style(
                CollusionStyle::Whipsaw,
                PlayerAction::Call,
                &snap,
                1,
                &cards("As Ac")
            ),
            PlayerAction::Call
        );
    }

    #[test]
    fn chip_dump_folds_strong_to_partner() {
        // Hero holds KK (strong) but partner's committed AA is stronger on a
        // full board → fold rather than win chips off the partner.
        let s = state(
            "Kh Kd",
            "2d 7c 9s Jd 3h",
            400,
            vec![seat(1, "trudy_1", 9_000, 400, true)],
        );
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(
            CollusionStyle::Dump,
            PlayerAction::Call,
            &snap,
            1,
            &cards("As Ah"),
        );
        assert_eq!(adjusted, PlayerAction::Fold);
    }

    #[test]
    fn colluder_folds_worse_team_hand() {
        // Preflop: hero 72o vs partner's committed AA → weaker team hand folds.
        let s = state("7d 2c", "", 300, vec![seat(1, "trudy_1", 9_700, 300, true)]);
        let snap = hand_state_to_snapshot(&s);
        let adjusted = apply_style(
            CollusionStyle::Dump,
            PlayerAction::Call,
            &snap,
            1,
            &cards("As Ah"),
        );
        assert_eq!(adjusted, PlayerAction::Fold);
    }

    #[test]
    fn chip_dump_keeps_base_when_hero_is_stronger() {
        let s = state("As Ah", "", 300, vec![seat(1, "trudy_1", 9_700, 300, true)]);
        let snap = hand_state_to_snapshot(&s);
        assert_eq!(
            apply_style(
                CollusionStyle::Dump,
                PlayerAction::Call,
                &snap,
                1,
                &cards("Kh Kd")
            ),
            PlayerAction::Call
        );
    }
}
