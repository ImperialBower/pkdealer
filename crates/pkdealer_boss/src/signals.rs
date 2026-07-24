//! Pairwise public-information signals over redacted hands.
//!
//! This module is **blind by construction**: it imports only
//! [`crate::redacted`] types plus pkcore's card-free [`ActionType`] enum and
//! stat aggregates. Nothing here can read a hole card. Every signal is
//! computed from the observed session alone — no counterfactual — which is
//! exactly why the Boss can run them live.

use std::collections::{HashMap, HashSet};

use pkcore::analysis::player_stats::PlayerStats;
use pkcore::hand_history::ActionType;
use uuid::Uuid;

use crate::redacted::{RedactedHand, RedactedStreet};

/// An unordered pair of players, normalized so `a <= b`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pair {
    /// The lesser player id.
    pub a: Uuid,
    /// The greater player id.
    pub b: Uuid,
}

impl Pair {
    /// Builds a normalized pair (order-insensitive).
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::signals::Pair;
    /// use uuid::Uuid;
    /// let (x, y) = (Uuid::from_u128(1), Uuid::from_u128(2));
    /// assert_eq!(Pair::new(x, y), Pair::new(y, x));
    /// ```
    #[must_use]
    pub fn new(x: Uuid, y: Uuid) -> Self {
        if x <= y {
            Self { a: x, b: y }
        } else {
            Self { a: y, b: x }
        }
    }

    /// Returns `true` when `id` is one of the two members.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::signals::Pair;
    /// use uuid::Uuid;
    /// let p = Pair::new(Uuid::from_u128(1), Uuid::from_u128(2));
    /// assert!(p.contains(Uuid::from_u128(1)));
    /// assert!(!p.contains(Uuid::from_u128(3)));
    /// ```
    #[must_use]
    pub fn contains(&self, id: Uuid) -> bool {
        self.a == id || self.b == id
    }
}

/// Directional chip flow resolved from a pair-only pot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PairPotOutcome {
    /// `a` lost chips to `b`; carries the amount transferred.
    FlowAtoB(f64),
    /// `b` lost chips to `a`; carries the amount transferred.
    FlowBtoA(f64),
    /// The pair contested the pot but neither cleanly won from the other.
    Neutral,
}

/// One hand's observations for a single pair.
#[derive(Clone, Debug, PartialEq)]
pub struct PairHandObs {
    /// The hand's monotonic number.
    pub hand_no: u32,
    /// Whether both pair members were dealt into this hand.
    pub both_dealt: bool,
    /// `(member, is_aggressive)` for voluntary actions taken while the live
    /// set was exactly the two pair members (heads-up together).
    pub hu_actions: Vec<(Uuid, bool)>,
    /// `(member, is_aggressive)` for voluntary actions in every other live-set
    /// configuration (the member's baseline).
    pub baseline_actions: Vec<(Uuid, bool)>,
    /// Number of whipsaw (raise → partner-re-raise → field-folds) patterns.
    pub whipsaw_events: u32,
    /// Directional flow when the pot was contested by the pair alone.
    pub pair_pot: Option<PairPotOutcome>,
}

/// Aggregate signals for one pair across a session.
#[derive(Clone, Debug, PartialEq)]
pub struct PairSignals {
    /// The pair these signals describe.
    pub pair: Pair,
    /// Hands in which both members were dealt.
    pub hands_together: u32,
    /// Number of pots the pair contested alone.
    pub pair_pots: u32,
    /// Net directional chip flow (positive ⇒ from `a` to `b`).
    pub net_flow_a_to_b: f64,
    /// Heads-up aggression rate divided by baseline aggression rate; `None`
    /// when either bucket is empty (catches soft-play as a value well below 1).
    pub soft_play_index: Option<f64>,
    /// Total whipsaw patterns across the session.
    pub whipsaw_count: u32,
    /// Members' VPIP rate in hands where the partner also played, aggregated
    /// over both members.
    pub vpip_with_partner: Option<f64>,
    /// Members' VPIP rate in hands where the partner folded, aggregated over
    /// both members.
    pub vpip_without_partner: Option<f64>,
}

