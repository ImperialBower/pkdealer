//! The sequential detector: a per-pair Wald SPRT over the public signals.
//!
//! Evidence accumulates as a running log-likelihood ratio (LLR):
//! `LLR += log P(signal | collusion) − log P(signal | honest)` for each
//! soft-play, whipsaw, and chip-flow observation, in `hand_no` order. A pair
//! is flagged the first hand its LLR crosses the Wald upper bound **and** a
//! sample-size floor is met — so the headline metric is *how few hands* until
//! confident, not "given the whole session."
//!
//! The likelihood models here are **pre-calibration defaults** (EPIC-70
//! Phase 5a replaces the numbers from an all-honest control run; the shapes
//! stay). Classic SPRT stops on either bound; we keep accumulating so the
//! report carries session-long evidence, but flagging still requires the
//! upper bound plus the floor.

use std::cmp::Ordering;

use pkcore::analysis::player_stats::Confidence;

use crate::redacted::RedactedHand;
use crate::signals::{Pair, PairPotOutcome, observe_hand, pairs_in};

/// Likelihood-model parameters for the sequential test. All ratios are
/// **pre-calibration defaults** (EPIC-70 Phase 5a fits them from an all-honest
/// control run); only the numbers change, not the model shapes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SprtParams {
    /// Target false-positive rate (Wald α).
    pub alpha: f64,
    /// Target false-negative rate (Wald β).
    pub beta: f64,
    /// Honest heads-up aggression assumed until a member's baseline warms up.
    pub default_honest_aggr: f64,
    /// Baseline sample size required before a member's own rate is trusted.
    pub min_baseline_actions: u32,
    /// A colluder's heads-up aggression is `discount × own baseline`.
    pub soft_play_discount: f64,
    /// P(hand has ≥1 whipsaw pattern | honest).
    pub whipsaw_honest: f64,
    /// P(hand has ≥1 whipsaw pattern | colluding).
    pub whipsaw_colluding: f64,
    /// P(pair-pot flow matches the running majority | honest).
    pub flow_honest: f64,
    /// P(pair-pot flow matches the running majority | colluding).
    pub flow_colluding: f64,
    /// Minimum hands-together before a flag may fire. Set to the pkcore
    /// [`Confidence`] Low/Medium boundary (50), so a flagged pair is never in
    /// the `Low` band.
    pub min_hands: u32,
}

impl Default for SprtParams {
    fn default() -> Self {
        Self {
            alpha: 0.01,
            beta: 0.10,
            default_honest_aggr: 0.40,
            min_baseline_actions: 20,
            soft_play_discount: 0.25,
            whipsaw_honest: 0.02,
            whipsaw_colluding: 0.15,
            flow_honest: 0.50,
            flow_colluding: 0.85,
            min_hands: 50,
        }
    }
}

impl SprtParams {
    /// Wald upper (accept-collusion) bound: `ln((1 − β) / α)`.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_boss::detector::SprtParams;
    /// assert!(SprtParams::default().upper_bound() > 0.0);
    /// ```
    #[must_use]
    pub fn upper_bound(&self) -> f64 {
        ((1.0 - self.beta) / self.alpha).ln()
    }

    /// Wald lower (accept-honest) bound: `ln(β / (1 − α))`.
    #[must_use]
    pub fn lower_bound(&self) -> f64 {
        (self.beta / (1.0 - self.alpha)).ln()
    }
}

/// A per-pair verdict: accumulated evidence and, when it crossed the bound,
/// the hand index at which the pair was first flagged.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// The pair judged.
    pub pair: Pair,
    /// Accumulated log-likelihood ratio at session end.
    pub llr: f64,
    /// Hands in which both members were dealt.
    pub hands_observed: u32,
    /// Sample-size confidence band for `hands_observed`.
    pub confidence: Confidence,
    /// Hand number at which the pair first crossed the flag threshold, if ever.
    pub flagged_at_hand: Option<u32>,
}

const P_MIN: f64 = 0.01;
const P_MAX: f64 = 0.99;

/// One Bernoulli-observation log-likelihood contribution, clamped to keep the
/// logs finite.
fn bernoulli_llr(observed: bool, p_honest: f64, p_colluding: f64) -> f64 {
    let ph = p_honest.clamp(P_MIN, P_MAX);
    let pc = p_colluding.clamp(P_MIN, P_MAX);
    if observed {
        (pc / ph).ln()
    } else {
        ((1.0 - pc) / (1.0 - ph)).ln()
    }
}

/// Assesses every pair in the session, returning a [`Verdict`] each, sorted by
/// descending LLR (the most suspicious pair first).
///
/// # Examples
///
/// ```
/// use pkdealer_boss::detector::{assess, SprtParams};
/// assert!(assess(&[], &SprtParams::default()).is_empty());
/// ```
#[must_use]
pub fn assess(hands: &[RedactedHand], params: &SprtParams) -> Vec<Verdict> {
    let pairs = pairs_in(hands);
    let mut sorted: Vec<&RedactedHand> = hands.iter().collect();
    sorted.sort_by_key(|h| h.hand_no);
    let mut verdicts: Vec<Verdict> = pairs
        .iter()
        .map(|pair| assess_pair(&sorted, pair, params))
        .collect();
    verdicts.sort_by(|a, b| b.llr.total_cmp(&a.llr));
    verdicts
}

/// Running baseline aggression for one member: `(aggressive, total)`.
type Baseline = (u32, u32);

