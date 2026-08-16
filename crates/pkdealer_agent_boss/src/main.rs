#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! Binary entry point for the live Boss (EPIC-70 Phase 4). Parses CLI/env,
//! initializes OpenTelemetry, and runs the poll loop in [`pkdealer_agent_boss::app`].

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;

use pkdealer_agent_boss::app::{RunConfig, run};
use pkdealer_agent_boss::otel::init_otel;

/// Live blind collusion detector: polls `ExportSession`, redacts at ingest, and
/// emits per-pair SPRT verdicts over OpenTelemetry.
#[derive(Parser, Debug)]
#[command(name = "pkdealer_agent_boss", version, about)]
struct Cli {
    /// Dealer service endpoint.
    #[arg(
        long,
        env = "PKDEALER_ENDPOINT",
        default_value = "http://localhost:50051"
    )]
    endpoint: String,

    /// Spectator token presented on `ExportSession` (the service gates it).
    #[arg(long, env = "PKDEALER_SPECTATOR_TOKEN", default_value = "spectator")]
    spectator_token: String,

    /// Optional ground-truth labels sidecar (YAML); enables the false-positive
    /// counter. A blind boss with no labels leaves that counter at zero.
    #[arg(long, env = "PKDEALER_BOSS_LABELS")]
    labels: Option<PathBuf>,

    /// Seconds between `GetSessionInfo` polls.
    #[arg(long, default_value_t = 5)]
    interval_secs: u64,

    /// Poll once and exit (a smoke check) instead of looping.
    #[arg(long)]
    once: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let _otel = match init_otel() {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("pkdealer_agent_boss: OTel init failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let config = RunConfig {
        endpoint: cli.endpoint,
        spectator_token: cli.spectator_token,
        labels: cli.labels,
        interval: Duration::from_secs(cli.interval_secs),
        once: cli.once,
    };

    match run(&config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pkdealer_agent_boss: {e}");
            ExitCode::FAILURE
        }
    }
}