/// A voluntary action is any action except a forced [`ActionType::Post`].
fn is_voluntary(action: &ActionType) -> bool {
    !matches!(action, ActionType::Post)
}

/// An aggressive action opens or raises the betting.
fn is_aggressive(action: &ActionType) -> bool {
    matches!(
        action,
        ActionType::Bet | ActionType::Raise | ActionType::AllIn
    )
}

/// A chip-committing action beyond the blinds.
fn invests(action: &ActionType) -> bool {
    matches!(
        action,
        ActionType::Call | ActionType::Bet | ActionType::Raise | ActionType::AllIn
    )
}

fn street_index(street: RedactedStreet) -> usize {
    match street {
        RedactedStreet::Preflop => 0,
        RedactedStreet::Flop => 1,
        RedactedStreet::Turn => 2,
        RedactedStreet::River => 3,
    }
}

/// Observes a single hand for one pair, bucketing the pair's voluntary actions
/// into heads-up-together vs baseline, counting whipsaw patterns, and resolving
/// any pair-only chip flow.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::redacted::RedactedHand;
/// use pkdealer_boss::signals::{observe_hand, Pair};
/// use uuid::Uuid;
///
/// let hand = RedactedHand {
///     hand_no: 1,
///     button_seat: Some(0),
///     big_blind: 100.0,
///     seats: vec![],
///     actions: vec![],
///     board: None,
/// };
/// let pair = Pair::new(Uuid::from_u128(1), Uuid::from_u128(2));
/// let obs = observe_hand(&hand, &pair);
/// assert!(!obs.both_dealt);
/// ```
#[must_use]
pub fn observe_hand(hand: &RedactedHand, pair: &Pair) -> PairHandObs {
    let dealt: HashSet<Uuid> = hand.seats.iter().map(|s| s.player_id).collect();
    let both_dealt = dealt.contains(&pair.a) && dealt.contains(&pair.b);

    let mut folded: HashSet<Uuid> = HashSet::new();
    let mut hu_actions = Vec::new();
    let mut baseline_actions = Vec::new();

    for action in &hand.actions {
        if is_voluntary(&action.action) && pair.contains(action.player_id) {
            let live_count = dealt.len() - folded.len();
            let live_is_pair =
                live_count == 2 && !folded.contains(&pair.a) && !folded.contains(&pair.b);
            let entry = (action.player_id, is_aggressive(&action.action));
            if live_is_pair {
                hu_actions.push(entry);
            } else {
                baseline_actions.push(entry);
            }
        }
        if action.action == ActionType::Fold {
            folded.insert(action.player_id);
        }
    }

    PairHandObs {
        hand_no: hand.hand_no,
        both_dealt,
        hu_actions,
        baseline_actions,
        whipsaw_events: count_whipsaw(hand, pair, &dealt),
        pair_pot: pair_pot_outcome(hand, pair),
    }
}

/// Counts whipsaw patterns: within a street, a pair member bets/raises, the
/// other member re-raises while a third party is still live, and after the
/// re-raise no third party takes any voluntary action but folding.
fn count_whipsaw(hand: &RedactedHand, pair: &Pair, dealt: &HashSet<Uuid>) -> u32 {
    let mut events = 0;
    for street in [
        RedactedStreet::Preflop,
        RedactedStreet::Flop,
        RedactedStreet::Turn,
        RedactedStreet::River,
    ] {
        let idxs: Vec<usize> = hand
            .actions
            .iter()
            .enumerate()
            .filter(|(_, a)| a.street == street)
            .map(|(k, _)| k)
            .collect();
        if street_has_whipsaw(hand, pair, dealt, &idxs) {
            events += 1;
        }
    }
    events
}

