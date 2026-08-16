//! The typed hole-card firewall: what an honest observer may see.

use pkcore::hand_history::{Action, ActionType, HandCollection, HandHistory};
use serde::Serialize;
use uuid::Uuid;

/// Betting street a redacted action belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RedactedStreet {
    /// Before the flop.
    Preflop,
    /// Three community cards dealt.
    Flop,
    /// Fourth community card dealt.
    Turn,
    /// Fifth community card dealt.
    River,
}

/// One seated player's public state in a [`RedactedHand`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RedactedSeat {
    /// Stable player identity (the recorder's `PlayerEntry.player_id`).
    pub player_id: Uuid,
    /// Seat number as recorded.
    pub seat: u8,
    /// Display name — public information at any table.
    pub name: String,
    /// Stack at hand start.
    pub starting_stack: f64,
    /// Net chips won (positive) or lost (negative) this hand.
    pub net: f64,
}

/// One public betting action in a [`RedactedHand`].
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RedactedAction {
    /// Acting player's stable identity.
    pub player_id: Uuid,
    /// Acting player's seat.
    pub seat: u8,
    /// Street the action happened on.
    pub street: RedactedStreet,
    /// The action taken (fold/check/call/bet/raise/post/all-in).
    pub action: ActionType,
    /// Amount wagered, when the action carries one.
    pub amount: Option<f64>,
    /// Whether the actor was all-in after this action.
    pub all_in: bool,
}

/// A single completed hand as an honest observer may see it: public actions
/// and chip movements, with every hole card and the deck structurally
/// removed. **There is no field that can hold a hole card.**
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RedactedHand {
    /// 1-based position of the hand within the recorded session.
    pub hand_no: u32,
    /// Dealer-button seat, when recorded.
    pub button_seat: Option<u8>,
    /// Big-blind amount for this hand.
    pub big_blind: f64,
    /// All seated players and their public per-hand outcomes.
    pub seats: Vec<RedactedSeat>,
    /// Every betting action across all streets, in order.
    pub actions: Vec<RedactedAction>,
    /// Community cards — dealt face-up, therefore public.
    pub board: Option<String>,
}

impl RedactedHand {
    /// Looks up the seat entry for `player_id`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::redacted::{RedactedHand, RedactedSeat};
    /// use uuid::Uuid;
    ///
    /// let alice = Uuid::from_u128(1);
    /// let hand = RedactedHand {
    ///     hand_no: 1,
    ///     button_seat: Some(0),
    ///     big_blind: 100.0,
    ///     seats: vec![RedactedSeat {
    ///         player_id: alice,
    ///         seat: 0,
    ///         name: "alice".to_string(),
    ///         starting_stack: 1_000.0,
    ///         net: 0.0,
    ///     }],
    ///     actions: vec![],
    ///     board: None,
    /// };
    /// assert_eq!(hand.seat_of(alice).map(|s| s.seat), Some(0));
    /// assert!(hand.seat_of(Uuid::from_u128(2)).is_none());
    /// ```
    #[must_use]
    pub fn seat_of(&self, player_id: Uuid) -> Option<&RedactedSeat> {
        self.seats.iter().find(|s| s.player_id == player_id)
    }

    /// All player ids dealt into this hand.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::redacted::{RedactedHand, RedactedSeat};
    /// use uuid::Uuid;
    ///
    /// let alice = Uuid::from_u128(1);
    /// let hand = RedactedHand {
    ///     hand_no: 1,
    ///     button_seat: Some(0),
    ///     big_blind: 100.0,
    ///     seats: vec![RedactedSeat {
    ///         player_id: alice,
    ///         seat: 0,
    ///         name: "alice".to_string(),
    ///         starting_stack: 1_000.0,
    ///         net: 0.0,
    ///     }],
    ///     actions: vec![],
    ///     board: None,
    /// };
    /// assert_eq!(hand.player_ids(), vec![alice]);
    /// ```
    #[must_use]
    pub fn player_ids(&self) -> Vec<Uuid> {
        self.seats.iter().map(|s| s.player_id).collect()
    }
}

