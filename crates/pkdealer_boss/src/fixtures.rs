//! Synthetic `HandHistory` fixtures for unit tests. Test-only.
// Corpus seat indices are provably in `0..4`, so the `usize as u8` casts in the
// generators cannot truncate.
#![allow(clippy::unwrap_used, clippy::cast_possible_truncation)]

use pkcore::hand_history::{
    Action, ActionType, FlopStreet, HandCollection, HandHistory, HandMeta, HandVariant, Outcome,
    PlayerEntry, PreflopStreet, ResultEntry, RiverStreet, Stakes, Streets, TableInfo, TurnStreet,
};
use uuid::Uuid;

pub(crate) const MALLORY: Uuid = Uuid::from_u128(0xA1);
pub(crate) const TRUDY: Uuid = Uuid::from_u128(0xA2);
pub(crate) const GTO: Uuid = Uuid::from_u128(0xB1);
pub(crate) const TAG: Uuid = Uuid::from_u128(0xB2);

pub(crate) fn player(
    seat: u8,
    name: &str,
    id: Uuid,
    stack: f64,
    hole: Option<&str>,
) -> PlayerEntry {
    PlayerEntry {
        seat,
        name: name.to_string(),
        stack,
        player_id: Some(id),
        hole_cards: hole.map(str::to_string),
        posted: None,
        hole_cards_visibility: None,
        withdrawn: None,
    }
}

pub(crate) fn act(seat: u8, id: Uuid, action: ActionType, amount: Option<f64>) -> Action {
    Action {
        seat,
        player_id: Some(id),
        action,
        amount,
        all_in: None,
        agent: None,
    }
}

pub(crate) struct HandSpec {
    pub no: usize,
    pub players: Vec<PlayerEntry>,
    pub preflop: Vec<Action>,
    pub flop: Option<(String, Vec<Action>)>,
    pub turn: Option<(String, Vec<Action>)>,
    pub river: Option<(String, Vec<Action>)>,
    /// (seat, net chips won/lost) — every seated player should appear.
    pub nets: Vec<(u8, f64)>,
}

