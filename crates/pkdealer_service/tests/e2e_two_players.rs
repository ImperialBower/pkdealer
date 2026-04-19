//! End-to-end test: two independent gRPC clients playing through a hand.
//!
//! Validates that `x-player-token` metadata is correctly transported over a
//! real HTTP/2 connection and that the server enforces seat ownership across
//! separate client connections.

use std::{
    io,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

use pkdealer_proto::dealer::{
    ActRequest, ActionType, GetChipsRequest, GetNextToActRequest, PlayerAction, SeatPlayerRequest,
    StartHandRequest, act_response, dealer_service_client::DealerServiceClient,
    get_next_to_act_response, seat_player_response,
};
use tonic::{Request, metadata::MetadataValue};

// ── process helpers ───────────────────────────────────────────────────────────

struct ChildProcessGuard {
    child: Child,
}

impl ChildProcessGuard {
    fn new(child: Child) -> Self {
        Self { child }
    }
}

impl Drop for ChildProcessGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reserve_local_port() -> io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn service_bin_path() -> io::Result<PathBuf> {
    std::env::var("CARGO_BIN_EXE_pkdealer_service")
        .map(PathBuf::from)
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e))
}

async fn wait_for_service_ready(endpoint: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(mut c) = DealerServiceClient::connect(endpoint.to_owned()).await
            && c.ping(Request::new(pkdealer_proto::new_ping_request("ready")))
                .await
                .is_ok()
        {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ── client helper ─────────────────────────────────────────────────────────────

const PLAYER_TOKEN_KEY: &str = "x-player-token";

type GrpcClient = DealerServiceClient<tonic::transport::Channel>;

/// Wraps a gRPC client with the seat number and auth token issued at seating.
struct PlayerClient {
    client: GrpcClient,
    seat: u32,
    token: String,
}

impl PlayerClient {
    async fn connect(
        endpoint: &str,
        name: &str,
        chips: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut client = DealerServiceClient::connect(endpoint.to_owned()).await?;
        let inner = client
            .seat_player(Request::new(SeatPlayerRequest {
                name: name.to_owned(),
                chips,
            }))
            .await?
            .into_inner();
        let seat = match inner.result {
            Some(seat_player_response::Result::SeatNumber(s)) => s,
            other => return Err(format!("seat_player failed: {other:?}").into()),
        };
        Ok(Self {
            client,
            seat,
            token: inner.player_token,
        })
    }

    /// Sends `Act` with this player's auth token attached as gRPC metadata.
    async fn act(
        &mut self,
        action: ActionType,
    ) -> Result<tonic::Response<pkdealer_proto::dealer::ActResponse>, tonic::Status> {
        let mut req = Request::new(ActRequest {
            action: Some(PlayerAction {
                seat: self.seat,
                action_type: action as i32,
                amount: 0,
            }),
        });
        req.metadata_mut().insert(
            PLAYER_TOKEN_KEY,
            self.token.parse::<MetadataValue<_>>().expect("valid token"),
        );
        self.client.act(req).await
    }

    /// Sends `Act` with a *different* token to verify rejection.
    async fn act_with_foreign_token(
        &mut self,
        action: ActionType,
        foreign_token: &str,
    ) -> Result<tonic::Response<pkdealer_proto::dealer::ActResponse>, tonic::Status> {
        let mut req = Request::new(ActRequest {
            action: Some(PlayerAction {
                seat: self.seat,
                action_type: action as i32,
                amount: 0,
            }),
        });
        req.metadata_mut().insert(
            PLAYER_TOKEN_KEY,
            foreign_token
                .parse::<MetadataValue<_>>()
                .expect("valid token"),
        );
        self.client.act(req).await
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Two independent clients connect to the service, seat themselves, and play
/// through a complete hand.  The test also verifies that using the wrong
/// client's token is rejected with `PERMISSION_DENIED` over the wire.
#[tokio::test]
async fn e2e_two_players_full_hand_with_token_enforcement() -> Result<(), Box<dyn std::error::Error>>
{
    let service_path = service_bin_path()?;
    let port = reserve_local_port()?;
    let service_addr = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{service_addr}");

    let _guard = ChildProcessGuard::new(
        Command::new(&service_path)
            .env("PKDEALER_ADDR", &service_addr)
            .spawn()?,
    );

    assert!(
        wait_for_service_ready(&endpoint, Duration::from_secs(5)).await,
        "service should become ready"
    );

    // Two independent connections — each client seats itself and captures its token.
    let mut player_a = PlayerClient::connect(&endpoint, "Alice", 1_000).await?;
    let mut player_b = PlayerClient::connect(&endpoint, "Bob", 1_000).await?;

    // A third connection acts as the table orchestrator (start hand / observe).
    let mut orchestrator = DealerServiceClient::connect(endpoint.clone()).await?;
    orchestrator
        .start_hand(Request::new(StartHandRequest {}))
        .await?;

    // ── Wire-level auth rejection ────────────────────────────────────────────
    // Player A sends Act for their own seat but with Player B's token.
    // The server must reject this with PERMISSION_DENIED over HTTP/2.
    let token_b = player_b.token.clone();
    let rejection = player_a
        .act_with_foreign_token(ActionType::Fold, &token_b)
        .await;
    assert!(rejection.is_err(), "foreign token must be rejected");
    assert_eq!(
        rejection.unwrap_err().code(),
        tonic::Code::PermissionDenied,
        "rejection must be PERMISSION_DENIED, not a game error"
    );

    // ── Preflop betting ──────────────────────────────────────────────────────
    // First actor (SB) calls the big blind; second actor (BB) checks.
    // After BB's check the `act` handler auto-advances to the flop via next_step().
    for i in 0..2_usize {
        let next_seat = {
            let resp = orchestrator
                .get_next_to_act(Request::new(GetNextToActRequest {}))
                .await?
                .into_inner();
            match resp.result {
                Some(get_next_to_act_response::Result::Info(info)) => info.seat,
                _ => break,
            }
        };

        // SB (first to act preflop) calls; BB (second) checks.
        let action = if i == 0 {
            ActionType::Call
        } else {
            ActionType::Check
        };

        let actor = if next_seat == player_a.seat {
            &mut player_a
        } else {
            &mut player_b
        };

        let resp = actor.act(action).await?.into_inner();
        assert!(
            matches!(resp.result, Some(act_response::Result::ActionResult(_))),
            "preflop action must succeed"
        );
    }

    // ── Post-preflop: check until hand_complete ──────────────────────────────
    // Streets (flop, turn, river) are auto-advanced by the `act` handler.
    // We loop checking until ActionResult.hand_complete is true.
    let mut hand_complete = false;
    for _ in 0..20 {
        // safety cap: 4 streets × 2 players × some margin
        if hand_complete {
            break;
        }
        let next_seat = {
            let resp = orchestrator
                .get_next_to_act(Request::new(GetNextToActRequest {}))
                .await?
                .into_inner();
            match resp.result {
                Some(get_next_to_act_response::Result::Info(info)) => info.seat,
                _ => break, // hand already over
            }
        };
        let actor = if next_seat == player_a.seat {
            &mut player_a
        } else {
            &mut player_b
        };
        let resp = actor.act(ActionType::Check).await?.into_inner();
        match resp.result {
            Some(act_response::Result::ActionResult(r)) => hand_complete = r.hand_complete,
            Some(act_response::Result::Error(e)) => return Err(e.into()),
            None => return Err("empty act response".into()),
        }
    }
    assert!(hand_complete, "hand must reach completion via Act alone");

    // Chips must be conserved across the full round trip.
    let chips = orchestrator
        .get_chips(Request::new(GetChipsRequest {}))
        .await?
        .into_inner()
        .chips;
    let total: u32 = chips.iter().map(|p| p.chips).sum();
    assert_eq!(total, 2_000, "chips must be conserved end-to-end");

    Ok(())
}

/// Verifies that two players with identical connection parameters receive
/// distinct tokens and that each token is seat-specific.
#[tokio::test]
async fn e2e_two_players_receive_distinct_tokens() -> Result<(), Box<dyn std::error::Error>> {
    let service_path = service_bin_path()?;
    let port = reserve_local_port()?;
    let service_addr = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{service_addr}");

    let _guard = ChildProcessGuard::new(
        Command::new(&service_path)
            .env("PKDEALER_ADDR", &service_addr)
            .spawn()?,
    );

    assert!(
        wait_for_service_ready(&endpoint, Duration::from_secs(5)).await,
        "service should become ready"
    );

    let player_a = PlayerClient::connect(&endpoint, "Alice", 1_000).await?;
    let player_b = PlayerClient::connect(&endpoint, "Bob", 1_000).await?;

    assert_ne!(
        player_a.seat, player_b.seat,
        "players must occupy different seats"
    );
    assert_ne!(
        player_a.token, player_b.token,
        "each player must receive a unique token"
    );
    assert!(!player_a.token.is_empty());
    assert!(!player_b.token.is_empty());

    Ok(())
}
