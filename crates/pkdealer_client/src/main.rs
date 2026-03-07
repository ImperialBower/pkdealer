#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! # Poker Dealer Client
//!
//! A gRPC client for interacting with the poker dealer service.
//!
//! This client provides a command-line interface for:
//! - Connecting to the poker dealer service
//! - Requesting card deals
//! - Managing game sessions
//! - Querying game state
//!
//! ## Usage
//!
//! Run the client:
//! ```bash
//! cargo run --bin pkdealer_client
//! ```

use std::process;

/// Main entry point for the poker dealer client.
///
/// Parses command-line arguments and executes the requested operation
/// against the poker dealer service.
///
/// # Examples
///
/// ```bash
/// cargo run --bin pkdealer_client
/// ```
fn main() {
    // Initialize and run the client
    if let Err(e) = run() {
        eprintln!("Application error: {e}");
        process::exit(1);
    }
}

/// Runs the poker dealer client.
///
/// Establishes connection to the gRPC service and handles user commands.
///
/// # Errors
///
/// Returns an error if:
/// - Unable to connect to the service
/// - Service request fails
/// - Invalid command-line arguments
///
/// # Examples
///
/// ```no_run
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// // Client initialization and operation
/// # Ok(())
/// # }
/// ```
#[allow(clippy::unnecessary_wraps)]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("Poker Dealer Client v{}", env!("CARGO_PKG_VERSION"));
    println!("Connecting to service...");

    // TODO: Parse command-line arguments
    // TODO: Establish gRPC connection to service
    // TODO: Execute requested operation
    // TODO: Display results

    println!("Client initialized successfully");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_initializes_successfully() {
        // Test that run() can be called without panicking
        // In a real implementation, this would test actual client initialization
        let result = run();
        assert!(result.is_ok());
    }
}