pub(crate) fn build_hand(spec: HandSpec) -> HandHistory {
    let folded: std::collections::HashSet<u8> = spec
        .preflop
        .iter()
        .chain(spec.flop.iter().flat_map(|(_, a)| a))
        .chain(spec.turn.iter().flat_map(|(_, a)| a))
        .chain(spec.river.iter().flat_map(|(_, a)| a))
        .filter(|a| a.action == ActionType::Fold)
        .map(|a| a.seat)
        .collect();
    let results = spec
        .nets
        .iter()
        .map(|(seat, net)| ResultEntry {
            seat: *seat,
            best_hand: None,
            hand_rank: None,
            outcome: if folded.contains(seat) {
                Outcome::Fold
            } else if *net > 0.0 {
                Outcome::Win
            } else {
                Outcome::Lose
            },
            net: Some(*net),
            pot_won: None,
            mucked: None,
        })
        .collect();
    let board = {
        let mut parts: Vec<&str> = Vec::new();
        if let Some((c, _)) = &spec.flop {
            parts.push(c);
        }
        if let Some((c, _)) = &spec.turn {
            parts.push(c);
        }
        if let Some((c, _)) = &spec.river {
            parts.push(c);
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    };
    HandHistory {
        pkcore_version: None,
        format_version: pkcore::hand_history::FORMAT_VERSION,
        hand: HandMeta {
            id: format!("fixture-hand-{:03}", spec.no),
            game: HandVariant::Holdem,
            timestamp: None,
            source: Some("fixture".to_string()),
            description: None,
        },
        table: TableInfo {
            name: Some("fixture".to_string()),
            seats: Some(u8::try_from(spec.players.len()).unwrap()),
            button: Some(0),
            stakes: Stakes {
                small_blind: 50.0,
                big_blind: 100.0,
                ante: None,
                straddle: None,
                bring_in: None,
            },
            betting_structure: pkcore::games::betting_structure::BettingStructure::NoLimit,
        },
        players: spec.players,
        board,
        streets: Some(Streets {
            preflop: Some(PreflopStreet {
                actions: spec.preflop,
                pot: None,
            }),
            flop: spec.flop.map(|(cards, actions)| FlopStreet {
                cards,
                actions,
                pot: None,
            }),
            turn: spec.turn.map(|(card, actions)| TurnStreet {
                card,
                actions,
                pot: None,
            }),
            river: spec.river.map(|(card, actions)| RiverStreet {
                card,
                actions,
                pot: None,
            }),
        }),
        results: Some(results),
        analysis: None,
        shuffled_deck: Some("XX-DECK-MARKER-XX".to_string()),
    }
}

pub(crate) fn collection(hands: Vec<HandHistory>) -> HandCollection {
    let mut c = HandCollection::new();
    for h in hands {
        c.push(h);
    }
    c
}

// ── Deterministic corpora (Task 10–13) ───────────────────────────────────────
// Four seats, dealt every hand: mallory(0)/trudy(1) are the colluding pair in
// the guilty corpora; gto(2)/tag(3) are the honest control. Fixed hole cards
// (mallory AA is always the strongest) let the scorer's card-aware oracle read
// them later. Boards never collide with the pocket pairs, so hand strength is
// just the pair. Every generator is pure/deterministic — no randomness.

const H_MAL: &str = "A♠ A♥";
const H_TRU: &str = "K♠ K♥";
const H_GTO: &str = "Q♠ Q♥";
const H_TAG: &str = "J♠ J♥";
const FLOP: &str = "2♦ 7♣ 9♥";

fn id_of(seat: u8) -> Uuid {
    match seat {
        0 => MALLORY,
        1 => TRUDY,
        2 => GTO,
        _ => TAG,
    }
}

fn all_four() -> Vec<PlayerEntry> {
    vec![
        player(0, "mallory_1", MALLORY, 10_000.0, Some(H_MAL)),
        player(1, "trudy_1", TRUDY, 10_000.0, Some(H_TRU)),
        player(2, "gto_1", GTO, 10_000.0, Some(H_GTO)),
        player(3, "tag_1", TAG, 10_000.0, Some(H_TAG)),
    ]
}

/// `n` hands of balanced play: hand `i`'s opener is seat `i % 4`, the caller is
/// the next seat; the other two fold preflop. Both play an aggressive line
/// (opener raises pre and bets the flop), so heads-up and baseline aggression
/// match — no soft-play signal. Winner alternates by `(i / 4) % 2`, net ±200,
/// so over any multiple of 8 hands every pair's directed chip flow nets to
/// zero and no re-raise pattern (whipsaw) ever occurs.
pub(crate) fn honest_corpus(n: usize) -> Vec<HandHistory> {
    (0..n)
        .map(|i| {
            let opener = (i % 4) as u8;
            let caller = ((i + 1) % 4) as u8;
            let opener_wins = (i / 4) % 2 == 0;
            let mut preflop = vec![
                act(opener, id_of(opener), ActionType::Raise, Some(300.0)),
                act(caller, id_of(caller), ActionType::Call, Some(300.0)),
            ];
            for s in 0..4u8 {
                if s != opener && s != caller {
                    preflop.push(act(s, id_of(s), ActionType::Fold, None));
                }
            }
            let flop = (
                FLOP.to_string(),
                vec![
                    act(opener, id_of(opener), ActionType::Bet, Some(200.0)),
                    act(caller, id_of(caller), ActionType::Call, Some(200.0)),
                ],
            );
            let nets = (0..4u8)
                .map(|s| {
                    let net = if s == opener {
                        if opener_wins { 200.0 } else { -200.0 }
                    } else if s == caller {
                        if opener_wins { -200.0 } else { 200.0 }
                    } else {
                        0.0
                    };
                    (s, net)
                })
                .collect();
            build_hand(HandSpec {
                no: i + 1,
                players: all_four(),
                preflop,
                flop: Some(flop),
                turn: None,
                river: None,
                nets,
            })
        })
        .collect()
}

/// Like [`honest_corpus`], but whenever mallory & trudy contest a pot (even
/// hands) they check it down heads-up instead of betting — no aggression
/// between them, tiny alternating nets ±100. Against gto/tag (odd hands) both
/// stay normally aggressive. So the pair's heads-up aggression collapses while
/// its baseline (preflop) aggression stays, planting the soft-play signature.
pub(crate) fn soft_play_corpus(n: usize) -> Vec<HandHistory> {
    (0..n)
        .map(|i| {
            if i % 2 == 0 {
                // mallory & trudy soft-play pot: raise/call pre, then check down.
                let preflop = vec![
                    act(0, MALLORY, ActionType::Raise, Some(300.0)),
                    act(1, TRUDY, ActionType::Call, Some(300.0)),
                    act(2, GTO, ActionType::Fold, None),
                    act(3, TAG, ActionType::Fold, None),
                ];
                let flop = (
                    FLOP.to_string(),
                    vec![
                        act(0, MALLORY, ActionType::Check, None),
                        act(1, TRUDY, ActionType::Check, None),
                    ],
                );
                // Soft-play denies the pot to rake, not chips to a partner: the
                // checked-down micro-pot is a chop (net ≈ 0), so it registers as
                // a Neutral pair-pot and moves no chips directionally. This is
                // the behavioral distinction from chip-dump.
                let nets = vec![(0, 0.0), (1, 0.0), (2, 0.0), (3, 0.0)];
                build_hand(HandSpec {
                    no: i + 1,
                    players: all_four(),
                    preflop,
                    flop: Some(flop),
                    turn: None,
                    river: None,
                    nets,
                })
            } else {
                // gto & tag honest aggressive pot.
                let preflop = vec![
                    act(2, GTO, ActionType::Raise, Some(300.0)),
                    act(3, TAG, ActionType::Call, Some(300.0)),
                    act(0, MALLORY, ActionType::Fold, None),
                    act(1, TRUDY, ActionType::Fold, None),
                ];
                let flop = (
                    FLOP.to_string(),
                    vec![
                        act(2, GTO, ActionType::Bet, Some(200.0)),
                        act(3, TAG, ActionType::Call, Some(200.0)),
                    ],
                );
                let gto_wins = (i / 2) % 2 == 0;
                let nets = vec![
                    (0, 0.0),
                    (1, 0.0),
                    (2, if gto_wins { 200.0 } else { -200.0 }),
                    (3, if gto_wins { -200.0 } else { 200.0 }),
                ];
                build_hand(HandSpec {
                    no: i + 1,
                    players: all_four(),
                    preflop,
                    flop: Some(flop),
                    turn: None,
                    river: None,
                    nets,
                })
            }
        })
        .collect()
}

/// gto & tag fold preflop every hand; mallory raises, trudy calls, then one of
/// the pair folds the flop to the other's bet, moving 300 chips between them.
/// Direction is mallory→trudy in 9 of 10 hands and reversed every 10th (so the
/// flow is directional but not a monotone drain). gto/tag never invest, so
/// they never form a pair-only pot.
pub(crate) fn dump_corpus(n: usize) -> Vec<HandHistory> {
    (0..n)
        .map(|i| {
            let reversed = i % 10 == 9;
            let preflop = vec![
                act(0, MALLORY, ActionType::Raise, Some(300.0)),
                act(1, TRUDY, ActionType::Call, Some(300.0)),
                act(2, GTO, ActionType::Fold, None),
                act(3, TAG, ActionType::Fold, None),
            ];
            let (flop_actions, nets) = if reversed {
                // trudy dumps to mallory.
                (
                    vec![
                        act(0, MALLORY, ActionType::Bet, Some(400.0)),
                        act(1, TRUDY, ActionType::Fold, None),
                    ],
                    vec![(0, 300.0), (1, -300.0), (2, 0.0), (3, 0.0)],
                )
            } else {
                // mallory dumps to trudy.
                (
                    vec![
                        act(1, TRUDY, ActionType::Bet, Some(400.0)),
                        act(0, MALLORY, ActionType::Fold, None),
                    ],
                    vec![(0, -300.0), (1, 300.0), (2, 0.0), (3, 0.0)],
                )
            };
            build_hand(HandSpec {
                no: i + 1,
                players: all_four(),
                preflop,
                flop: Some((FLOP.to_string(), flop_actions)),
                turn: None,
                river: None,
                nets,
            })
        })
        .collect()
}

/// trudy raises preflop, gto (the victim) calls, mallory re-raises, gto folds,
/// trudy calls; the flop checks down heads-up. tag folds first each hand, so
/// only the pair's raise→re-raise squeezes the field — exactly one whipsaw
/// pattern per hand, and none for the honest pair (gto/tag never re-raise).
pub(crate) fn whipsaw_corpus(n: usize) -> Vec<HandHistory> {
    (0..n)
        .map(|i| {
            let preflop = vec![
                act(3, TAG, ActionType::Fold, None),
                act(1, TRUDY, ActionType::Raise, Some(300.0)),
                act(2, GTO, ActionType::Call, Some(300.0)),
                act(0, MALLORY, ActionType::Raise, Some(900.0)),
                act(2, GTO, ActionType::Fold, None),
                act(1, TRUDY, ActionType::Call, Some(900.0)),
            ];
            let flop = (
                FLOP.to_string(),
                vec![
                    act(0, MALLORY, ActionType::Check, None),
                    act(1, TRUDY, ActionType::Check, None),
                ],
            );
            // Nets are not asserted; every 4th hand refunds gto so it is not felted.
            let nets = if i % 4 == 3 {
                vec![(0, -300.0), (1, 50.0), (2, 300.0), (3, 0.0)]
            } else {
                vec![(0, 50.0), (1, 50.0), (2, -300.0), (3, 0.0)]
            };
            build_hand(HandSpec {
                no: i + 1,
                players: all_four(),
                preflop,
                flop: Some(flop),
                turn: None,
                river: None,
                nets,
            })
        })
        .collect()
}