fn street_has_whipsaw(
    hand: &RedactedHand,
    pair: &Pair,
    dealt: &HashSet<Uuid>,
    idxs: &[usize],
) -> bool {
    for (offset, &i) in idxs.iter().enumerate() {
        let opener = &hand.actions[i];
        if !(pair.contains(opener.player_id)
            && matches!(opener.action, ActionType::Bet | ActionType::Raise))
        {
            continue;
        }
        let other = if opener.player_id == pair.a {
            pair.b
        } else {
            pair.a
        };
        for &j in &idxs[offset + 1..] {
            let reraise = &hand.actions[j];
            if !(reraise.player_id == other && reraise.action == ActionType::Raise) {
                continue;
            }
            let folded_before_j: HashSet<Uuid> = hand.actions[..j]
                .iter()
                .filter(|a| a.action == ActionType::Fold)
                .map(|a| a.player_id)
                .collect();
            let third_party_live = dealt
                .iter()
                .any(|p| !pair.contains(*p) && !folded_before_j.contains(p));
            if !third_party_live {
                continue;
            }
            let field_folds = hand.actions[j + 1..].iter().all(|a| {
                pair.contains(a.player_id)
                    || a.action == ActionType::Fold
                    || !is_voluntary(&a.action)
            });
            if field_folds {
                return true;
            }
        }
    }
    false
}

/// Resolves directional chip flow when the pot was contested by the pair
/// alone (only the two members committed chips; everyone else folded without
/// investing).
fn pair_pot_outcome(hand: &RedactedHand, pair: &Pair) -> Option<PairPotOutcome> {
    let investors: HashSet<Uuid> = hand
        .actions
        .iter()
        .filter(|a| invests(&a.action))
        .map(|a| a.player_id)
        .collect();
    if !(investors.len() == 2 && investors.contains(&pair.a) && investors.contains(&pair.b)) {
        return None;
    }
    let net_a = hand.seat_of(pair.a).map_or(0.0, |s| s.net);
    let net_b = hand.seat_of(pair.b).map_or(0.0, |s| s.net);
    if net_a < 0.0 && net_b > 0.0 {
        Some(PairPotOutcome::FlowAtoB(net_b.min(-net_a)))
    } else if net_a > 0.0 && net_b < 0.0 {
        Some(PairPotOutcome::FlowBtoA(net_a.min(-net_b)))
    } else {
        Some(PairPotOutcome::Neutral)
    }
}

/// Enumerates every unordered pair of players seen across the session.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::signals::pairs_in;
/// assert!(pairs_in(&[]).is_empty());
/// ```
#[must_use]
pub fn pairs_in(hands: &[RedactedHand]) -> Vec<Pair> {
    let mut ids: Vec<Uuid> = Vec::new();
    let mut seen: HashSet<Uuid> = HashSet::new();
    for hand in hands {
        for seat in &hand.seats {
            if seen.insert(seat.player_id) {
                ids.push(seat.player_id);
            }
        }
    }
    ids.sort();
    let mut pairs = Vec::new();
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            pairs.push(Pair::new(ids[i], ids[j]));
        }
    }
    pairs
}

fn member_vpip_preflop(hand: &RedactedHand, id: Uuid) -> bool {
    hand.actions
        .iter()
        .any(|a| a.player_id == id && a.street == RedactedStreet::Preflop && invests(&a.action))
}

fn vpip_conditioning(hands: &[RedactedHand], pair: &Pair) -> (Option<f64>, Option<f64>) {
    let (mut with_num, mut with_den) = (0u32, 0u32);
    let (mut without_num, mut without_den) = (0u32, 0u32);
    for hand in hands {
        let dealt: HashSet<Uuid> = hand.seats.iter().map(|s| s.player_id).collect();
        if !(dealt.contains(&pair.a) && dealt.contains(&pair.b)) {
            continue;
        }
        let a_vpip = member_vpip_preflop(hand, pair.a);
        let b_vpip = member_vpip_preflop(hand, pair.b);
        // member a conditioned on b, then member b conditioned on a.
        for (this, other) in [(a_vpip, b_vpip), (b_vpip, a_vpip)] {
            if other {
                with_den += 1;
                if this {
                    with_num += 1;
                }
            } else {
                without_den += 1;
                if this {
                    without_num += 1;
                }
            }
        }
    }
    let rate = |num: u32, den: u32| (den > 0).then(|| f64::from(num) / f64::from(den));
    (rate(with_num, with_den), rate(without_num, without_den))
}

