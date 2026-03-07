#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! # Poker Dealer Service
//!
//! A gRPC service that provides poker dealer functionality.
//!
//! This service handles poker game operations such as:
//! - Shuffling and dealing cards
//! - Managing game state
//! - Enforcing poker rules
//!
//! ## Usage
//!
//! Run the service:
//! ```bash
//! cargo run --bin pkdealer_service
//! ```

use std::process;

/// Main entry point for the poker dealer service.
///
/// Initializes the gRPC server and starts listening for client connections.
///
/// # Examples
///
/// ```bash
/// cargo run --bin pkdealer_service
/// ```
fn main() {
    // Initialize the service
    if let Err(e) = run() {
        eprintln!("Application error: {}", e);
        process::exit(1);
    }
}

/// Runs the poker dealer service.
///
/// Sets up the gRPC server and handles incoming requests.
///
/// # Errors
///
/// Returns an error if the service fails to start or encounters
/// a critical error during operation.
///
/// # Examples
///
/// ```no_run
/// # fn run() -> Result<(), Box<dyn std::error::Error>> {
/// // Service initialization and operation
/// # Ok(())
/// # }
/// ```
fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("Poker Dealer Service v{}", env!("CARGO_PKG_VERSION"));
    println!("Starting gRPC server...");

    // TODO: Initialize gRPC server
    // TODO: Register service handlers
    // TODO: Start listening on configured port

    println!("Service is ready to accept connections");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_initializes_successfully() {
        // Test that run() can be called without panicking
        // In a real implementation, this would test actual service initialization
        let result = run();
        assert!(result.is_ok());
    }
}
