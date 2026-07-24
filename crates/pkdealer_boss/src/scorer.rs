//! Ground-truth grading — **the only detection-adjacent code allowed to read
//! hole cards.**
//!
//! The scorer reads the full, un-redacted [`HandCollection`] plus the
//! [`GroundTruthLabels`] sidecar to grade a Boss run: per labeled colluding
//! pair it reports the hands-to-detection (straight from the SPRT verdict) and
//! runs the card-aware **EV-sacrifice oracle** — a perfect-information upper
//! bound on what an omniscient auditor could catch, bounding what the blind
//! Boss can be asked to achieve. Its output **grades** the Boss and never
//! feeds detection inputs.

use std::collections::HashSet;
use std::str::FromStr;

use pkcore::arrays::HandRanker;
use pkcore::arrays::seven::Seven;
use pkcore::hand_history::{Action, ActionType, HandCollection, HandHistory, PlayerEntry};
use uuid::Uuid;

use crate::detector::Verdict;
use crate::labels::GroundTruthLabels;
use crate::signals::Pair;

/// pkcore hand-rank value at or below which a hand is two-pair-or-better
/// (lower rank value = stronger hand).
const TWO_PAIR_OR_BETTER: u16 = 3325;

/// What the card-aware oracle found for one pair: how many spots it examined
/// and how many were genuine EV sacrifices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OracleScore {
    /// Spots where a collusion-relevant decision could be judged.
    pub spots: u32,
    /// Spots where the player gave up EV in the partner's favor.
    pub sacrifices: u32,
}

/// The graded result for one labeled colluding pair.
#[derive(Clone, Debug, PartialEq)]
pub struct PairScore {
    /// The pair graded.
    pub pair: Pair,
    /// The pair's display names, for the report.
    pub names: (String, String),
    /// Hand index at which the blind Boss flagged the pair, if it did.
    pub hands_to_detection: Option<u32>,
    /// Card-aware oracle upper bound.
    pub oracle: OracleScore,
}

/// A full grading report over a Boss run.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreReport {
    /// One entry per labeled colluding pair.
    pub labeled: Vec<PairScore>,
    /// Honest pairs the Boss wrongly flagged.
    pub false_positives: Vec<Pair>,
    /// Total unlabeled (honest) pairs among the verdicts.
    pub honest_pairs: u32,
    /// False-positive rate: `false_positives / honest_pairs` (0 when none).
    pub fp_rate: f64,
}

/// Grades a Boss run against ground truth.
///
/// Reads hole cards (grading tier only). Returns per-pair hands-to-detection,
/// the oracle score, and the false-positive rate over honest pairs.
///
/// # Examples
///
/// ```
/// use pkcore::hand_history::HandCollection;
/// use pkdealer_boss::labels::GroundTruthLabels;
/// use pkdealer_boss::scorer::score;
///
/// let report = score(
///     &HandCollection::new(),
///     &GroundTruthLabels { colluding_pairs: vec![] },
///     &[],
/// );
/// assert!(report.labeled.is_empty());
/// assert_eq!(report.fp_rate, 0.0);
/// ```
#[must_use]
pub fn score(
    collection: &HandCollection,
    labels: &GroundTruthLabels,
    verdicts: &[Verdict],
) -> ScoreReport {
    let labeled = labels
        .colluding_pairs
        .iter()
        .map(|lp| {
            let pair = Pair::new(lp.a, lp.b);
            let hands_to_detection = verdicts
                .iter()
                .find(|v| v.pair == pair)
                .and_then(|v| v.flagged_at_hand);
            PairScore {
                pair,
                names: (lp.a_name.clone(), lp.b_name.clone()),
                hands_to_detection,
                oracle: oracle_for_pair(collection, lp.a, lp.b),
            }
        })
        .collect();

    let mut false_positives = Vec::new();
    let mut honest_pairs = 0u32;
    for verdict in verdicts {
        if !labels.is_colluding(verdict.pair.a, verdict.pair.b) {
            honest_pairs += 1;
            if verdict.flagged_at_hand.is_some() {
                false_positives.push(verdict.pair);
            }
        }
    }
    let fp_count = u32::try_from(false_positives.len()).unwrap_or(u32::MAX);
    let fp_rate = if honest_pairs > 0 {
        f64::from(fp_count) / f64::from(honest_pairs)
    } else {
        0.0
    };

    ScoreReport {
        labeled,
        false_positives,
        honest_pairs,
        fp_rate,
    }
}

