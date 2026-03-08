#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! # Poker Dealer Client
//!
//! A gRPC client for interacting with the poker dealer service.

use std::process;

use pkdealer_proto::{dealer::dealer_service_client::DealerServiceClient, new_ping_request};

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
    let endpoint =
        std::env::var("PKDEALER_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_owned());
    let client_id =
        std::env::var("PKDEALER_CLIENT_ID").unwrap_or_else(|_| DEFAULT_CLIENT_ID.to_owned());

    println!("Poker Dealer Client v{}", env!("CARGO_PKG_VERSION"));
    println!("Connecting to service at {endpoint}...");

    let message = ping(&endpoint, &client_id).await?;
    println!("Service response: {message}");

    Ok(())
}

async fn ping(endpoint: &str, client_id: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut client = DealerServiceClient::connect(endpoint.to_owned()).await?;
    let request = tonic::Request::new(new_ping_request(client_id));
    let response = client.ping(request).await?;

    Ok(response.into_inner().message)
}

#[cfg(test)]
mod tests {
    use super::*;

    use pkdealer_proto::dealer::{
        ActRequest, ActResponse, AdvanceStreetRequest, AdvanceStreetResponse, EndHandRequest,
        EndHandResponse, GetBoardRequest, GetBoardResponse, GetChipsRequest, GetChipsResponse,
        GetEventLogRequest, GetEventLogResponse, GetNextToActRequest, GetNextToActResponse,
        GetPotRequest, GetPotResponse, GetStatusRequest, GetStatusResponse, PingReply, PingRequest,
        RemovePlayerRequest, RemovePlayerResponse, SeatPlayerAtRequest, SeatPlayerAtResponse,
        SeatPlayerRequest, SeatPlayerResponse, StartHandRequest, StartHandResponse,
        StreamEventsRequest, TableEvent,
        dealer_service_server::{DealerService as DealerServiceTrait, DealerServiceServer},
    };
    use tokio::net::TcpListener;
    use tokio_stream::wrappers::TcpListenerStream;
    use tonic::{Request, Response, Status, transport::Server};

    #[derive(Debug, Default)]
    struct TestDealerService;

    #[tonic::async_trait]
    impl DealerServiceTrait for TestDealerService {
        async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
            let client_id = request.into_inner().client_id;
            Ok(Response::new(PingReply {
                message: format!("pong:{client_id}"),
            }))
        }

        async fn seat_player(
            &self,
            _request: Request<SeatPlayerRequest>,
        ) -> Result<Response<SeatPlayerResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn seat_player_at(
            &self,
            _request: Request<SeatPlayerAtRequest>,
        ) -> Result<Response<SeatPlayerAtResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn remove_player(
            &self,
            _request: Request<RemovePlayerRequest>,
        ) -> Result<Response<RemovePlayerResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn start_hand(
            &self,
            _request: Request<StartHandRequest>,
        ) -> Result<Response<StartHandResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn advance_street(
            &self,
            _request: Request<AdvanceStreetRequest>,
        ) -> Result<Response<AdvanceStreetResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn end_hand(
            &self,
            _request: Request<EndHandRequest>,
        ) -> Result<Response<EndHandResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn act(
            &self,
            _request: Request<ActRequest>,
        ) -> Result<Response<ActResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn get_status(
            &self,
            _request: Request<GetStatusRequest>,
        ) -> Result<Response<GetStatusResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn get_next_to_act(
            &self,
            _request: Request<GetNextToActRequest>,
        ) -> Result<Response<GetNextToActResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn get_board(
            &self,
            _request: Request<GetBoardRequest>,
        ) -> Result<Response<GetBoardResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn get_chips(
            &self,
            _request: Request<GetChipsRequest>,
        ) -> Result<Response<GetChipsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn get_pot(
            &self,
            _request: Request<GetPotRequest>,
        ) -> Result<Response<GetPotResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        async fn get_event_log(
            &self,
            _request: Request<GetEventLogRequest>,
        ) -> Result<Response<GetEventLogResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }

        type StreamEventsStream =
            tokio_stream::wrappers::ReceiverStream<Result<TableEvent, Status>>;

        async fn stream_events(
            &self,
            _request: Request<StreamEventsRequest>,
        ) -> Result<Response<Self::StreamEventsStream>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
    }

    #[tokio::test]
    async fn ping_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let incoming = TcpListenerStream::new(listener);

        let server_handle = tokio::spawn(async move {
            Server::builder()
                .add_service(DealerServiceServer::new(TestDealerService))
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
