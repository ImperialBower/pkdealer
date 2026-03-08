#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! # Poker Dealer Service
//!
//! A gRPC service that provides poker dealer functionality.

use std::{env, net::SocketAddr, process};

use pkdealer_proto::dealer::{
    PingReply, PingRequest,
    dealer_server::{Dealer, DealerServer},
};
use tonic::{Request, Response, Status, transport::Server};

const DEFAULT_SERVICE_ADDR: &str = "127.0.0.1:50051";

#[derive(Debug, Default)]
struct DealerService;

#[tonic::async_trait]
impl Dealer for DealerService {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
        let client_id = request.into_inner().client_id;
        let message = if client_id.is_empty() {
            "pong".to_owned()
        } else {
            format!("pong:{client_id}")
        };

        Ok(Response::new(PingReply { message }))
    }
}

/// Main entry point for the poker dealer service binary.
#[tokio::main]
async fn main() {
    if let Err(error) = run_from_env().await {
        eprintln!("Application error: {error}");
        process::exit(1);
    }
}

async fn run_from_env() -> Result<(), Box<dyn std::error::Error>> {
    let addr = env::var("PKDEALER_ADDR").unwrap_or_else(|_| DEFAULT_SERVICE_ADDR.to_owned());
    run(&addr).await
}

async fn run(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    let socket_addr: SocketAddr = addr.parse()?;

    println!("Poker Dealer Service v{}", env!("CARGO_PKG_VERSION"));
    println!("Starting gRPC server on {socket_addr}...");

    Server::builder()
        .add_service(DealerServer::new(DealerService))
        .serve(socket_addr)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dealer_service_ping_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = DealerService;
        let request = Request::new(PingRequest {
            client_id: "client-99".to_owned(),
        });

        let response = service.ping(request).await?;

        assert_eq!(response.into_inner().message, "pong:client-99");
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_ping_empty_client_id() -> Result<(), Box<dyn std::error::Error>> {
        let service = DealerService;
        let request = Request::new(PingRequest {
            client_id: String::new(),
        });

        let response = service.ping(request).await?;

        assert_eq!(response.into_inner().message, "pong");
        Ok(())
    }
}