fn assess_pair(sorted: &[&RedactedHand], pair: &Pair, params: &SprtParams) -> Verdict {
    let mut llr = 0.0f64;
    let mut hands_observed = 0u32;
    let mut flagged_at_hand: Option<u32> = None;
    let mut base_a: Baseline = (0, 0);
    let mut base_b: Baseline = (0, 0);
    let (mut flow_atob, mut flow_btoa) = (0u32, 0u32);

    for hand in sorted {
        let obs = observe_hand(hand, pair);

        // 1. Update each member's running baseline aggression FIRST.
        for (member, aggressive) in &obs.baseline_actions {
            let base = if *member == pair.a {
                &mut base_a
            } else {
                &mut base_b
            };
            base.1 += 1;
            base.0 += u32::from(*aggressive);
        }

        // 2. Soft-play: heads-up aggression vs the member's own honest baseline.
        for (member, aggressive) in &obs.hu_actions {
            let base = if *member == pair.a { base_a } else { base_b };
            let p_h = if base.1 >= params.min_baseline_actions {
                f64::from(base.0) / f64::from(base.1)
            } else {
                params.default_honest_aggr
            };
            let p_c = p_h * params.soft_play_discount;
            llr += bernoulli_llr(*aggressive, p_h, p_c);
        }

        // 3. Whipsaw: one Bernoulli per hand both members were dealt.
        if obs.both_dealt {
            llr += bernoulli_llr(
                obs.whipsaw_events > 0,
                params.whipsaw_honest,
                params.whipsaw_colluding,
            );
        }

        // 4. Chip flow: does this pair-pot match the running majority direction?
        match obs.pair_pot {
            Some(PairPotOutcome::FlowAtoB(_)) => {
                if let Some(matches) = majority_match(flow_atob, flow_btoa) {
                    llr += bernoulli_llr(matches, params.flow_honest, params.flow_colluding);
                }
                flow_atob += 1;
            }
            Some(PairPotOutcome::FlowBtoA(_)) => {
                if let Some(matches) = majority_match(flow_btoa, flow_atob) {
                    llr += bernoulli_llr(matches, params.flow_honest, params.flow_colluding);
                }
                flow_btoa += 1;
            }
            Some(PairPotOutcome::Neutral) | None => {}
        }

        // 5. Advance the sample and flag once, floor-gated.
        if obs.both_dealt {
            hands_observed += 1;
        }
        if flagged_at_hand.is_none()
            && hands_observed >= params.min_hands
            && llr >= params.upper_bound()
        {
            flagged_at_hand = Some(hand.hand_no);
        }
    }

    Verdict {
        pair: *pair,
        llr,
        hands_observed,
        confidence: Confidence::from_sample_size(u64::from(hands_observed)),
        flagged_at_hand,
    }
}

/// Whether this direction's event matches the running majority of prior
/// events: `Some(true/false)` when a majority exists, `None` on a tie (the
/// first event, and any exact tie, contribute no evidence).
fn majority_match(this_dir: u32, other_dir: u32) -> Option<bool> {
    match this_dir.cmp(&other_dir) {
        Ordering::Greater => Some(true),
        Ordering::Less => Some(false),
        Ordering::Equal => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{self, GTO, MALLORY, TAG, TRUDY};
    use crate::redacted::redact;

    #[test]
    fn wald_bounds_from_targets() {
        let p = SprtParams::default();
        assert!((p.upper_bound() - (0.90_f64 / 0.01).ln()).abs() < 1e-9);
        assert!((p.lower_bound() - (0.10_f64 / 0.99).ln()).abs() < 1e-9);
    }

    #[test]
    fn sprt_flags_colluders() {
        let hands = redact(&fixtures::collection(fixtures::dump_corpus(120)));
        let verdicts = assess(&hands, &SprtParams::default());
        let guilty = verdicts
            .iter()
            .find(|v| v.pair == Pair::new(MALLORY, TRUDY))
            .unwrap();
        let at = guilty.flagged_at_hand.expect("colluding pair must flag");
        assert!(at >= 50, "confidence floor holds: {at}");
        assert!(at <= 120, "flag within the session: {at}");
    }

    #[test]
    fn sprt_flags_soft_play_and_whipsaw_too() {
        for corpus in [
            fixtures::soft_play_corpus(120),
            fixtures::whipsaw_corpus(120),
        ] {
            let hands = redact(&fixtures::collection(corpus));
            let verdicts = assess(&hands, &SprtParams::default());
            let guilty = verdicts
                .iter()
                .find(|v| v.pair == Pair::new(MALLORY, TRUDY))
                .unwrap();
            assert!(guilty.flagged_at_hand.is_some());
        }
    }

    #[test]
    fn sprt_honest_under_fp_bound() {
        let hands = redact(&fixtures::collection(fixtures::honest_corpus(160)));
        let verdicts = assess(&hands, &SprtParams::default());
        assert!(
            verdicts.iter().all(|v| v.flagged_at_hand.is_none()),
            "honest lineup must not flag: {verdicts:?}"
        );
    }

    #[test]
    fn suspicion_confidence_low_on_small_sample() {
        // 30 hands of blatant dumping — still below the Confidence floor.
        let hands = redact(&fixtures::collection(fixtures::dump_corpus(30)));
        let verdicts = assess(&hands, &SprtParams::default());
        let guilty = verdicts
            .iter()
            .find(|v| v.pair == Pair::new(MALLORY, TRUDY))
            .unwrap();
        assert!(guilty.flagged_at_hand.is_none());
        assert_eq!(guilty.confidence, Confidence::Low);
    }

    #[test]
    fn honest_pair_never_flags_in_guilty_session() {
        // Even in a dumping session, the honest control pair stays clean.
        let hands = redact(&fixtures::collection(fixtures::dump_corpus(120)));
        let verdicts = assess(&hands, &SprtParams::default());
        let honest = verdicts
            .iter()
            .find(|v| v.pair == Pair::new(GTO, TAG))
            .unwrap();
        assert!(honest.flagged_at_hand.is_none());
    }
}
