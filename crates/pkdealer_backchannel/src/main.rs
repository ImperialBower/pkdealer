#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! `pkdealer_backchannel` broker binary (EPIC-70 Phase 3). Binds
//! `PKDEALER_BACKCHANNEL_BIND` (default `0.0.0.0:9099`) and relays `CardShare`
//! lines between colluding agents. Never contacts the dealer service.

use std::process::ExitCode;

use pkdealer_backchannel::Broker;

#[tokio::main]
async fn main() -> ExitCode {
    let bind =
        std::env::var("PKDEALER_BACKCHANNEL_BIND").unwrap_or_else(|_| "0.0.0.0:9099".to_string());
    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("pkdealer_backchannel: bind {bind} failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("pkdealer_backchannel: relaying on {bind}");
    if let Err(e) = Broker::new().serve(listener).await {
        eprintln!("pkdealer_backchannel: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