/// The ONLY constructor for redacted hands. Consumes a [`HandCollection`],
/// dropping `hole_cards`, `hole_cards_visibility`, `best_hand`, and
/// `shuffled_deck` at the boundary. Once redacted, the cards are gone.
///
/// Hands where any seat lacks a `player_id` (legacy/manual records) are
/// skipped entirely — pairwise detection is meaningless without stable
/// identity.
///
/// The detection API cannot accept the un-redacted collection:
///
/// ```compile_fail
/// use pkcore::hand_history::HandCollection;
/// // signals/detector take &[RedactedHand]; a HandCollection does not coerce.
/// let hands: &[pkdealer_boss::redacted::RedactedHand] = &HandCollection::new();
/// ```
///
/// # Examples
///
/// ```
/// use pkdealer_boss::redacted::redact;
/// use pkcore::hand_history::HandCollection;
///
/// assert!(redact(&HandCollection::new()).is_empty());
/// ```
#[must_use]
pub fn redact(collection: &HandCollection) -> Vec<RedactedHand> {
    collection
        .hands()
        .iter()
        .enumerate()
        .filter_map(|(index, hand)| redact_hand(index, hand))
        .collect()
}

#[allow(clippy::cast_possible_truncation)]
fn redact_hand(index: usize, hand: &HandHistory) -> Option<RedactedHand> {
    let seats: Option<Vec<RedactedSeat>> = hand
        .players
        .iter()
        .map(|p| {
            let net = hand
                .results
                .as_ref()
                .and_then(|rs| rs.iter().find(|r| r.seat == p.seat))
                .and_then(|r| r.net)
                .unwrap_or(0.0);
            p.player_id.map(|player_id| RedactedSeat {
                player_id,
                seat: p.seat,
                name: p.name.clone(),
                starting_stack: p.stack,
                net,
            })
        })
        .collect();
    let seats = seats?; // any missing identity ⇒ skip the hand
    let seat_ids: std::collections::HashMap<u8, Uuid> =
        seats.iter().map(|s| (s.seat, s.player_id)).collect();

    let mut actions = Vec::new();
    if let Some(streets) = &hand.streets {
        let buckets: [(RedactedStreet, Option<&Vec<Action>>); 4] = [
            (
                RedactedStreet::Preflop,
                streets.preflop.as_ref().map(|s| &s.actions),
            ),
            (
                RedactedStreet::Flop,
                streets.flop.as_ref().map(|s| &s.actions),
            ),
            (
                RedactedStreet::Turn,
                streets.turn.as_ref().map(|s| &s.actions),
            ),
            (
                RedactedStreet::River,
                streets.river.as_ref().map(|s| &s.actions),
            ),
        ];
        for (street, bucket) in buckets {
            for action in bucket.into_iter().flatten() {
                let Some(player_id) = action
                    .player_id
                    .or_else(|| seat_ids.get(&action.seat).copied())
                else {
                    continue;
                };
                actions.push(RedactedAction {
                    player_id,
                    seat: action.seat,
                    street,
                    action: action.action.clone(),
                    amount: action.amount,
                    all_in: action.all_in.unwrap_or(false),
                });
            }
        }
    }

    Some(RedactedHand {
        hand_no: (index + 1) as u32,
        button_seat: hand.table.button,
        big_blind: hand.table.stakes.big_blind,
        seats,
        actions,
        board: hand.board.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{self, GTO, MALLORY, TAG, TRUDY};
    use pkcore::hand_history::ActionType;

    fn two_hand_collection() -> pkcore::hand_history::HandCollection {
        let h1 = fixtures::build_hand(fixtures::HandSpec {
            no: 1,
            players: vec![
                fixtures::player(0, "mallory_1", MALLORY, 10_000.0, Some("A♠ A♥")),
                fixtures::player(1, "trudy_1", TRUDY, 10_000.0, Some("K♠ K♥")),
                fixtures::player(2, "gto_1", GTO, 10_000.0, Some("7♦ 2♣")),
            ],
            preflop: vec![
                fixtures::act(2, GTO, ActionType::Fold, None),
                fixtures::act(0, MALLORY, ActionType::Raise, Some(300.0)),
                fixtures::act(1, TRUDY, ActionType::Call, Some(300.0)),
            ],
            flop: Some((
                "Q♣ 6♦ 5♥".to_string(),
                vec![
                    fixtures::act(0, MALLORY, ActionType::Bet, Some(400.0)),
                    fixtures::act(1, TRUDY, ActionType::Fold, None),
                ],
            )),
            turn: None,
            river: None,
            nets: vec![(0, 450.0), (1, -400.0), (2, -50.0)],
        });
        let h2 = fixtures::build_hand(fixtures::HandSpec {
            no: 2,
            players: vec![
                fixtures::player(0, "mallory_1", MALLORY, 10_450.0, Some("9♠ 9♥")),
                fixtures::player(1, "trudy_1", TRUDY, 9_600.0, Some("8♠ 8♥")),
                fixtures::player(3, "tag_1", TAG, 10_000.0, Some("A♦ K♦")),
            ],
            preflop: vec![
                fixtures::act(3, TAG, ActionType::Raise, Some(300.0)),
                fixtures::act(0, MALLORY, ActionType::Fold, None),
                fixtures::act(1, TRUDY, ActionType::Call, Some(300.0)),
            ],
            flop: None,
            turn: None,
            river: None,
            nets: vec![(0, 0.0), (1, -300.0), (3, 300.0)],
        });
        fixtures::collection(vec![h1, h2])
    }

    #[test]
    fn redact_drops_hole_cards() {
        let hands = redact(&two_hand_collection());
        let json = serde_json::to_string(&hands).unwrap();
        // Every planted secret must be gone; suits appear only via the board.
        for secret in [
            "A♠ A♥",
            "K♠ K♥",
            "7♦ 2♣",
            "9♠ 9♥",
            "8♠ 8♥",
            "A♦ K♦",
            "XX-DECK-MARKER-XX",
            "hole",
            "deck",
        ] {
            assert!(!json.contains(secret), "leaked {secret:?} in {json}");
        }
    }

    #[test]
    fn redact_keeps_public_board_and_actions() {
        let hands = redact(&two_hand_collection());
        assert_eq!(hands.len(), 2);
        assert_eq!(hands[0].hand_no, 1);
        assert_eq!(hands[1].hand_no, 2);
        assert_eq!(hands[0].board.as_deref(), Some("Q♣ 6♦ 5♥"));
        assert_eq!(hands[0].actions.len(), 5);
        assert_eq!(hands[0].actions[3].street, RedactedStreet::Flop);
        assert_eq!(hands[0].actions[3].action, ActionType::Bet);
        assert!((hands[0].actions[3].amount.unwrap() - 400.0).abs() < f64::EPSILON);
        assert!((hands[0].big_blind - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn redact_maps_seats_nets_and_ids() {
        let hands = redact(&two_hand_collection());
        let mallory = hands[0].seat_of(MALLORY).unwrap();
        assert_eq!(mallory.seat, 0);
        assert_eq!(mallory.name, "mallory_1");
        assert!((mallory.net - 450.0).abs() < f64::EPSILON);
        assert_eq!(hands[1].player_ids().len(), 3);
    }

    #[test]
    fn redact_skips_hands_without_player_identity() {
        let mut anonymous = fixtures::build_hand(fixtures::HandSpec {
            no: 1,
            players: vec![fixtures::player(0, "a", MALLORY, 1_000.0, None)],
            preflop: vec![],
            flop: None,
            turn: None,
            river: None,
            nets: vec![(0, 0.0)],
        });
        anonymous.players[0].player_id = None;
        let hands = redact(&fixtures::collection(vec![anonymous]));
        assert!(
            hands.is_empty(),
            "identity-less hands cannot be attributed pairwise"
        );
    }

    #[test]
    fn redact_empty_collection_is_empty() {
        assert!(redact(&pkcore::hand_history::HandCollection::new()).is_empty());
    }
}
