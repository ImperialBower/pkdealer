#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! # Poker Dealer Service
//!
//! A gRPC service that provides poker dealer functionality.

use std::{env, net::SocketAddr, process};

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
use tonic::{Request, Response, Status, transport::Server};

const DEFAULT_SERVICE_ADDR: &str = "127.0.0.1:50051";

#[derive(Debug, Default)]
struct DealerService;

#[tonic::async_trait]
impl DealerServiceTrait for DealerService {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
        let client_id = request.into_inner().client_id;
        let message = if client_id.is_empty() {
            "pong".to_owned()
        } else {
            format!("pong:{client_id}")
        };

        Ok(Response::new(PingReply { message }))
    }

    async fn seat_player(
        &self,
        _request: Request<SeatPlayerRequest>,
    ) -> Result<Response<SeatPlayerResponse>, Status> {
        Err(Status::unimplemented("seat_player not yet implemented"))
    }

    async fn seat_player_at(
        &self,
        _request: Request<SeatPlayerAtRequest>,
    ) -> Result<Response<SeatPlayerAtResponse>, Status> {
        Err(Status::unimplemented("seat_player_at not yet implemented"))
    }

    async fn remove_player(
        &self,
        _request: Request<RemovePlayerRequest>,
    ) -> Result<Response<RemovePlayerResponse>, Status> {
        Err(Status::unimplemented("remove_player not yet implemented"))
    }

    async fn start_hand(
        &self,
        _request: Request<StartHandRequest>,
    ) -> Result<Response<StartHandResponse>, Status> {
        Err(Status::unimplemented("start_hand not yet implemented"))
    }

    async fn advance_street(
        &self,
        _request: Request<AdvanceStreetRequest>,
    ) -> Result<Response<AdvanceStreetResponse>, Status> {
        Err(Status::unimplemented("advance_street not yet implemented"))
    }

    async fn end_hand(
        &self,
        _request: Request<EndHandRequest>,
    ) -> Result<Response<EndHandResponse>, Status> {
        Err(Status::unimplemented("end_hand not yet implemented"))
    }

    async fn act(&self, _request: Request<ActRequest>) -> Result<Response<ActResponse>, Status> {
        Err(Status::unimplemented("act not yet implemented"))
    }

    async fn get_status(
        &self,
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        Err(Status::unimplemented("get_status not yet implemented"))
    }

    async fn get_next_to_act(
        &self,
        _request: Request<GetNextToActRequest>,
    ) -> Result<Response<GetNextToActResponse>, Status> {
        Err(Status::unimplemented("get_next_to_act not yet implemented"))
    }

    async fn get_board(
        &self,
        _request: Request<GetBoardRequest>,
    ) -> Result<Response<GetBoardResponse>, Status> {
        Err(Status::unimplemented("get_board not yet implemented"))
    }

    async fn get_chips(
        &self,
        _request: Request<GetChipsRequest>,
    ) -> Result<Response<GetChipsResponse>, Status> {
        Err(Status::unimplemented("get_chips not yet implemented"))
    }

    async fn get_pot(
        &self,
        _request: Request<GetPotRequest>,
    ) -> Result<Response<GetPotResponse>, Status> {
        Err(Status::unimplemented("get_pot not yet implemented"))
    }

    async fn get_event_log(
        &self,
        _request: Request<GetEventLogRequest>,
    ) -> Result<Response<GetEventLogResponse>, Status> {
        Err(Status::unimplemented("get_event_log not yet implemented"))
    }

    type StreamEventsStream = tokio_stream::wrappers::ReceiverStream<Result<TableEvent, Status>>;

    async fn stream_events(
        &self,
        _request: Request<StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        Err(Status::unimplemented("stream_events not yet implemented"))
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
        .add_service(DealerServiceServer::new(DealerService))
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
