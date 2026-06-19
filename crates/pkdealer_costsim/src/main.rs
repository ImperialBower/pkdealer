//! `pkdealer_costsim` CLI (EPIC-44 Phase 0).
//!
//! Reads a recorded [`pkcore::hand_history::HandCollection`] (the EPIC-25 YAML
//! sink), sums each seat's LLM token usage, and prints a notional-cost
//! leaderboard. A pure consumer of recorded output — the live arena is never
//! touched. See [`pkdealer_costsim`] crate docs for the full model.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use pkdealer_costsim::app::{RunConfig, run};

/// Command-line arguments for the cost-simulation tool.
#[derive(Parser, Debug)]
#[command(
    name = "pkdealer_costsim",
    version,
    about = "Offline token-accounting and notional-cost analysis over recorded pkdealer sessions"
)]
struct Cli {
    /// Recorded `HandCollection` YAML file (the EPIC-25 session sink).
    hand_collection: PathBuf,

    /// `pricing.toml` with per-million-token rates. Without it, tokens are
    /// reported but costs are shown as unknown.
    #[arg(long, value_name = "FILE")]
    pricing: Option<PathBuf>,

    /// Price an actual model as a notional one: `--price-as gemma=claude-opus-4-8`.
    /// Repeatable. Overrides any `--scenario` mapping for that model.
    #[arg(long = "price-as", value_name = "MODEL=NOTIONAL")]
    price_as: Vec<String>,

    /// Named pricing scenario: `mixed` (default), `all-opus`, or `all-haiku`.
    #[arg(long, value_name = "NAME")]
    scenario: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = RunConfig {
        collection: cli.hand_collection,
        pricing: cli.pricing,
        price_as: cli.price_as,
        scenario: cli.scenario,
    };
    match run(&config) {
        Ok(table) => {
            print!("{table}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("pkdealer_costsim: {err}");
            ExitCode::FAILURE
        }
    }
}