/// Runs the EV-sacrifice oracle for one labeled pair across the session.
fn oracle_for_pair(collection: &HandCollection, a: Uuid, b: Uuid) -> OracleScore {
    let mut score = OracleScore::default();
    for hand in collection.hands() {
        let Some(board) = hand.board.as_deref() else {
            continue;
        };
        if board.split_whitespace().count() != 5 {
            continue; // needs a complete 5-card board
        }
        let (Some(a_entry), Some(b_entry)) = (find_player(hand, a), find_player(hand, b)) else {
            continue;
        };
        let (Some(a_hole), Some(b_hole)) =
            (a_entry.hole_cards.as_deref(), b_entry.hole_cards.as_deref())
        else {
            continue;
        };
        let (Some(a_rank), Some(b_rank)) = (rank7(a_hole, board), rank7(b_hole, board)) else {
            continue;
        };
        let actions = all_actions(hand);

        // Fold-the-better-hand: a member folds while the partner is committed.
        for (folder, folder_rank, partner, partner_rank) in
            [(a, a_rank, b, b_rank), (b, b_rank, a, a_rank)]
        {
            let folded = actions
                .iter()
                .any(|ac| ac.player_id == Some(folder) && ac.action == ActionType::Fold);
            let partner_committed = actions
                .iter()
                .any(|ac| ac.player_id == Some(partner) && ac.amount.unwrap_or(0.0) > 0.0);
            if folded && partner_committed {
                score.spots += 1;
                if folder_rank < partner_rank {
                    // Folder held the stronger hand — a genuine sacrifice.
                    score.sacrifices += 1;
                }
            }
        }

        // Passive-strong: in a pair-only pot, a member slow-plays a big hand.
        if pair_only_pot(hand, a, b) {
            for (member, member_rank) in [(a, a_rank), (b, b_rank)] {
                if member_rank <= TWO_PAIR_OR_BETTER {
                    score.spots += 1;
                    let aggressive = actions
                        .iter()
                        .any(|ac| ac.player_id == Some(member) && is_aggressive(&ac.action));
                    if !aggressive {
                        score.sacrifices += 1;
                    }
                }
            }
        }
    }
    score
}

fn find_player(hand: &HandHistory, id: Uuid) -> Option<&PlayerEntry> {
    hand.players.iter().find(|p| p.player_id == Some(id))
}

fn all_actions(hand: &HandHistory) -> Vec<&Action> {
    let mut actions = Vec::new();
    if let Some(streets) = &hand.streets {
        if let Some(s) = &streets.preflop {
            actions.extend(s.actions.iter());
        }
        if let Some(s) = &streets.flop {
            actions.extend(s.actions.iter());
        }
        if let Some(s) = &streets.turn {
            actions.extend(s.actions.iter());
        }
        if let Some(s) = &streets.river {
            actions.extend(s.actions.iter());
        }
    }
    actions
}

fn is_aggressive(action: &ActionType) -> bool {
    matches!(
        action,
        ActionType::Bet | ActionType::Raise | ActionType::AllIn
    )
}

/// Whether the pair are the only players who reached the end without folding.
fn pair_only_pot(hand: &HandHistory, a: Uuid, b: Uuid) -> bool {
    let folded: HashSet<Uuid> = all_actions(hand)
        .iter()
        .filter(|ac| ac.action == ActionType::Fold)
        .filter_map(|ac| ac.player_id)
        .collect();
    let non_folders: HashSet<Uuid> = hand
        .players
        .iter()
        .filter_map(|p| p.player_id)
        .filter(|id| !folded.contains(id))
        .collect();
    non_folders.len() == 2 && non_folders.contains(&a) && non_folders.contains(&b)
}

