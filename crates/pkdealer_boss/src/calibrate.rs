//! EPIC-70 Phase 5 — the calibration & validation **harness**.
//!
//! Given K recorded runs, this module fits the honest null distribution per
//! pairwise signal ([`fit_null`]), reports a false-positive rate with a
//! confidence interval ([`fp_rate_with_ci`]), and measures whether the cheat
//! actually paid ([`win_rate_lift`]).
//!
//! **Harness, not results.** Every function computes numbers *only* from the runs
//! it is given. No seeded or live corpus exists yet (EPIC-41 is unstarted), so
//! the EPIC ships this machinery with fixture-driven tests and **does not**
//! publish fabricated calibration figures. The write-up
//! (`docs/notes/EPIC-70_calibration.md`) leaves every result table as an
//! explicit `pending live run` placeholder until real K-run data exists.

use crate::detector::Verdict;
use crate::labels::GroundTruthLabels;
use crate::redacted::RedactedHand;
use crate::signals::{Pair, aggregate, pairs_in};

/// 95% two-sided normal quantile, for the Wilson interval.
const Z_95: f64 = 1.959_963_984_540_054;

/// Mean and population standard deviation of one signal over a sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignalNull {
    /// Sample mean (0 when the sample is empty).
    pub mean: f64,
    /// Population standard deviation (0 when the sample has fewer than 2 points).
    pub std: f64,
    /// Number of observations.
    pub n: usize,
}

/// The fitted honest null distribution per pairwise signal — the reference the
/// SPRT's honest hypothesis is calibrated against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NullModel {
    /// Directional chip flow between the pair (`net_flow_a_to_b`).
    pub net_flow: SignalNull,
    /// Heads-up / baseline aggression ratio (`soft_play_index`).
    pub soft_play_index: SignalNull,
    /// Whipsaw pattern count per pair.
    pub whipsaw_count: SignalNull,
}

/// Summarizes a sample into a [`SignalNull`] (mean + population std).
#[must_use]
fn summarize(xs: &[f64]) -> SignalNull {
    let n = xs.len();
    if n == 0 {
        return SignalNull {
            mean: 0.0,
            std: 0.0,
            n: 0,
        };
    }
    #[allow(clippy::cast_precision_loss)]
    let count = n as f64;
    let mean = xs.iter().sum::<f64>() / count;
    let variance = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / count;
    SignalNull {
        mean,
        std: variance.sqrt(),
        n,
    }
}

/// Fits the honest null over every pair in every honest control run.
///
/// Each entry of `honest_runs` is one session's redacted hands. The per-signal
/// samples pool every pair across every run; signals that are `None` for a pair
/// (empty bucket) are simply omitted from that signal's sample.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::calibrate::fit_null;
/// // No runs → an empty null model (every signal has n == 0).
/// let model = fit_null(&[]);
/// assert_eq!(model.net_flow.n, 0);
/// ```
#[must_use]
pub fn fit_null(honest_runs: &[Vec<RedactedHand>]) -> NullModel {
    let mut flow = Vec::new();
    let mut soft = Vec::new();
    let mut whip = Vec::new();
    for hands in honest_runs {
        for pair in pairs_in(hands) {
            let s = aggregate(hands, &pair);
            flow.push(s.net_flow_a_to_b);
            if let Some(v) = s.soft_play_index {
                soft.push(v);
            }
            whip.push(f64::from(s.whipsaw_count));
        }
    }
    NullModel {
        net_flow: summarize(&flow),
        soft_play_index: summarize(&soft),
        whipsaw_count: summarize(&whip),
    }
}

/// One run's detector output paired with its ground truth, for the FP study.
#[derive(Clone, Debug, PartialEq)]
pub struct FpRun {
    /// The per-pair verdicts the Boss produced for this run.
    pub verdicts: Vec<Verdict>,
    /// Ground-truth labels for this run (which pairs actually colluded).
    pub labels: GroundTruthLabels,
}

