//! Agent run loop: connect, seat, stream events, and act.

use std::time::Duration;

use pkdealer_proto::dealer::{
    ActRequest, ActionType, AgentFidelity as ProtoAgentFidelity, EventType, GetNextToActRequest,
    GetStatusRequest, GetTableConfigRequest, NextToActInfo, PlayerAction as ProtoAction,
    SeatPlayerAtRequest, SeatPlayerRequest, StartHandRequest, StreamEventsRequest, Street,
    TableStatus, act_response, dealer_service_client::DealerServiceClient,
    get_next_to_act_response, seat_player_at_response, seat_player_response,
};

use crate::{AgentError, AgentFidelity, Decision, HandState, PokerAgent, hand_state::street_name};

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
    /// Pause applied before this agent submits each action, so a spectator can
    /// follow the table. Read once at startup from `PKDEALER_ACTION_DELAY_SECS`.
    action_delay: Duration,
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
    let action_delay = delay_from_env("PKDEALER_ACTION_DELAY_SECS", DEFAULT_ACTION_DELAY);
    let hand_end_delay = delay_from_env("PKDEALER_HAND_END_DELAY_SECS", DEFAULT_HAND_END_DELAY);
    let ctx = SeatCtx {
        name: &config.name,
        seat: my_seat,
        token: &my_token,
        big_blind,
        action_delay,
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
        // Reconcile fallback: the dealer is purely event-driven, so if this
        // agent ever misses its turn-prompt (a dropped/late StreetAdvanced after
        // a reconnect, or a HandStarted that arrived before we subscribed) it
        // would block here forever and wedge the whole table. Time-box the wait
        // so that, even with no event, we periodically re-check whether the
        // table is silently waiting on us and act if so.
        match tokio::time::timeout(RECONCILE_INTERVAL, event_stream.message()).await {
            Ok(message) => {
                let Some(event) = message? else {
                    break;
                };

                match EventType::try_from(event.event_type).unwrap_or(EventType::Unspecified) {
                    EventType::HandStarted => {
                        action_history.clear();
                        eprintln!("[{}] hand started", config.name);
                    }
                    EventType::HandEnded => {
                        eprintln!("[{}] hand ended — {}", config.name, event.description);
                        // Pause after every hand — showdown or fold-win alike —
                        // so viewers can see how it ended before the next deal.
                        if !hand_end_delay.is_zero() {
                            tokio::time::sleep(hand_end_delay).await;
                        }
                        try_start_hand(&mut client).await;
                    }
                    EventType::StreetAdvanced => action_history.clear(),
                    EventType::PlayerAction => action_history.push(event.description.clone()),
                    _ => {}
                }

                let Some(status) = event.current_status else {
                    continue;
                };
                act_if_my_turn(&mut client, &agent, &ctx, &status, &action_history).await?;
            }
            Err(_elapsed) => {
                // No event for RECONCILE_INTERVAL. Pull the authoritative status
                // and act if the table is stuck waiting on this seat.
                let Some(status) = fetch_status(&mut client).await? else {
                    continue;
                };
                act_if_my_turn(&mut client, &agent, &ctx, &status, &action_history).await?;
            }
        }
    }

    Ok(())
}

/// How long the play loop waits for a stream event before falling back to a
/// status reconcile. Short enough that a missed turn-prompt recovers quickly,
/// long enough that idle agents don't poll the service hard.
const RECONCILE_INTERVAL: Duration = Duration::from_secs(3);

/// Default pause before each action, overridable with `PKDEALER_ACTION_DELAY_SECS`.
const DEFAULT_ACTION_DELAY: Duration = Duration::from_secs(1);

/// Default pause after a hand ends, overridable with `PKDEALER_HAND_END_DELAY_SECS`.
const DEFAULT_HAND_END_DELAY: Duration = Duration::from_secs(5);

