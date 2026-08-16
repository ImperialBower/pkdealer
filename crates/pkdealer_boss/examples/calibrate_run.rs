//! Label a recorded session, grade the blind Boss against it, and fit the honest
//! null — the EPIC-70 Phase 5 workflow as a runnable driver until a proper CLI
//! lands (labels + calibration have no first-class binary yet).
//!
//! ```text
//! cargo run -p pkdealer_boss --example calibrate_run -- <session.yaml> [a_name b_name]
//! ```
//!
//! The colluding pair defaults to `carol_1` / `dave_1` (the `make detect`
//! lineup). Detection is blind — labels only *grade* it after the fact.

use std::process::ExitCode;

use pkcore::hand_history::HandCollection;
use pkdealer_boss::calibrate::{fit_null, pooled_bb_per_100};
use pkdealer_boss::detector::{SprtParams, assess};
use pkdealer_boss::labels::{GroundTruthLabels, LabelStyle, LabelVector};
use pkdealer_boss::redacted::redact;
use pkdealer_boss::scorer::score;
use pkdealer_boss::signals::Pair;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let session = args
        .next()
        .ok_or("usage: calibrate_run <session.yaml> [a_name b_name]")?;
    let a_name = args.next().unwrap_or_else(|| "carol_1".to_owned());
    let b_name = args.next().unwrap_or_else(|| "dave_1".to_owned());

    // Load + redact at ingest (the collection is still needed for label
    // resolution + the card-aware oracle, both grading-tier reads).
    let raw = std::fs::read_to_string(&session)?;
    let collection: HandCollection = if raw.trim_start().starts_with('{') {
        serde_json::from_str(&raw)?
    } else {
        HandCollection::from_yaml(&raw)?
    };
    let hands = redact(&collection);
    println!("session: {session}");
    println!("hands: {}   redacted-pairs: {}", hands.len(), {
        use pkdealer_boss::signals::pairs_in;
        pairs_in(&hands).len()
    });

    // Resolve the named colluding pair to stable UUIDs from the recorded hands.
    let labels = GroundTruthLabels::resolve(
        &collection,
        &[(
            a_name.clone(),
            b_name.clone(),
            LabelVector::Peer,
            LabelStyle::ChipDump,
        )],
    )?;
    let lp = labels
        .colluding_pairs
        .first()
        .ok_or("colluding pair did not resolve — check the names exist in the session")?;
    let pair = Pair::new(lp.a, lp.b);

    // Detect (blind) + grade (against labels).
    let params = SprtParams::default();
    let verdicts = assess(&hands, &params);
    let report = score(&collection, &labels, &verdicts);
    let graded = report
        .labeled
        .first()
        .ok_or("no labeled pair in the grading report")?;

    println!("\n== detection (blind Boss) ==");
    println!("colluding pair: {} + {}", lp.a_name, lp.b_name);
    match graded.hands_to_detection {
        Some(h) => println!("result: FLAGGED at hand {h}"),
        None => println!("result: NOT flagged (miss)"),
    }
    if let Some(v) = verdicts.iter().find(|v| v.pair == pair) {
        println!(
            "final LLR: {:.2}   (flag threshold {:.2}; positive ⇒ suspicious)",
            v.llr,
            params.upper_bound()
        );
        println!(
            "hands together: {}   confidence: {:?}",
            v.hands_observed, v.confidence
        );
    }
    println!(
        "false positives (honest pairs flagged): {}   fp_rate: {:.3}",
        report.false_positives.len(),
        report.fp_rate
    );
    println!(
        "card-aware oracle: {} spots, {} sacrifices",
        graded.oracle.spots, graded.oracle.sacrifices
    );

    println!("\n== did it pay? (single run, no control) ==");
    println!(
        "pair pooled bb/100: {:.2}",
        pooled_bb_per_100(&hands, &pair)
    );

    // Fit the null from THIS run. Caveat printed below: a real null needs
    // honest-only control runs; this pools every pair in one mixed run.
    let null = fit_null(&[hands]);
    println!("\n== fitted null (this run as one sample) ==");
    println!(
        "net_flow      mean {:>10.1}  std {:>10.1}  n {}",
        null.net_flow.mean, null.net_flow.std, null.net_flow.n
    );
    println!(
        "soft_play_idx mean {:>10.3}  std {:>10.3}  n {}",
        null.soft_play_index.mean, null.soft_play_index.std, null.soft_play_index.n
    );
    println!(
        "whipsaw_count mean {:>10.2}  std {:>10.2}  n {}",
        null.whipsaw_count.mean, null.whipsaw_count.std, null.whipsaw_count.n
    );
    println!(
        "\nNote: a real calibration null needs HONEST-only control runs; this pools\n\
         ALL pairs in one mixed run, so treat these as a harness smoke test, not a\n\
         calibrated model."
    );

    Ok(())
}
