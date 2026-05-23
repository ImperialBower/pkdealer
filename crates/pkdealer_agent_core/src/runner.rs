//! Agent run loop: connect, seat, stream events, and act.

use pkdealer_proto::dealer::{
    ActRequest, ActionType, EventType, GetNextToActRequest, GetTableConfigRequest, NextToActInfo,
    PlayerAction as ProtoAction, SeatPlayerAtRequest, SeatPlayerRequest, StartHandRequest,
    StreamEventsRequest, Street, TableStatus, act_response,
    dealer_service_client::DealerServiceClient, get_next_to_act_response, seat_player_at_response,
    seat_player_response,
};

use crate::{AgentError, Decision, HandState, PokerAgent, hand_state::street_name};

const PLAYER_TOKEN_METADATA_KEY: &str = "x-player-token";

/// Configuration for connecting and seating an agent at a pkdealer table.
///
/// # Examples
///
/// ```rust
/// use pkdealer_agent_core::AgentConfig;
///
/// let cfg = AgentConfig {
///     endpoint: "http://127.0.0.1:50051".to_string(),
///     name: "rando".to_string(),
///     seat: None,
///     chips: 10_000,
///     client_secret: String::new(),
/// };
/// assert_eq!(cfg.chips, 10_000);
/// assert!(cfg.seat.is_none());
/// ```
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// gRPC service address, e.g. `"http://127.0.0.1:50051"`.
    pub endpoint: String,
    /// Player name displayed at the table.
    pub name: String,
    /// Specific seat to request. `None` → next available seat.
    pub seat: Option<u32>,
    /// Buy-in chip count. `0` → server default (10 000).
    pub chips: u32,
    /// Opaque seat-resume token. Empty string disables seat resume.
    pub client_secret: String,
}

/// Fixed per-session values threaded into [`decide_and_act`].
struct SeatCtx<'a> {
    name: &'a str,
    seat: u8,
    token: &'a str,
    big_blind: u32,
}