/// Parses a delay given in (possibly fractional) seconds.
///
/// Falls back to `default` when the value is absent, unparseable, negative, or
/// non-finite. A value of `"0"` is honoured and disables the pause.
fn parse_delay(raw: Option<&str>, default: Duration) -> Duration {
    match raw.map(str::trim).map(str::parse::<f64>) {
        Some(Ok(secs)) if secs.is_finite() && secs >= 0.0 => Duration::from_secs_f64(secs),
        _ => default,
    }
}

/// Reads a delay in seconds from environment variable `key`, using `default`
/// when the variable is unset or invalid.
fn delay_from_env(key: &str, default: Duration) -> Duration {
    parse_delay(std::env::var(key).ok().as_deref(), default)
}

/// Acts when the table is in progress and the authoritative next-to-act is this
/// agent's seat; otherwise does nothing. Shared by the event-driven path and
/// the reconcile-timeout path so both recover a stuck seat identically.
async fn act_if_my_turn<A: PokerAgent>(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
    agent: &A,
    ctx: &SeatCtx<'_>,
    status: &TableStatus,
    action_history: &[String],
) -> Result<(), AgentError> {
    if !status.hand_in_progress {
        return Ok(());
    }

    // status.next_to_act is captured before auto-advance runs and can be stale.
    // Ask the service for the authoritative current actor instead.
    let Some(info) = fetch_next_to_act_info(client).await? else {
        return Ok(());
    };
    if info.seat != u32::from(ctx.seat) {
        return Ok(());
    }

    decide_and_act(client, agent, ctx, status, &info, action_history).await
}

async fn decide_and_act<A: PokerAgent>(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
    agent: &A,
    ctx: &SeatCtx<'_>,
    status: &TableStatus,
    info: &NextToActInfo,
    action_history: &[String],
) -> Result<(), AgentError> {
    // Pace the table: hold this seat's turn briefly so a spectator can read
    // each action as it happens. Only the acting agent sleeps, so the delay
    // applies once per action across the table.
    if !ctx.action_delay.is_zero() {
        tokio::time::sleep(ctx.action_delay).await;
    }

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

    let (intended, mut fidelity) = agent.decide_with_fidelity(&hand_state).await;
    // Default the model id from the agent's configured name when the agent
    // itself didn't supply one (rules/random), so every arena action carries an
    // identifier in its recorded provenance.
    if fidelity.model.is_none() && !ctx.name.is_empty() {
        fidelity.model = Some(ctx.name.to_string());
    }
    // Raise(n) is a *total* bet level; pkcore validates `n - current_bet >= min_raise`.
    // The floor is therefore `current_bet + min_raise`, not `to_call + min_raise` —
    // those differ whenever the acting player already has chips in this street
    // (e.g. the small blind preflop, or the opener facing a re-raise).
    let floor_raise = info
        .current_bet
        .saturating_add(info.min_raise.max(ctx.big_blind));
    let is_preflop = hand_state.street == "preflop";
    let decision = finalize_decision(
        &intended,
        &mut fidelity,
        floor_raise,
        is_preflop,
        info.amount_to_call,
    );
    eprintln!(
        "[{}] seat={my_seat} {} pot={} to_call={} → {decision:?}",
        ctx.name, hand_state.street, info.pot, info.amount_to_call
    );

    if let Some(e) = send_action(client, my_seat, &decision, ctx.token, &fidelity).await? {
        // Service rejected the action; fall back to a safe action so the table
        // isn't left stuck waiting for this seat.
        let safe = if hand_state.to_call > 0 {
            Decision::Fold
        } else {
            Decision::Check
        };
        // Rejection retry (EPIC-25 Phase 4): the applied fallback is a coercion;
        // keep the agent's raw/token/model provenance but record the original
        // intent and the coerced flag.
        let coerced = AgentFidelity {
            was_coerced: Some(true),
            intended_action: Some(intended.clone()),
            ..fidelity.clone()
        };
        eprintln!(
            "[{}] act rejected ({e}) — falling back to {safe:?}",
            ctx.name
        );
        if let Some(e2) = send_action(client, my_seat, &safe, ctx.token, &coerced).await? {
            // The fallback was rejected too. Without escalating, the table hangs
            // forever on this seat (the dealer is purely reactive and only
            // advances when the acting seat submits a valid action). Fold is
            // legal on any turn, so it always frees the seat. Skip if `safe` was
            // already Fold — nothing further would help.
            eprintln!(
                "[{}] fallback {safe:?} rejected ({e2}) — folding to unblock table",
                ctx.name
            );
            if !matches!(safe, Decision::Fold)
                && let Some(e3) =
                    send_action(client, my_seat, &Decision::Fold, ctx.token, &coerced).await?
            {
                eprintln!("[{}] fold also rejected ({e3})", ctx.name);
            }
        }
    }
    Ok(())
}

