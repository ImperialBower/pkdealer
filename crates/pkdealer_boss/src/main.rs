#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! `pkdealer_boss` CLI (EPIC-70 Phase 2d): read a recorded session (+ optional
//! ground-truth labels), run the blind detection pipeline, print the per-pair
//! report. A pure consumer of recorded output — it needs no service and no
//! spectator token.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use pkdealer_boss::app::{RunConfig, run};

/// Command-line arguments for the Boss.
#[derive(Parser, Debug)]
#[command(
    name = "pkdealer_boss",
    version,
    about = "Blind collusion detection over recorded pkdealer sessions"
)]
struct Cli {
    /// Recorded `HandCollection` session file (YAML from the EPIC-25 sink, or
    /// JSON from `ExportSession`).
    #[arg(long, value_name = "FILE")]
    session: PathBuf,

    /// Ground-truth labels sidecar (YAML). Adds the scorer section:
    /// hands-to-detection, false-positive rate, and the EV-sacrifice oracle.
    #[arg(long, value_name = "FILE")]
    labels: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = RunConfig {
        session: cli.session,
        labels: cli.labels,
    };
    match run(&config) {
        Ok(report) => {
            print!("{report}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("pkdealer_boss: {err}");
            ExitCode::FAILURE
        }
    }
}