/// Seven-card rank value for `hole` + a complete 5-card `board`; `None` if the
/// combined 7-card string does not parse.
fn rank7(hole: &str, board: &str) -> Option<u16> {
    Seven::from_str(&format!("{hole} {board}"))
        .ok()
        .map(|s| s.hand_rank_value())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::{SprtParams, assess};
    use crate::fixtures::{self, GTO, MALLORY, TAG, TRUDY};
    use crate::labels::{GroundTruthLabels, LabelStyle, LabelVector, LabeledPair};
    use crate::redacted::redact;
    use crate::signals::Pair;
    use pkcore::analysis::player_stats::Confidence;
    use pkcore::hand_history::ActionType;

    #[test]
    fn scorer_reports_hands_to_detection() {
        let c = fixtures::collection(fixtures::dump_corpus(120));
        let hands = redact(&c);
        let verdicts = assess(&hands, &SprtParams::default());
        let labels = GroundTruthLabels::resolve(
            &c,
            &[(
                "mallory_1".into(),
                "trudy_1".into(),
                LabelVector::Spectator,
                LabelStyle::ChipDump,
            )],
        )
        .unwrap();
        let report = score(&c, &labels, &verdicts);
        assert!(report.labeled[0].hands_to_detection.is_some());
        assert!(report.false_positives.is_empty());
        assert!(report.fp_rate.abs() < f64::EPSILON);
    }

    /// EPIC-70 Phase 3c — A/B channel parity, at the detection + grading tier.
    ///
    /// The Boss must catch the *behavior*, not the channel (exit criterion 5).
    /// Two facts make that structural here rather than a matter of discipline:
    ///
    /// 1. `detector::assess` takes only `&[RedactedHand]` — it never receives a
    ///    channel/vector label, so it *cannot* branch on it. The verdicts below
    ///    are computed once and shared by both gradings.
    /// 2. `scorer::score` receives `GroundTruthLabels` (which do carry
    ///    `LabelVector`), but grades on the pair UUIDs alone. This test pins
    ///    that: the *same* corpus and verdicts, graded against labels differing
    ///    **only** in `LabelVector::Spectator` vs `Peer`, must yield a
    ///    byte-identical `ScoreReport`.
    ///
    /// Honest caveat: this is parity at the redacted/label layer, where Vector A
    /// and Vector B are identical by construction — the firewall erases how the
    /// cards arrived. The full behavioral parity claim (same table dynamics over
    /// two genuinely different wires) still needs a live Vector-B arena run
    /// (Phase 3 integration + Phase 5); this guards the offline pipeline against
    /// ever growing a channel-dependent branch.
    #[test]
    fn vector_label_does_not_change_the_grade() {
        let c = fixtures::collection(fixtures::dump_corpus(120));
        let hands = redact(&c);
        let verdicts = assess(&hands, &SprtParams::default());

        let grade_under = |vector| {
            let labels = GroundTruthLabels::resolve(
                &c,
                &[(
                    "mallory_1".into(),
                    "trudy_1".into(),
                    vector,
                    LabelStyle::ChipDump,
                )],
            )
            .unwrap();
            score(&c, &labels, &verdicts)
        };

        let spectator = grade_under(LabelVector::Spectator);
        let peer = grade_under(LabelVector::Peer);
        assert_eq!(spectator, peer);
        // Non-vacuity: both grade a *flagged* pair, so the equality is over a
        // real detection, not two empty reports.
        assert!(spectator.labeled[0].hands_to_detection.is_some());
    }

    #[test]
    fn oracle_ev_sacrifice_scores_softplay() {
        // Pair-only pot, mallory checks down AA on a KK board (two pair or
        // better) vs trudy → at least 1 spot and 1 sacrifice.
        let hand = fixtures::build_hand(fixtures::HandSpec {
            no: 1,
            players: vec![
                fixtures::player(0, "mallory_1", MALLORY, 10_000.0, Some("A♠ A♥")),
                fixtures::player(1, "trudy_1", TRUDY, 10_000.0, Some("Q♠ Q♥")),
            ],
            preflop: vec![
                fixtures::act(0, MALLORY, ActionType::Call, Some(100.0)),
                fixtures::act(1, TRUDY, ActionType::Check, None),
            ],
            flop: Some((
                "K♠ K♥ 7♦".into(),
                vec![
                    fixtures::act(0, MALLORY, ActionType::Check, None),
                    fixtures::act(1, TRUDY, ActionType::Check, None),
                ],
            )),
            turn: Some((
                "3♣".into(),
                vec![
                    fixtures::act(0, MALLORY, ActionType::Check, None),
                    fixtures::act(1, TRUDY, ActionType::Check, None),
                ],
            )),
            river: Some((
                "2♦".into(),
                vec![
                    fixtures::act(0, MALLORY, ActionType::Check, None),
                    fixtures::act(1, TRUDY, ActionType::Check, None),
                ],
            )),
            nets: vec![(0, 100.0), (1, -100.0)],
        });
        let c = fixtures::collection(vec![hand]);
        let labels = GroundTruthLabels::resolve(
            &c,
            &[(
                "mallory_1".into(),
                "trudy_1".into(),
                LabelVector::Spectator,
                LabelStyle::SoftPlay,
            )],
        )
        .unwrap();
        let report = score(&c, &labels, &[]);
        assert!(report.labeled[0].oracle.spots >= 1);
        assert!(report.labeled[0].oracle.sacrifices >= 1);
    }

    #[test]
    fn oracle_counts_fold_of_better_hand() {
        // mallory folds AA on the river while committed trudy holds QQ →
        // sacrifice (aces beat queens on a K-high rainbow board).
        let hand = fixtures::build_hand(fixtures::HandSpec {
            no: 1,
            players: vec![
                fixtures::player(0, "mallory_1", MALLORY, 10_000.0, Some("A♠ A♥")),
                fixtures::player(1, "trudy_1", TRUDY, 10_000.0, Some("Q♠ Q♥")),
            ],
            preflop: vec![
                fixtures::act(0, MALLORY, ActionType::Call, Some(100.0)),
                fixtures::act(1, TRUDY, ActionType::Check, None),
            ],
            flop: Some((
                "K♠ 8♥ 7♦".into(),
                vec![
                    fixtures::act(0, MALLORY, ActionType::Check, None),
                    fixtures::act(1, TRUDY, ActionType::Check, None),
                ],
            )),
            turn: Some((
                "3♣".into(),
                vec![
                    fixtures::act(0, MALLORY, ActionType::Check, None),
                    fixtures::act(1, TRUDY, ActionType::Check, None),
                ],
            )),
            river: Some((
                "2♦".into(),
                vec![
                    fixtures::act(1, TRUDY, ActionType::Bet, Some(400.0)),
                    fixtures::act(0, MALLORY, ActionType::Fold, None),
                ],
            )),
            nets: vec![(0, -100.0), (1, 100.0)],
        });
        let c = fixtures::collection(vec![hand]);
        let labels = GroundTruthLabels::resolve(
            &c,
            &[(
                "mallory_1".into(),
                "trudy_1".into(),
                LabelVector::Spectator,
                LabelStyle::ChipDump,
            )],
        )
        .unwrap();
        let report = score(&c, &labels, &[]);
        assert!(report.labeled[0].oracle.sacrifices >= 1);
    }

    #[test]
    fn honest_flag_counts_as_false_positive() {
        // A verdict flagging (GTO, TAG) with empty labels → fp_rate = 1.0.
        let c = fixtures::collection(fixtures::honest_corpus(8));
        let labels = GroundTruthLabels {
            colluding_pairs: vec![],
        };
        let fake = Verdict {
            pair: Pair::new(GTO, TAG),
            llr: 9.0,
            hands_observed: 60,
            confidence: Confidence::Medium,
            flagged_at_hand: Some(55),
        };
        let report = score(&c, &labels, &[fake]);
        assert_eq!(report.false_positives.len(), 1);
        assert!((report.fp_rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn labeled_pair_carries_names() {
        let c = fixtures::collection(fixtures::dump_corpus(4));
        let labels = GroundTruthLabels {
            colluding_pairs: vec![LabeledPair {
                a: MALLORY,
                b: TRUDY,
                a_name: "mallory_1".into(),
                b_name: "trudy_1".into(),
                vector: LabelVector::Spectator,
                style: LabelStyle::ChipDump,
            }],
        };
        let report = score(&c, &labels, &[]);
        assert_eq!(report.labeled[0].names.0, "mallory_1");
        assert_eq!(report.labeled[0].names.1, "trudy_1");
    }
}
