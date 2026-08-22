//! End-to-end test: two independent gRPC clients playing through a hand.
//!
//! Validates that `x-player-token` metadata is correctly transported over a
//! real HTTP/2 connection and that the server enforces seat ownership across
//! separate client connections.

// The helpers below deliberately mirror the generated gRPC signature
// (`Result<Response<T>, tonic::Status>`) so they read like the API under test.
// `tonic::Status` is 176 bytes, so `result_large_err` fires; boxing the error
// here would only make the helpers diverge from the calls they wrap.
#![allow(clippy::result_large_err)]

use std::{
    io,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

use pkdealer_proto::dealer::{
    ActRequest, ActionType, AgentFidelity, ExportSessionRequest, GetChipsRequest,
    GetNextToActRequest, PlayerAction, SeatPlayerRequest, StartHandRequest, act_response,
    dealer_service_client::DealerServiceClient, get_next_to_act_response, seat_player_response,
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
                client_secret: String::new(),
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
                agent: None,
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
                agent: None,
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

    /// Sends `Act` with agent-fidelity metadata attached to the `PlayerAction`.
    async fn act_with_agent(
        &mut self,
        action: ActionType,
        agent: AgentFidelity,
    ) -> Result<tonic::Response<pkdealer_proto::dealer::ActResponse>, tonic::Status> {
        let mut req = Request::new(ActRequest {
            action: Some(PlayerAction {
                seat: self.seat,
                action_type: action as i32,
                amount: 0,
                agent: Some(agent),
            }),
        });
        req.metadata_mut().insert(
            PLAYER_TOKEN_KEY,
            self.token.parse::<MetadataValue<_>>().expect("valid token"),
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
        wait_for_service_ready(&endpoint, Duration::from_secs(15)).await,
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

    // ── EPIC-25: export the session and replay it off the wire ───────────────
    // The spectator token authorizes the export; the YAML must round-trip into a
    // HandCollection whose single hand replays with chip conservation.
    let mut export_req = Request::new(ExportSessionRequest::default());
    export_req.metadata_mut().insert(
        PLAYER_TOKEN_KEY,
        MetadataValue::try_from("spectator").expect("valid token"),
    );
    let export = orchestrator.export_session(export_req).await?.into_inner();
    assert_eq!(export.hand_count, 1, "one hand recorded this session");
    assert_eq!(export.source, "arena");

    let collection = pkcore::hand_history::HandCollection::from_yaml(&export.payload)
        .expect("exported YAML parses as a HandCollection");
    assert_eq!(collection.len(), 1);
    let replay = collection.hands[0].replay().expect("replay succeeds");
    assert!(
        replay.is_consistent,
        "exported hand must replay with chip conservation"
    );

    // Export without the spectator token must be denied (payload has all cards).
    let denied = orchestrator
        .export_session(Request::new(ExportSessionRequest::default()))
        .await;
    assert_eq!(
        denied.unwrap_err().code(),
        tonic::Code::PermissionDenied,
        "export without spectator token must be PERMISSION_DENIED"
    );

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
        wait_for_service_ready(&endpoint, Duration::from_secs(15)).await,
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

/// Counts actions carrying agent-fidelity across every street of a hand.
fn count_agent_actions(hand: &pkcore::hand_history::HandHistory) -> usize {
    let Some(streets) = hand.streets.as_ref() else {
        return 0;
    };
    [
        streets.preflop.as_ref().map(|s| &s.actions),
        streets.flop.as_ref().map(|s| &s.actions),
        streets.turn.as_ref().map(|s| &s.actions),
        streets.river.as_ref().map(|s| &s.actions),
    ]
    .into_iter()
    .flatten()
    .flat_map(|acts| acts.iter())
    .filter(|a| a.agent.is_some())
    .count()
}

/// EPIC-25 Phase 4: agent-fidelity submitted on an `Act` is recorded onto the
/// matching action in the exported `HandHistory`, while acts submitted without
/// it stay clean (no empty `agent` blocks) and replay is unaffected.
#[tokio::test]
async fn e2e_agent_fidelity_recorded_in_hand_history() -> Result<(), Box<dyn std::error::Error>> {
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
        wait_for_service_ready(&endpoint, Duration::from_secs(15)).await,
        "service should become ready"
    );

    let mut player_a = PlayerClient::connect(&endpoint, "Alice", 1_000).await?;
    let mut player_b = PlayerClient::connect(&endpoint, "Bob", 1_000).await?;
    let mut orchestrator = DealerServiceClient::connect(endpoint.clone()).await?;
    orchestrator
        .start_hand(Request::new(StartHandRequest {}))
        .await?;

    // Play to completion. Attach agent-fidelity to the FIRST voluntary act only;
    // every later act is a bare check (no agent data), exercising the strip pass.
    let mut tagged_first = false;
    let mut hand_complete = false;
    for _ in 0..24 {
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
                _ => break,
            }
        };
        let actor = if next_seat == player_a.seat {
            &mut player_a
        } else {
            &mut player_b
        };
        let resp = if tagged_first {
            actor.act(ActionType::Check).await?.into_inner()
        } else {
            tagged_first = true;
            let agent = AgentFidelity {
                raw_response: Some("call to keep range wide".to_string()),
                was_coerced: Some(true),
                intended_action_type: Some(ActionType::Raise as i32),
                intended_amount: Some(300),
                input_tokens: Some(1234),
                output_tokens: Some(7),
                model: Some("claude-e2e".to_string()),
                prompt: Some("e2e prompt".to_string()),
            };
            actor
                .act_with_agent(ActionType::Call, agent)
                .await?
                .into_inner()
        };
        match resp.result {
            Some(act_response::Result::ActionResult(r)) => hand_complete = r.hand_complete,
            Some(act_response::Result::Error(e)) => return Err(e.into()),
            None => return Err("empty act response".into()),
        }
    }
    assert!(hand_complete, "hand must reach completion via Act alone");

    // Export and inspect the recorded hand off the wire.
    let mut export_req = Request::new(ExportSessionRequest::default());
    export_req.metadata_mut().insert(
        PLAYER_TOKEN_KEY,
        MetadataValue::try_from("spectator").expect("valid token"),
    );
    let export = orchestrator.export_session(export_req).await?.into_inner();
    assert_eq!(export.hand_count, 1);

    let collection = pkcore::hand_history::HandCollection::from_yaml(&export.payload)
        .expect("exported YAML parses as a HandCollection");
    let hand = &collection.hands[0];

    // Exactly one action across the whole hand is annotated — the strip pass
    // cleared the empty placeholders buffered for the bare-check acts.
    assert_eq!(
        count_agent_actions(hand),
        1,
        "only the agent-tagged act keeps an agent block"
    );

    // That one annotation carries exactly the values we sent, converted to the
    // pkcore representation (enum mapped, amount widened to f64).
    let agent = hand
        .streets
        .as_ref()
        .and_then(|s| s.preflop.as_ref())
        .expect("preflop street")
        .actions
        .iter()
        .find_map(|a| a.agent.as_ref())
        .expect("an annotated preflop action");
    assert_eq!(
        agent.raw_response.as_deref(),
        Some("call to keep range wide")
    );
    assert_eq!(agent.was_coerced, Some(true));
    assert_eq!(
        agent.intended_action,
        Some(pkcore::hand_history::ActionType::Raise)
    );
    assert_eq!(agent.intended_amount, Some(300.0));
    assert_eq!(agent.input_tokens, Some(1234));
    assert_eq!(agent.output_tokens, Some(7));
    assert_eq!(agent.model.as_deref(), Some("claude-e2e"));

    // Replay ignores the metadata — the hand still replays consistently.
    assert!(
        hand.replay().expect("replay succeeds").is_consistent,
        "agent metadata must not affect replay"
    );

    Ok(())
}
