#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! # Poker Dealer Client
//!
//! A gRPC client for interacting with the poker dealer service.

use std::process;

use pkdealer_proto::{dealer::dealer_client::DealerClient, new_ping_request};

const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:50051";
const DEFAULT_CLIENT_ID: &str = "pkdealer-client";

/// Main entry point for the poker dealer client binary.
#[tokio::main]
async fn main() {
    if let Err(error) = run_from_env().await {
        eprintln!("Application error: {error}");
        process::exit(1);
    }
}

async fn run_from_env() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = std::env::var("PKDEALER_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
    let client_id = std::env::var("PKDEALER_CLIENT_ID").unwrap_or_else(|_| DEFAULT_CLIENT_ID.to_owned());

    println!("Poker Dealer Client v{}", env!("CARGO_PKG_VERSION"));
    println!("Connecting to service at {endpoint}...");

    let message = ping(&endpoint, &client_id).await?;
    println!("Service response: {message}");

    Ok(())
}

async fn ping(endpoint: &str, client_id: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut client = DealerClient::connect(endpoint.to_owned()).await?;
    let request = tonic::Request::new(new_ping_request(client_id));
    let response = client.ping(request).await?;

    Ok(response.into_inner().message)
}

#[cfg(test)]
mod tests {
    use super::*;

    use pkdealer_proto::dealer::{
        PingReply, PingRequest,
        dealer_server::{Dealer, DealerServer},
    };
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::{Request, Response, Status, transport::Server};

    #[derive(Debug, Default)]
    struct TestDealerService;

    #[tonic::async_trait]
    impl Dealer for TestDealerService {
        async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
            let client_id = request.into_inner().client_id;
            Ok(Response::new(PingReply {
                message: format!("pong:{client_id}"),
            }))
        }
    }

    #[tokio::test]
    async fn ping_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let incoming = TcpListenerStream::new(listener);

        let server_handle = tokio::spawn(async move {
            Server::builder()
                .add_service(DealerServer::new(TestDealerService))
                .serve_with_incoming(incoming)
                .await
        });

        let endpoint = format!("http://{addr}");
        let message = ping(&endpoint, "client-7").await?;

        assert_eq!(message, "pong:client-7");

        server_handle.abort();
        Ok(())
    }
}
