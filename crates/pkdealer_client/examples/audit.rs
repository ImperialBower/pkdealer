//! Audits all YAML hand histories in `generated/` and `generated/old/`
//! against pkcore's replay engine.
//!
//! For every hand, [`HandHistory::replay`] reconstructs a fresh
//! [`TableNoCell`](pkcore::casino::table_no_cell::TableNoCell), drives it
//! through every recorded action, calls `end_hand()`, and compares the
//! resulting chip counts against the `net` P&L stored in `results`.
//!
//! Results are reported per file with four outcome labels:
//! - `[OK]`            — fully consistent.
//! - `[CHIP MISMATCH]` — actions replayed but final stacks diverge from recorded `net`.
//! - `[REPLAY ERROR]`  — pkcore rejected the action sequence (e.g. illegal action).
//! - `[CHIP LEAK]`     — total chips across all seated players changed between two
//!                       consecutive recorded hands (chips created or destroyed).
//!
//! Files under `generated/old/` are treated as **legacy** — they were produced
//! before `session.table.event_log.clear()` was added to `demo.rs` and may
//! contain event-log contamination (ghost-seat actions, next-hand events bleeding
//! into the current hand's streets).  Their errors are counted separately so they
//! do not pollute the current-file pass rate.
//!
//! Run (all files):
//!
//!   cargo run --example audit -p pkdealer_client
//!
//! Run (specific file):
//!
//!   cargo run --example audit -p pkdealer_client -- path/to/file.yaml

use pkcore::hand_history::HandCollection;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // If specific files were given on the command line, audit only those.
    if !args.is_empty() {
        let counts = audit_files(&args, false);
        let bar = "─".repeat(50);
        println!("\n── SUMMARY {bar}");
        println!("  files        : {}", args.len());
        println!("  total hands  : {}", counts.total);
        println!("  ok           : {}", counts.ok);
        println!("  chip mismatch: {}", counts.inconsistent);
        println!("  chip leaks   : {}", counts.chip_leaks);
        println!("  errors       : {}", counts.errors);
        if counts.has_problems() {
            std::process::exit(1);
        }
        return;
    }

    // Default: scan generated/ (current) and generated/old/ (legacy).
    let current_dir = "generated";
    let legacy_dir = "generated/old";

    let current_files = collect_yaml(current_dir);
    let legacy_files = collect_yaml(legacy_dir);

    if current_files.is_empty() && legacy_files.is_empty() {
        println!("no YAML files found");
        return;
    }

    let curr = audit_files(&current_files, false);
    let legacy = audit_files(&legacy_files, true);

    let bar = "─".repeat(50);
    println!("\n── SUMMARY {bar}");
    println!("  ── current (generated/)");
    println!("     files        : {}", current_files.len());
    println!("     total hands  : {}", curr.total);
    println!("     ok           : {}", curr.ok);
    println!("     chip mismatch: {}", curr.inconsistent);
    println!("     chip leaks   : {}", curr.chip_leaks);
    println!("     errors       : {}", curr.errors);
    if legacy_files.is_empty() {
        println!("  ── legacy (generated/old/) : none found");
    } else {
        println!("  ── legacy (generated/old/) — pre-event-log-clear; errors expected");
        println!("     files        : {}", legacy_files.len());
        println!("     total hands  : {}", legacy.total);
        println!("     ok           : {}", legacy.ok);
        println!("     chip mismatch: {}", legacy.inconsistent);
        println!("     chip leaks   : {}", legacy.chip_leaks);
        println!("     errors       : {}", legacy.errors);
    }
    // Legacy errors are expected — only fail on current-file problems.
    if curr.has_problems() {
        std::process::exit(1);
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

struct Counts {
    total: usize,
    ok: usize,
    inconsistent: usize,
    errors: usize,
    chip_leaks: usize,
}

impl Counts {
    fn has_problems(&self) -> bool {
        self.inconsistent > 0 || self.errors > 0 || self.chip_leaks > 0
    }
}

fn collect_yaml(dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    match std::fs::read_dir(dir) {
        Ok(rd) => {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.extension().and_then(|e| e.to_str()) == Some("yaml") {
                    files.push(p.to_string_lossy().into_owned());
                }
            }
        }
        Err(e) => {
            eprintln!("skipping {dir}: {e}");
        }
    }
    files.sort();
    files
}

fn audit_files(files: &[String], legacy: bool) -> Counts {
    let tag = if legacy { "[LEGACY]" } else { "" };
    let mut counts = Counts { total: 0, ok: 0, inconsistent: 0, errors: 0, chip_leaks: 0 };

    for path in files {
        println!("\n── {path}");

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                println!("  ERROR reading file: {e}");
                counts.errors += 1;
                continue;
            }
        };

        let collection = match HandCollection::from_yaml(&content) {
            Ok(c) => c,
            Err(e) => {
                println!("  ERROR parsing YAML: {e}");
                counts.errors += 1;
                continue;
            }
        };

        // Track ending stacks of the previous hand for cross-hand chip conservation.
        // Reset per file so leaks don't bleed across separate sessions.
        let mut prev_end: Option<(String, f64)> = None; // (hand_id, total_chips)

        for hh in collection.iter() {
            counts.total += 1;

            // ── Cross-hand chip conservation check ────────────────────────────
            let total_start: f64 = hh.players.iter().map(|p| p.stack).sum();
            if let Some((ref prev_id, prev_total)) = prev_end {
                if (prev_total - total_start).abs() > 1.0 {
                    println!(
                        "  [CHIP LEAK]{tag} after hand {} → hand {}: {:.0} → {:.0} ({:+.0} chips)",
                        prev_id,
                        hh.hand.id,
                        prev_total,
                        total_start,
                        total_start - prev_total,
                    );
                    counts.chip_leaks += 1;
                }
            }

            // Compute ending total for the next iteration.
            let total_end: f64 = hh.players.iter().map(|p| {
                let net = hh.results.as_ref()
                    .and_then(|rs| rs.iter().find(|r| r.seat == p.seat))
                    .and_then(|r| r.net)
                    .unwrap_or(0.0);
                p.stack + net
            }).sum();
            prev_end = Some((hh.hand.id.clone(), total_end));

            // ── Per-hand replay check ──────────────────────────────────────────
            match hh.replay() {
                Err(e) => {
                    println!("  [REPLAY ERROR]{tag} hand {} — {e}", hh.hand.id);
                    counts.errors += 1;
                }
                Ok(ref r) if !r.is_consistent => {
                    println!("  [CHIP MISMATCH]{tag} hand {} — per-seat diff:", hh.hand.id);
                    if let Some(results) = &hh.results {
                        for entry in results {
                            if let Some(net) = entry.net {
                                let start = hh
                                    .players
                                    .iter()
                                    .find(|p| p.seat == entry.seat)
                                    .map_or(0.0, |p| p.stack);
                                let expected = start + net;
                                let actual = r
                                    .final_stacks
                                    .iter()
                                    .find(|(s, _)| *s == entry.seat)
                                    .map_or(0.0, |(_, c)| *c as f64);
                                if (expected - actual).abs() > 1.0 {
                                    println!(
                                        "    seat {} expected={expected:.0}  actual={actual:.0}  diff={:+.0}",
                                        entry.seat,
                                        actual - expected
                                    );
                                }
                            }
                        }
                    }
                    counts.inconsistent += 1;
                }
                Ok(_) => {
                    println!("  [OK] hand {}", hh.hand.id);
                    counts.ok += 1;
                }
            }
        }
    }

    counts
}