/// Aggregates every pairwise signal for one pair across the whole session.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::signals::{aggregate, Pair};
/// use uuid::Uuid;
/// let signals = aggregate(&[], &Pair::new(Uuid::from_u128(1), Uuid::from_u128(2)));
/// assert_eq!(signals.hands_together, 0);
/// ```
#[must_use]
pub fn aggregate(hands: &[RedactedHand], pair: &Pair) -> PairSignals {
    let mut hands_together = 0u32;
    let mut pair_pots = 0u32;
    let mut net_flow = 0.0f64;
    let (mut hu_aggr, mut hu_total) = (0u32, 0u32);
    let (mut base_aggr, mut base_total) = (0u32, 0u32);
    let mut whipsaw = 0u32;

    for hand in hands {
        let obs = observe_hand(hand, pair);
        if obs.both_dealt {
            hands_together += 1;
        }
        for (_, aggressive) in &obs.hu_actions {
            hu_total += 1;
            hu_aggr += u32::from(*aggressive);
        }
        for (_, aggressive) in &obs.baseline_actions {
            base_total += 1;
            base_aggr += u32::from(*aggressive);
        }
        whipsaw += obs.whipsaw_events;
        match obs.pair_pot {
            Some(PairPotOutcome::FlowAtoB(x)) => {
                pair_pots += 1;
                net_flow += x;
            }
            Some(PairPotOutcome::FlowBtoA(x)) => {
                pair_pots += 1;
                net_flow -= x;
            }
            Some(PairPotOutcome::Neutral) => pair_pots += 1,
            None => {}
        }
    }

    let soft_play_index = if hu_total > 0 && base_total > 0 {
        let hu_rate = f64::from(hu_aggr) / f64::from(hu_total);
        let base_rate = f64::from(base_aggr) / f64::from(base_total);
        (base_rate > 0.0).then_some(hu_rate / base_rate)
    } else {
        None
    };

    let (vpip_with_partner, vpip_without_partner) = vpip_conditioning(hands, pair);

    PairSignals {
        pair: *pair,
        hands_together,
        pair_pots,
        net_flow_a_to_b: net_flow,
        soft_play_index,
        whipsaw_count: whipsaw,
        vpip_with_partner,
        vpip_without_partner,
    }
}

/// Builds pkcore [`PlayerStats`] populated from public actions only, so
/// `vpip()`, `pfr()`, and `aggression_factor()` resolve. Only the fields
/// derivable without hole cards are set — the rest stay `Default`.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::signals::public_stats;
/// assert!(public_stats(&[]).is_empty());
/// ```
#[must_use]
pub fn public_stats(hands: &[RedactedHand]) -> HashMap<Uuid, PlayerStats> {
    let mut stats: HashMap<Uuid, PlayerStats> = HashMap::new();
    for hand in hands {
        for seat in &hand.seats {
            let entry = stats.entry(seat.player_id).or_default();
            entry.hands_dealt += 1;
            entry.pfr_opportunities += 1;
        }
        let mut voluntary: HashSet<Uuid> = HashSet::new();
        let mut pfred: HashSet<Uuid> = HashSet::new();
        for action in &hand.actions {
            let Some(entry) = stats.get_mut(&action.player_id) else {
                continue;
            };
            let street = street_index(action.street);
            match action.action {
                ActionType::Fold => entry.by_street[street].folds += 1,
                ActionType::Check => entry.by_street[street].checks += 1,
                ActionType::Call => entry.by_street[street].calls += 1,
                ActionType::Bet => entry.by_street[street].bets += 1,
                ActionType::Raise => entry.by_street[street].raises += 1,
                ActionType::AllIn => entry.by_street[street].all_ins += 1,
                // `Post` (blinds/antes) and any future non-exhaustive variant
                // are not tracked in these public counts.
                _ => {}
            }
            if action.street == RedactedStreet::Preflop {
                if invests(&action.action) {
                    voluntary.insert(action.player_id);
                }
                if action.action == ActionType::Raise {
                    pfred.insert(action.player_id);
                }
            }
        }
        for id in voluntary {
            if let Some(entry) = stats.get_mut(&id) {
                entry.hands_voluntarily_played += 1;
            }
        }
        for id in pfred {
            if let Some(entry) = stats.get_mut(&id) {
                entry.pfr_count += 1;
            }
        }
    }
    stats
}