/// A false-positive rate with a 95% Wilson score interval.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FpStudy {
    /// Honest pairs the Boss wrongly flagged, summed across runs.
    pub flagged_honest: u32,
    /// Total honest pairs observed across runs.
    pub honest_pairs: u32,
    /// Point estimate `flagged_honest / honest_pairs` (0 when none observed).
    pub rate: f64,
    /// Lower bound of the 95% Wilson interval.
    pub lo: f64,
    /// Upper bound of the 95% Wilson interval.
    pub hi: f64,
}

/// Computes the pooled false-positive rate over K runs with a 95% Wilson score
/// interval — the honest way to report "≈ 0" over a finite sample, rather than
/// asserting an absolute zero (exit criterion 4).
///
/// A pair counts as honest when its run's labels do not mark it colluding; it is
/// a false positive when it was nonetheless flagged (`flagged_at_hand.is_some()`).
///
/// # Examples
///
/// ```
/// use pkdealer_boss::calibrate::{fp_rate_with_ci, FpStudy};
/// // No runs → a degenerate study at zero.
/// let study = fp_rate_with_ci(&[]);
/// assert_eq!(study.honest_pairs, 0);
/// assert_eq!(study.rate, 0.0);
/// ```
#[must_use]
pub fn fp_rate_with_ci(runs: &[FpRun]) -> FpStudy {
    let mut flagged_honest = 0u32;
    let mut honest_pairs = 0u32;
    for run in runs {
        for verdict in &run.verdicts {
            if run.labels.is_colluding(verdict.pair.a, verdict.pair.b) {
                continue;
            }
            honest_pairs += 1;
            if verdict.flagged_at_hand.is_some() {
                flagged_honest += 1;
            }
        }
    }
    let (rate, lo, hi) = wilson(flagged_honest, honest_pairs);
    FpStudy {
        flagged_honest,
        honest_pairs,
        rate,
        lo,
        hi,
    }
}

/// The Wilson score interval for `successes / trials` at 95%, returning
/// `(point_estimate, lo, hi)`. Degenerate `(0, 0, 0)` when `trials == 0`.
#[must_use]
fn wilson(successes: u32, trials: u32) -> (f64, f64, f64) {
    if trials == 0 {
        return (0.0, 0.0, 0.0);
    }
    let n = f64::from(trials);
    let p = f64::from(successes) / n;
    let z2 = Z_95 * Z_95;
    let denom = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denom;
    let margin = (Z_95 / denom) * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt();
    (p, (center - margin).max(0.0), (center + margin).min(1.0))
}

/// Pooled big-blinds-per-100-hands for a pair over a redacted session.
///
/// Sums both members' per-hand `net` (in big blinds) across every hand either
/// member was seated, normalized to a per-100-hands rate. Public information
/// only — `net` and `big_blind` are visible to any observer.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::calibrate::pooled_bb_per_100;
/// use pkdealer_boss::signals::Pair;
/// use uuid::Uuid;
/// // No hands → zero.
/// let pair = Pair::new(Uuid::from_u128(1), Uuid::from_u128(2));
/// assert_eq!(pooled_bb_per_100(&[], &pair), 0.0);
/// ```
#[must_use]
pub fn pooled_bb_per_100(hands: &[RedactedHand], pair: &Pair) -> f64 {
    let mut total_bb = 0.0;
    let mut counted = 0u32;
    for hand in hands {
        if hand.big_blind <= 0.0 {
            continue;
        }
        let seated: Vec<&crate::redacted::RedactedSeat> = hand
            .seats
            .iter()
            .filter(|s| pair.contains(s.player_id))
            .collect();
        if seated.is_empty() {
            continue;
        }
        let net: f64 = seated.iter().map(|s| s.net).sum();
        total_bb += net / hand.big_blind;
        counted += 1;
    }
    if counted == 0 {
        return 0.0;
    }
    100.0 * total_bb / f64::from(counted)
}

