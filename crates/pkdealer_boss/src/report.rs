//! Plain-text rendering of a Boss run — redacted tier only (it reads verdicts,
//! signals, and the optional grading report, never hole cards).

use std::collections::HashMap;
use std::fmt::Write as _;

use uuid::Uuid;

use crate::detector::{SprtParams, Verdict};
use crate::scorer::ScoreReport;
use crate::signals::{Pair, PairSignals};

fn pair_label(pair: Pair, names: &HashMap<Uuid, String>) -> String {
    let a = names.get(&pair.a).map_or("?", String::as_str);
    let b = names.get(&pair.b).map_or("?", String::as_str);
    format!("{a} + {b}")
}

fn opt2(value: Option<f64>) -> String {
    value.map_or_else(|| "—".to_string(), |x| format!("{x:.2}"))
}

/// Renders the per-pair report: a header, one row per pair (sorted by the
/// verdict order), the SPRT parameter line, and — when labels were supplied —
/// the ground-truth grading section.
///
/// # Examples
///
/// ```
/// use pkdealer_boss::detector::SprtParams;
/// use pkdealer_boss::report::render;
/// use std::collections::HashMap;
///
/// let text = render(&[], &[], &HashMap::new(), None, &SprtParams::default());
/// assert!(text.contains("pkdealer_boss"));
/// ```
// `names` is always the default-hasher map from `names_from`; a generic hasher
// buys nothing here.
#[allow(clippy::implicit_hasher)]
#[must_use]
pub fn render(
    verdicts: &[Verdict],
    signals: &[PairSignals],
    names: &HashMap<Uuid, String>,
    score: Option<&ScoreReport>,
    params: &SprtParams,
) -> String {
    let by_pair: HashMap<Pair, &PairSignals> = signals.iter().map(|s| (s.pair, s)).collect();
    let hands = verdicts.iter().map(|v| v.hands_observed).max().unwrap_or(0);

    let mut out = String::new();
    let _ = writeln!(out, "pkdealer_boss — blind collusion report (EPIC-70)");
    let _ = writeln!(
        out,
        "hands: {hands}   players: {}   pairs: {}",
        names.len(),
        verdicts.len()
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<28}  {:>5}  {:>8}  {:>7}  {:>9}  {:>10}  {:>7}  flagged@",
        "pair", "hands", "soft-idx", "whipsaw", "pair-pots", "net-flow", "llr"
    );
    for verdict in verdicts {
        let label = pair_label(verdict.pair, names);
        let signal = by_pair.get(&verdict.pair);
        let soft = opt2(signal.and_then(|s| s.soft_play_index));
        let whipsaw = signal.map_or(0, |s| s.whipsaw_count);
        let pair_pots = signal.map_or(0, |s| s.pair_pots);
        let net_flow = signal.map_or(0.0, |s| s.net_flow_a_to_b);
        let flagged = verdict
            .flagged_at_hand
            .map_or_else(|| "—".to_string(), |h| h.to_string());
        let _ = writeln!(
            out,
            "{label:<28}  {:>5}  {soft:>8}  {whipsaw:>7}  {pair_pots:>9}  {net_flow:>10.1}  {:>7.2}  {flagged}",
            verdict.hands_observed, verdict.llr
        );
    }
    let _ = writeln!(
        out,
        "SPRT: alpha={:.3} beta={:.3} upper={:.2} lower={:.2} confidence-floor={} hands (pre-calibration defaults)",
        params.alpha,
        params.beta,
        params.upper_bound(),
        params.lower_bound(),
        params.min_hands
    );

    if let Some(report) = score {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "ground truth: {} labeled pair(s)",
            report.labeled.len()
        );
        for pair_score in &report.labeled {
            let detection = pair_score.hands_to_detection.map_or_else(
                || "MISSED".to_string(),
                |h| format!("DETECTED  hands-to-detection={h}"),
            );
            let _ = writeln!(
                out,
                "  {} + {}  {detection}  oracle: {} spots / {} sacrifices",
                pair_score.names.0,
                pair_score.names.1,
                pair_score.oracle.spots,
                pair_score.oracle.sacrifices
            );
        }
        let _ = writeln!(
            out,
            "false positives: {} / {} honest pairs (rate {:.2})",
            report.false_positives.len(),
            report.honest_pairs,
            report.fp_rate
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detector::{SprtParams, assess};
    use crate::fixtures;
    use crate::labels::{GroundTruthLabels, LabelStyle, LabelVector};
    use crate::redacted::redact;
    use crate::scorer::score;
    use crate::signals::{aggregate, names_from, pairs_in};

    fn dump_render(labels: Option<&GroundTruthLabels>) -> String {
        let c = fixtures::collection(fixtures::dump_corpus(120));
        let hands = redact(&c);
        let params = SprtParams::default();
        let signals: Vec<_> = pairs_in(&hands)
            .iter()
            .map(|p| aggregate(&hands, p))
            .collect();
        let verdicts = assess(&hands, &params);
        let names = names_from(&hands);
        let report = labels.map(|l| score(&c, l, &verdicts));
        render(&verdicts, &signals, &names, report.as_ref(), &params)
    }

    #[test]
    fn render_includes_header_and_pair_rows() {
        let text = dump_render(None);
        assert!(text.contains("pkdealer_boss — blind collusion report"));
        assert!(text.contains("mallory_1 + trudy_1"));
        assert!(text.contains("SPRT:"));
    }

    #[test]
    fn render_marks_unflagged_pairs_with_dash() {
        let c = fixtures::collection(fixtures::honest_corpus(96));
        let hands = redact(&c);
        let params = SprtParams::default();
        let signals: Vec<_> = pairs_in(&hands)
            .iter()
            .map(|p| aggregate(&hands, p))
            .collect();
        let verdicts = assess(&hands, &params);
        let names = names_from(&hands);
        let text = render(&verdicts, &signals, &names, None, &params);
        assert!(text.contains("—"));
        assert!(!text.contains("DETECTED"));
    }

    #[test]
    fn render_shows_ground_truth_when_labeled() {
        let c = fixtures::collection(fixtures::dump_corpus(120));
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
        let text = dump_render(Some(&labels));
        assert!(text.contains("ground truth:"));
        assert!(text.contains("DETECTED"));
        assert!(text.contains("false positives: 0"));
    }
}