/// Maps each player id to its last-seen display name.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::signals::names_from;
/// assert!(names_from(&[]).is_empty());
/// ```
#[must_use]
pub fn names_from(hands: &[RedactedHand]) -> HashMap<Uuid, String> {
    let mut names = HashMap::new();
    for hand in hands {
        for seat in &hand.seats {
            names.insert(seat.player_id, seat.name.clone());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{self, GTO, MALLORY, TAG, TRUDY};
    use crate::redacted::redact;

    #[test]
    fn pair_new_normalizes_order() {
        assert_eq!(Pair::new(TRUDY, MALLORY), Pair::new(MALLORY, TRUDY));
    }

    #[test]
    fn pairs_in_enumerates_unordered_pairs() {
        let hands = redact(&fixtures::collection(fixtures::honest_corpus(8)));
        assert_eq!(pairs_in(&hands).len(), 6);
    }

    #[test]
    fn metric_chip_flow_flags_dump() {
        let hands = redact(&fixtures::collection(fixtures::dump_corpus(100)));
        let guilty = aggregate(&hands, &Pair::new(MALLORY, TRUDY));
        let honest = aggregate(&hands, &Pair::new(GTO, TAG));
        assert!(
            guilty.net_flow_a_to_b.abs() > 20.0 * 100.0,
            "planted dump flow: {}",
            guilty.net_flow_a_to_b
        );
        assert!(honest.pair_pots == 0 || honest.net_flow_a_to_b.abs() < 500.0);
    }

    #[test]
    fn chipflow_honest_nets_zero() {
        let hands = redact(&fixtures::collection(fixtures::honest_corpus(96)));
        for pair in pairs_in(&hands) {
            let s = aggregate(&hands, &pair);
            assert!(
                s.net_flow_a_to_b.abs() < 300.0,
                "pair {pair:?} drifted: {}",
                s.net_flow_a_to_b
            );
        }
    }

    #[test]
    fn metric_soft_play_index_flags_soft() {
        let hands = redact(&fixtures::collection(fixtures::soft_play_corpus(100)));
        let guilty = aggregate(&hands, &Pair::new(MALLORY, TRUDY))
            .soft_play_index
            .unwrap();
        let honest = aggregate(&hands, &Pair::new(GTO, TAG))
            .soft_play_index
            .unwrap_or(1.0);
        assert!(guilty < 0.5 * honest, "guilty {guilty} vs honest {honest}");
    }

    #[test]
    fn metric_whipsaw_count_flags_whipsaw() {
        let hands = redact(&fixtures::collection(fixtures::whipsaw_corpus(80)));
        assert!(aggregate(&hands, &Pair::new(MALLORY, TRUDY)).whipsaw_count >= 50);
        assert_eq!(aggregate(&hands, &Pair::new(GTO, TAG)).whipsaw_count, 0);
    }

    #[test]
    fn public_stats_resolve_vpip_pfr() {
        let hands = redact(&fixtures::collection(fixtures::honest_corpus(40)));
        let stats = public_stats(&hands);
        let m = &stats[&MALLORY];
        assert!(m.vpip().is_some() && m.pfr().is_some());
    }
}