/// Connect to the service, seat the agent, and run the event-driven play loop.
///
/// The loop reads from the `StreamEvents` gRPC stream and calls
/// [`PokerAgent::decide`] each time the server signals it is this agent's turn.
/// The loop terminates when the event stream closes (server shutdown) or an
/// unrecoverable error occurs.
///
/// # Errors
///
/// Returns [`AgentError::Connect`] if the gRPC channel cannot be established,
/// [`AgentError::Seat`] if the service rejects the seat request,
/// [`AgentError::Rpc`] for gRPC status errors during play, and
/// [`AgentError::InvalidMetadata`] if the player token cannot be parsed as
/// HTTP/2 metadata.
///
/// # Examples
///
/// ```rust,no_run
/// use pkdealer_agent_core::{AgentConfig, Decision, HandState, PokerAgent, run_agent};
///
/// struct AlwaysFold;
///
/// #[async_trait::async_trait]
/// impl PokerAgent for AlwaysFold {
///     async fn decide(&self, _state: &HandState) -> Decision {
///         Decision::Fold
///     }
/// }
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = AgentConfig {
///     endpoint: "http://127.0.0.1:50051".to_string(),
///     name: "folder".to_string(),
///     seat: None,
///     chips: 10_000,
///     client_secret: String::new(),
/// };
/// run_agent(AlwaysFold, config).await?;
/// # Ok(())
/// # }
/// ```
pub async fn run_agent<A: PokerAgent>(agent: A, config: AgentConfig) -> Result<(), AgentError> {
    let mut client = DealerServiceClient::connect(config.endpoint.clone()).await?;

    let (my_seat, my_token) = seat_agent(&mut client, &config).await?;
    eprintln!("[{}] seated at seat {my_seat}", config.name);

    let big_blind = fetch_big_blind(&mut client).await?;
    let ctx = SeatCtx {
        name: &config.name,
        seat: my_seat,
        token: &my_token,
        big_blind,
    };

    let stream_req = StreamEventsRequest {
        player_token: my_token.clone(),
    };
    let mut event_stream = client
        .stream_events(tonic::Request::new(stream_req))
        .await?
        .into_inner();

    // Attempt to start the first hand. Both agents race here; the loser gets
    // an error which is intentionally ignored.
    try_start_hand(&mut client).await;

    let mut action_history: Vec<String> = Vec::new();

    loop {
        let Some(event) = event_stream.message().await? else {
            break;
        };

        match EventType::try_from(event.event_type).unwrap_or(EventType::Unspecified) {
            EventType::HandStarted => {
                action_history.clear();
                eprintln!("[{}] hand started", config.name);
            }
            EventType::HandEnded => {
                eprintln!("[{}] hand ended — {}", config.name, event.description);
                try_start_hand(&mut client).await;
            }
            EventType::StreetAdvanced => action_history.clear(),
            EventType::PlayerAction => action_history.push(event.description.clone()),
            _ => {}
        }

        let Some(status) = event.current_status else {
            continue;
        };
        if !status.hand_in_progress {
            continue;
        }

        // status.next_to_act is captured before auto-advance runs and can be
        // stale. Ask the service for the authoritative current actor instead.
        let Some(info) = fetch_next_to_act_info(&mut client).await? else {
            continue;
        };
        if info.seat != u32::from(ctx.seat) {
            continue;
        }

        decide_and_act(&mut client, &agent, &ctx, &status, &info, &action_history).await?;
    }

    Ok(())
}

async fn decide_and_act<A: PokerAgent>(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
    agent: &A,
    ctx: &SeatCtx<'_>,
    status: &TableStatus,
    info: &NextToActInfo,
    action_history: &[String],
) -> Result<(), AgentError> {
    let my_seat = ctx.seat;
    let my_cards = status
        .seats
        .iter()
        .find(|s| s.seat_number == u32::from(my_seat))
        .map(|s| s.cards.clone())
        .unwrap_or_default();
    let my_chips = status
        .seats
        .iter()
        .find(|s| s.seat_number == u32::from(my_seat))
        .map_or(0, |s| s.chips);
    let stacks: Vec<(u8, String, u32)> = status
        .seats
        .iter()
        .filter_map(|s| {
            u8::try_from(s.seat_number)
                .ok()
                .map(|n| (n, s.player_name.clone(), s.chips))
        })
        .collect();
    let street_str =
        street_name(Street::try_from(status.current_street).unwrap_or(Street::Unspecified));

    let hand_state = HandState {
        seat: my_seat,
        hole_cards: my_cards,
        board: status.board.clone(),
        pot: info.pot,
        to_call: info.amount_to_call,
        my_chips,
        stacks,
        big_blind: ctx.big_blind,
        street: street_str.to_string(),
        action_history: action_history.to_vec(),
    };

    let decision = agent.decide(&hand_state).await;
    // Raise(n) takes a total-amount; min_raise is the minimum increment above
    // the call.  Minimum valid total = amount_to_call + min_raise.  When
    // min_raise is 0 (blinds not yet swept into pot preflop) fall back to
    // 2× BB per standard NLHE rules.
    let floor_raise = if info.min_raise > 0 {
        info.amount_to_call.saturating_add(info.min_raise)
    } else if info.amount_to_call > 0 {
        ctx.big_blind.saturating_mul(2)
    } else {
        0
    };
    let decision = match decision {
        Decision::Raise(n) if n < floor_raise => Decision::Raise(floor_raise),
        // Preflop the blind is a live bet; Bet is invalid when to_call==0
        // (BB's option). Convert to Check so the service never rejects it.
        Decision::Bet(_) if hand_state.street == "preflop" && info.amount_to_call == 0 => {
            Decision::Check
        }
        other => other,
    };
    eprintln!(
        "[{}] seat={my_seat} {} pot={} to_call={} → {decision:?}",
        ctx.name, hand_state.street, info.pot, info.amount_to_call
    );

    let proto_action = decision_to_proto(my_seat, &decision);
    let mut act_req = tonic::Request::new(ActRequest {
        action: Some(proto_action),
    });
    act_req
        .metadata_mut()
        .insert(PLAYER_TOKEN_METADATA_KEY, ctx.token.parse()?);
    let resp = client.act(act_req).await?.into_inner();
    if let Some(act_response::Result::Error(e)) = resp.result {
        // Service rejected the action; fall back to a safe action so the game
        // isn't left stuck waiting for this seat.
        let safe = if hand_state.to_call > 0 {
            Decision::Fold
        } else {
            Decision::Check
        };
        eprintln!(
            "[{}] act rejected ({e}) — falling back to {safe:?}",
            ctx.name
        );
        let proto_safe = decision_to_proto(my_seat, &safe);
        let mut safe_req = tonic::Request::new(ActRequest {
            action: Some(proto_safe),
        });
        safe_req
            .metadata_mut()
            .insert(PLAYER_TOKEN_METADATA_KEY, ctx.token.parse()?);
        client.act(safe_req).await?;
    }
    Ok(())
}

// Both agents race to call StartHand after seating and after each HandEnded.
// The service rejects the duplicate; the error is intentionally discarded.
async fn try_start_hand(client: &mut DealerServiceClient<tonic::transport::Channel>) {
    let _ = client
        .start_hand(tonic::Request::new(StartHandRequest {}))
        .await;
}

async fn seat_agent(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
    config: &AgentConfig,
) -> Result<(u8, String), AgentError> {
    if let Some(seat_num) = config.seat {
        let req = SeatPlayerAtRequest {
            seat: seat_num,
            name: config.name.clone(),
            chips: config.chips,
            client_secret: config.client_secret.clone(),
        };
        let resp = client
            .seat_player_at(tonic::Request::new(req))
            .await?
            .into_inner();
        if let Some(seat_player_at_response::Result::Error(e)) = resp.result {
            return Err(AgentError::Seat(e));
        }
        let seat = u8::try_from(seat_num)
            .map_err(|_| AgentError::Seat(format!("seat {seat_num} exceeds u8 range")))?;
        Ok((seat, resp.player_token))
    } else {
        let req = SeatPlayerRequest {
            name: config.name.clone(),
            chips: config.chips,
            client_secret: config.client_secret.clone(),
        };
        let resp = client
            .seat_player(tonic::Request::new(req))
            .await?
            .into_inner();
        match resp.result {
            Some(seat_player_response::Result::SeatNumber(n)) => {
                let seat = u8::try_from(n)
                    .map_err(|_| AgentError::Seat(format!("seat number {n} exceeds u8 range")))?;
                Ok((seat, resp.player_token))
            }
            Some(seat_player_response::Result::Error(e)) => Err(AgentError::Seat(e)),
            None => Err(AgentError::Seat("empty SeatPlayer response".to_string())),
        }
    }
}

async fn fetch_big_blind(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
) -> Result<u32, AgentError> {
    let resp = client
        .get_table_config(tonic::Request::new(GetTableConfigRequest {}))
        .await?
        .into_inner();
    Ok(resp.config.map_or(100, |c| c.big_blind))
}

async fn fetch_next_to_act_info(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
) -> Result<Option<NextToActInfo>, AgentError> {
    let resp = client
        .get_next_to_act(tonic::Request::new(GetNextToActRequest {}))
        .await?
        .into_inner();
    Ok(match resp.result {
        Some(get_next_to_act_response::Result::Info(info)) => Some(info),
        _ => None,
    })
}

fn decision_to_proto(seat: u8, decision: &Decision) -> ProtoAction {
    let (action_type, amount) = match decision {
        Decision::Fold => (ActionType::Fold, 0),
        Decision::Check => (ActionType::Check, 0),
        Decision::Call => (ActionType::Call, 0),
        Decision::Bet(n) => (ActionType::Bet, *n),
        Decision::Raise(n) => (ActionType::Raise, *n),
        Decision::AllIn => (ActionType::AllIn, 0),
    };
    ProtoAction {
        seat: u32::from(seat),
        action_type: action_type as i32,
        amount,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_construction_happy_path() {
        let cfg = AgentConfig {
            endpoint: "http://localhost:50051".to_string(),
            name: "test-agent".to_string(),
            seat: None,
            chips: 10_000,
            client_secret: String::new(),
        };
        assert_eq!(cfg.endpoint, "http://localhost:50051");
        assert_eq!(cfg.name, "test-agent");
        assert!(cfg.seat.is_none());
        assert_eq!(cfg.chips, 10_000);
        assert!(cfg.client_secret.is_empty());
    }

    #[test]
    fn test_agent_config_with_specific_seat() {
        let cfg = AgentConfig {
            endpoint: "http://localhost:50051".to_string(),
            name: "seated".to_string(),
            seat: Some(3),
            chips: 5_000,
            client_secret: "resume-token".to_string(),
        };
        assert_eq!(cfg.seat, Some(3));
        assert_eq!(cfg.client_secret, "resume-token");
    }

    #[test]
    fn test_agent_config_clone() {
        let cfg = AgentConfig {
            endpoint: "http://localhost:50051".to_string(),
            name: "orig".to_string(),
            seat: None,
            chips: 10_000,
            client_secret: String::new(),
        };
        let cloned = cfg.clone();
        assert_eq!(cloned.name, cfg.name);
        assert_eq!(cloned.chips, cfg.chips);
    }

    #[test]
    fn test_decision_to_proto_fold() {
        let action = decision_to_proto(3, &Decision::Fold);
        assert_eq!(action.seat, 3);
        assert_eq!(action.action_type, ActionType::Fold as i32);
        assert_eq!(action.amount, 0);
    }

    #[test]
    fn test_decision_to_proto_check() {
        let action = decision_to_proto(5, &Decision::Check);
        assert_eq!(action.action_type, ActionType::Check as i32);
        assert_eq!(action.amount, 0);
    }

    #[test]
    fn test_decision_to_proto_call() {
        let action = decision_to_proto(2, &Decision::Call);
        assert_eq!(action.action_type, ActionType::Call as i32);
        assert_eq!(action.amount, 0);
    }

    #[test]
    fn test_decision_to_proto_bet() {
        let action = decision_to_proto(1, &Decision::Bet(250));
        assert_eq!(action.seat, 1);
        assert_eq!(action.action_type, ActionType::Bet as i32);
        assert_eq!(action.amount, 250);
    }

    #[test]
    fn test_decision_to_proto_raise() {
        let action = decision_to_proto(0, &Decision::Raise(400));
        assert_eq!(action.action_type, ActionType::Raise as i32);
        assert_eq!(action.amount, 400);
    }

    #[test]
    fn test_decision_to_proto_all_in() {
        let action = decision_to_proto(7, &Decision::AllIn);
        assert_eq!(action.action_type, ActionType::AllIn as i32);
        assert_eq!(action.amount, 0);
    }

    #[test]
    fn test_decision_to_proto_seat_encoded() {
        let action = decision_to_proto(8, &Decision::Fold);
        assert_eq!(action.seat, 8);
    }
}