/// Sends a single `Act` RPC for `seat` carrying the player `token`. Returns
/// `Ok(None)` when the service accepted the action, `Ok(Some(error))` when it
/// returned a structured rejection, and `Err` for transport/metadata failures.
async fn send_action(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
    seat: u8,
    decision: &Decision,
    token: &str,
    fidelity: &AgentFidelity,
) -> Result<Option<String>, AgentError> {
    let mut req = tonic::Request::new(ActRequest {
        action: Some(decision_to_proto(seat, decision, fidelity)),
    });
    req.metadata_mut()
        .insert(PLAYER_TOKEN_METADATA_KEY, token.parse()?);
    let resp = client.act(req).await?.into_inner();
    Ok(match resp.result {
        Some(act_response::Result::Error(e)) => Some(e),
        _ => None,
    })
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

/// Fetches the current full table status for the reconcile path. Returns
/// `Ok(None)` if the service reports no status (e.g. between hands).
async fn fetch_status(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
) -> Result<Option<TableStatus>, AgentError> {
    let resp = client
        .get_status(tonic::Request::new(GetStatusRequest {}))
        .await?
        .into_inner();
    Ok(resp.status)
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

/// Applies the runner's legality clamp to the agent's `intended` decision and
/// records the coercion in `fidelity` when the applied action differs.
///
/// Two clamps keep the dealer from rejecting an otherwise-reasonable action: a
/// `Raise` below the legal minimum is bumped up to `floor_raise`, and a preflop
/// `Bet` with nothing to call (the big blind's option) becomes a `Check`. When
/// either fires, `was_coerced` is set and the pre-clamp `intended_action` is
/// preserved (EPIC-25 Phase 4).
fn finalize_decision(
    intended: &Decision,
    fidelity: &mut AgentFidelity,
    floor_raise: u32,
    is_preflop: bool,
    amount_to_call: u32,
) -> Decision {
    let applied = match intended {
        Decision::Raise(n) if *n < floor_raise => Decision::Raise(floor_raise),
        Decision::Bet(_) if is_preflop && amount_to_call == 0 => Decision::Check,
        other => other.clone(),
    };
    if &applied != intended {
        fidelity.was_coerced = Some(true);
        fidelity.intended_action = Some(intended.clone());
    }
    applied
}

/// Maps a [`Decision`] to its proto `(action_type, amount)` pair. Amount is
/// meaningful only for `Bet`/`Raise`; other actions report `0`.
fn decision_parts(decision: &Decision) -> (ActionType, u32) {
    match decision {
        Decision::Fold => (ActionType::Fold, 0),
        Decision::Check => (ActionType::Check, 0),
        Decision::Call => (ActionType::Call, 0),
        Decision::Bet(n) => (ActionType::Bet, *n),
        Decision::Raise(n) => (ActionType::Raise, *n),
        Decision::AllIn => (ActionType::AllIn, 0),
    }
}

/// Maps core [`AgentFidelity`] provenance to the proto message, or `None` when
/// it carries nothing — so a bare, un-annotated action emits no `agent` block.
fn fidelity_to_proto(fidelity: &AgentFidelity) -> Option<ProtoAgentFidelity> {
    if fidelity == &AgentFidelity::default() {
        return None;
    }
    let (intended_action_type, intended_amount) = match fidelity.intended_action.as_ref() {
        Some(decision) => {
            let (action_type, amount) = decision_parts(decision);
            let intended_amount =
                matches!(decision, Decision::Bet(_) | Decision::Raise(_)).then_some(amount);
            (Some(action_type as i32), intended_amount)
        }
        None => (None, None),
    };
    Some(ProtoAgentFidelity {
        raw_response: fidelity.raw_response.clone(),
        was_coerced: fidelity.was_coerced,
        intended_action_type,
        intended_amount,
        input_tokens: fidelity.input_tokens,
        output_tokens: fidelity.output_tokens,
        model: fidelity.model.clone(),
        prompt: fidelity.prompt.clone(),
    })
}

fn decision_to_proto(seat: u8, decision: &Decision, fidelity: &AgentFidelity) -> ProtoAction {
    let (action_type, amount) = decision_parts(decision);
    ProtoAction {
        seat: u32::from(seat),
        action_type: action_type as i32,
        amount,
        agent: fidelity_to_proto(fidelity),
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
        let action = decision_to_proto(3, &Decision::Fold, &AgentFidelity::default());
        assert_eq!(action.seat, 3);
        assert_eq!(action.action_type, ActionType::Fold as i32);
        assert_eq!(action.amount, 0);
    }

    #[test]
    fn test_decision_to_proto_check() {
        let action = decision_to_proto(5, &Decision::Check, &AgentFidelity::default());
        assert_eq!(action.action_type, ActionType::Check as i32);
        assert_eq!(action.amount, 0);
    }

    #[test]
    fn test_decision_to_proto_call() {
        let action = decision_to_proto(2, &Decision::Call, &AgentFidelity::default());
        assert_eq!(action.action_type, ActionType::Call as i32);
        assert_eq!(action.amount, 0);
    }

    #[test]
    fn test_decision_to_proto_bet() {
        let action = decision_to_proto(1, &Decision::Bet(250), &AgentFidelity::default());
        assert_eq!(action.seat, 1);
        assert_eq!(action.action_type, ActionType::Bet as i32);
        assert_eq!(action.amount, 250);
    }

    #[test]
    fn test_decision_to_proto_raise() {
        let action = decision_to_proto(0, &Decision::Raise(400), &AgentFidelity::default());
        assert_eq!(action.action_type, ActionType::Raise as i32);
        assert_eq!(action.amount, 400);
    }

    #[test]
    fn test_decision_to_proto_all_in() {
        let action = decision_to_proto(7, &Decision::AllIn, &AgentFidelity::default());
        assert_eq!(action.action_type, ActionType::AllIn as i32);
        assert_eq!(action.amount, 0);
    }

    #[test]
    fn test_decision_to_proto_seat_encoded() {
        let action = decision_to_proto(8, &Decision::Fold, &AgentFidelity::default());
        assert_eq!(action.seat, 8);
    }

    // ── EPIC-25 Phase 4: agent-fidelity mapping + clamp coercion ───────────

    #[test]
    fn decision_to_proto_attaches_agent_when_present() {
        let fidelity = AgentFidelity {
            model: Some("rules-v1".to_string()),
            ..Default::default()
        };
        let action = decision_to_proto(2, &Decision::Call, &fidelity);
        assert_eq!(
            action.agent.and_then(|a| a.model).as_deref(),
            Some("rules-v1")
        );
    }

    #[test]
    fn decision_to_proto_no_agent_block_when_empty() {
        let action = decision_to_proto(2, &Decision::Call, &AgentFidelity::default());
        assert!(action.agent.is_none());
    }

    #[test]
    fn fidelity_to_proto_maps_all_fields_with_intended() {
        let fidelity = AgentFidelity {
            raw_response: Some("raise to 250".to_string()),
            was_coerced: Some(true),
            intended_action: Some(Decision::Raise(250)),
            input_tokens: Some(100),
            output_tokens: Some(5),
            model: Some("m".to_string()),
            prompt: Some("hero prompt".to_string()),
        };
        let Some(p) = fidelity_to_proto(&fidelity) else {
            panic!("expected a populated fidelity");
        };
        assert_eq!(p.raw_response.as_deref(), Some("raise to 250"));
        assert_eq!(p.was_coerced, Some(true));
        assert_eq!(p.intended_action_type, Some(ActionType::Raise as i32));
        assert_eq!(p.intended_amount, Some(250));
        assert_eq!(p.input_tokens, Some(100));
        assert_eq!(p.output_tokens, Some(5));
        assert_eq!(p.model.as_deref(), Some("m"));
        assert_eq!(p.prompt.as_deref(), Some("hero prompt"));
    }

    #[test]
    fn fidelity_to_proto_intended_fold_carries_no_amount() {
        let fidelity = AgentFidelity {
            intended_action: Some(Decision::Fold),
            ..Default::default()
        };
        let Some(p) = fidelity_to_proto(&fidelity) else {
            panic!("expected a populated fidelity");
        };
        assert_eq!(p.intended_action_type, Some(ActionType::Fold as i32));
        assert_eq!(p.intended_amount, None);
    }

    #[test]
    fn finalize_decision_clamps_sub_floor_raise_and_records_intent() {
        let mut fidelity = AgentFidelity::default();
        // Intended raise of 50 is below the 200 floor → bumped, coercion recorded.
        let applied = finalize_decision(&Decision::Raise(50), &mut fidelity, 200, false, 100);
        assert_eq!(applied, Decision::Raise(200));
        assert_eq!(fidelity.was_coerced, Some(true));
        assert_eq!(fidelity.intended_action, Some(Decision::Raise(50)));
    }

    #[test]
    fn finalize_decision_converts_preflop_bet_with_no_call_to_check() {
        let mut fidelity = AgentFidelity::default();
        let applied = finalize_decision(&Decision::Bet(300), &mut fidelity, 0, true, 0);
        assert_eq!(applied, Decision::Check);
        assert_eq!(fidelity.was_coerced, Some(true));
        assert_eq!(fidelity.intended_action, Some(Decision::Bet(300)));
    }

    #[test]
    fn finalize_decision_legal_action_is_untouched() {
        let mut fidelity = AgentFidelity::default();
        let applied = finalize_decision(&Decision::Call, &mut fidelity, 200, false, 100);
        assert_eq!(applied, Decision::Call);
        assert_eq!(fidelity, AgentFidelity::default()); // no coercion recorded
    }

    #[test]
    fn parse_delay_defaults_when_absent() {
        assert_eq!(
            parse_delay(None, Duration::from_secs(1)),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn parse_delay_reads_whole_seconds() {
        assert_eq!(
            parse_delay(Some("3"), Duration::from_secs(1)),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn parse_delay_reads_fractional_seconds() {
        assert_eq!(
            parse_delay(Some(" 0.5 "), Duration::from_secs(1)),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn parse_delay_zero_disables_the_pause() {
        assert_eq!(
            parse_delay(Some("0"), Duration::from_secs(1)),
            Duration::ZERO
        );
    }

    #[test]
    fn parse_delay_rejects_negative_and_garbage() {
        let default = Duration::from_secs(5);
        assert_eq!(parse_delay(Some("-2"), default), default);
        assert_eq!(parse_delay(Some("abc"), default), default);
        assert_eq!(parse_delay(Some(""), default), default);
        assert_eq!(parse_delay(Some("inf"), default), default);
    }
}