/// Win-rate **lift**: pooled bb/100 for the pair under collusion minus the same
/// under a collusion-off control run. Positive ⇒ the cheat paid (exit
/// criterion 1's per-run quantity, before replication across K runs).
///
/// # Examples
///
/// ```
/// use pkdealer_boss::calibrate::win_rate_lift;
/// use pkdealer_boss::signals::Pair;
/// use uuid::Uuid;
/// // Empty collusion and control → zero lift.
/// let pair = Pair::new(Uuid::from_u128(1), Uuid::from_u128(2));
/// assert_eq!(win_rate_lift(&[], &[], &pair), 0.0);
/// ```
#[must_use]
pub fn win_rate_lift(collusion: &[RedactedHand], control: &[RedactedHand], pair: &Pair) -> f64 {
    pooled_bb_per_100(collusion, pair) - pooled_bb_per_100(control, pair)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::redacted::redact;

    #[test]
    fn summarize_matches_hand_computation() {
        let s = summarize(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]);
        assert_eq!(s.n, 8);
        assert!((s.mean - 5.0).abs() < 1e-9);
        assert!((s.std - 2.0).abs() < 1e-9); // population std of this classic set
    }

    #[test]
    fn summarize_empty_is_zero() {
        let s = summarize(&[]);
        assert_eq!(s, SignalNull { mean: 0.0, std: 0.0, n: 0 });
    }

    #[test]
    fn fit_null_pools_pairs_across_runs() {
        // Two honest runs; every pair contributes to each signal's sample.
        let run1 = redact(&fixtures::collection(fixtures::honest_corpus(30)));
        let run2 = redact(&fixtures::collection(fixtures::honest_corpus(30)));
        let model = fit_null(&[run1, run2]);
        assert!(model.net_flow.n >= 2, "at least one pair per run pooled");
        assert!(model.whipsaw_count.n >= 2);
        assert!(model.net_flow.std >= 0.0);
    }

    #[test]
    fn wilson_zero_successes_has_zero_lower_bound() {
        // 0 / 100 → rate 0, lower bound 0, upper bound small but positive.
        let (rate, lo, hi) = wilson(0, 100);
        assert!(rate.abs() < f64::EPSILON, "0/100 is a zero rate");
        assert!(lo.abs() < 1e-12, "lower bound is (floating-point) zero, got {lo}");
        assert!(hi > 0.0 && hi < 0.05, "one-sided Wilson upper bound is small");
    }

    #[test]
    fn wilson_interval_brackets_the_point_estimate() {
        let (rate, lo, hi) = wilson(5, 100);
        assert!((rate - 0.05).abs() < 1e-9);
        assert!(lo < rate && rate < hi);
        assert!(lo >= 0.0 && hi <= 1.0);
    }

    #[test]
    fn fp_study_over_honest_runs_counts_pairs() {
        // Honest corpus, default detector: build verdicts and empty labels
        // (no one colludes), so every flagged pair is a false positive.
        use crate::detector::{SprtParams, assess};
        let hands = redact(&fixtures::collection(fixtures::honest_corpus(60)));
        let verdicts = assess(&hands, &SprtParams::default());
        let run = FpRun {
            verdicts,
            labels: GroundTruthLabels { colluding_pairs: vec![] },
        };
        let study = fp_rate_with_ci(&[run]);
        assert!(study.honest_pairs >= 1);
        assert!(study.rate >= 0.0 && study.rate <= 1.0);
        assert!(study.lo <= study.rate && study.rate <= study.hi);
    }

    #[test]
    fn win_rate_lift_positive_when_dumpers_out_earn_control() {
        // The dump corpus concentrates chips in the pair; an honest control of
        // the same size should not. The pooled bb/100 lift must be finite, and
        // the collusion run's pooled result is well-defined.
        let colluding = redact(&fixtures::collection(fixtures::dump_corpus(120)));
        let control = redact(&fixtures::collection(fixtures::honest_corpus(120)));
        let pair = Pair::new(fixtures::MALLORY, fixtures::TRUDY);
        let lift = win_rate_lift(&colluding, &control, &pair);
        assert!(lift.is_finite());
        // Sanity: the pair's collusion-run pooled bb/100 is itself finite and
        // the lift equals collusion − control exactly.
        let c = pooled_bb_per_100(&colluding, &pair);
        let k = pooled_bb_per_100(&control, &pair);
        assert!((lift - (c - k)).abs() < 1e-9);
    }
}
