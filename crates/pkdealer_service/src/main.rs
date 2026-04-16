#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::cast_possible_truncation)]

//! # Poker Dealer Service
//!
//! A gRPC service that manages a single poker table. Clients can seat players,
//! start hands, process actions, and stream live table events.
//!
//! ## Table defaults
//!
//! - 9 seats, No-Limit Hold'em
//! - Small blind: 50, Big blind: 100
//! - Default buy-in when `chips == 0`: 10,000
//!
//! ## Configuration
//!
//! | Variable                    | Default           | Purpose                              |
//! |-----------------------------|-------------------|--------------------------------------|
//! | `PKDEALER_ADDR`             | 127.0.0.1:50051   | gRPC listen address                  |
//! | `PKDEALER_SPECTATOR_TOKEN`  | `spectator`       | Shared secret for full-table card visibility |
//!
//! ## Authentication
//!
//! Players receive a UUID token in the `player_token` field of `SeatPlayerResponse`
//! or `SeatPlayerAtResponse`.  They must include it in every mutating RPC as the
//! `x-player-token` gRPC metadata value.
//!
//! - `Act` — requires a valid token matching the acting seat; returns
//!   `PERMISSION_DENIED` otherwise.
//! - `GetStatus` — with a valid player token returns that player's hole cards only;
//!   with the spectator token returns all hole cards; with no token returns no hole
//!   cards.

use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    process,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use pkcore::casino::{
    action::PlayerAction,
    game::ForcedBets,
    session::{PokerSession, SessionStep},
    table_no_cell::{PlayerNoCell, SeatNoCell, SeatsNoCell, TableNoCell},
};
use pkdealer_proto::dealer::{
    ActRequest, ActResponse, ActionResult, ActionType, AdvanceStreetRequest, AdvanceStreetResponse,
    EndHandRequest, EndHandResponse, EventType, GetBoardRequest, GetBoardResponse, GetChipsRequest,
    GetChipsResponse, GetEventLogRequest, GetEventLogResponse, GetNextToActRequest,
    GetNextToActResponse, GetPotRequest, GetPotResponse, GetStatusRequest, GetStatusResponse,
    NextToActInfo, PingReply, PingRequest, PlayerChips, RemovePlayerRequest, RemovePlayerResponse,
    SeatInfo, SeatPlayerAtRequest, SeatPlayerAtResponse, SeatPlayerRequest, SeatPlayerResponse,
    StartHandRequest, StartHandResponse, StreamEventsRequest, TableEvent, TableStatus,
    act_response, advance_street_response,
    dealer_service_server::{DealerService as DealerServiceTrait, DealerServiceServer},
    end_hand_response, get_next_to_act_response, remove_player_response, seat_player_at_response,
    seat_player_response, start_hand_response,
};
use tokio::sync::broadcast;
use tonic::{Request, Response, Status, metadata::MetadataMap, transport::Server};
use uuid::Uuid;

const DEFAULT_SERVICE_ADDR: &str = "127.0.0.1:50051";
const DEFAULT_CHIPS: usize = 10_000;
const DEFAULT_SMALL_BLIND: usize = 50;
const DEFAULT_BIG_BLIND: usize = 100;
const DEFAULT_SEAT_COUNT: u8 = 9;
const EVENT_CHANNEL_CAPACITY: usize = 64;
/// gRPC metadata key carrying the player's UUID auth token.
const PLAYER_TOKEN_METADATA_KEY: &str = "x-player-token";
/// Default spectator token used when `PKDEALER_SPECTATOR_TOKEN` is not set.
const DEFAULT_SPECTATOR_TOKEN: &str = "spectator";

// ── CardVisibility ────────────────────────────────────────────────────────────

/// Controls which hole cards appear in a [`TableStatus`] snapshot.
#[derive(Clone, Copy)]
enum CardVisibility {
    /// No hole cards are revealed — used for broadcast events and unauthenticated queries.
    Hidden,
    /// Only the given seat's hole cards are revealed — used for authenticated player queries.
    Player(u8),
    /// All hole cards are revealed — used for spectator / admin access.
    Spectator,
}

// ── TableState ───────────────────────────────────────────────────────────────

/// Wraps [`PokerSession`] and the player auth token maps for use behind an `Arc<Mutex<_>>`.
///
/// [`PokerSession`] wraps [`TableNoCell`], which has no `Cell`/`RefCell` interior
/// mutability, so it is `Send + Sync` without any unsafe code.
struct TableState {
    session: PokerSession,
    /// Maps player UUID tokens → seat numbers.
    token_to_seat: HashMap<Uuid, u8>,
    /// Maps seat numbers → player UUID tokens (for O(1) cleanup on `remove_player`).
    seat_to_token: HashMap<u8, Uuid>,
}

// ── DealerService ─────────────────────────────────────────────────────────────

/// gRPC service implementation for the poker dealer.
#[derive(Clone)]
struct DealerService {
    state: Arc<Mutex<TableState>>,
    event_tx: broadcast::Sender<TableEvent>,
}

impl DealerService {
    /// Creates a fresh table with default blind/seat configuration.
    fn new() -> Self {
        let seats = SeatsNoCell::new(
            (0..DEFAULT_SEAT_COUNT)
                .map(|_| SeatNoCell::default())
                .collect(),
        );
        let table = TableNoCell::nlh_from_seats(
            seats,
            ForcedBets::new(DEFAULT_SMALL_BLIND, DEFAULT_BIG_BLIND),
        );
        let session = PokerSession::new(table);
        let state = Arc::new(Mutex::new(TableState {
            session,
            token_to_seat: HashMap::new(),
            seat_to_token: HashMap::new(),
        }));
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        DealerService { state, event_tx }
    }

    /// Acquires the state lock and returns an error `Status` if the lock is poisoned.
    // `tonic::Status` is 176 bytes, but it is the mandatory error type for all
    // gRPC handlers in this crate.  Boxing it here would just push the
    // unboxing cost to every call site for no real benefit.
    #[allow(clippy::result_large_err)]
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, TableState>, Status> {
        self.state
            .lock()
            .map_err(|_| Status::internal("table state lock is poisoned"))
    }

    /// Builds a [`TableStatus`] snapshot from the current dealer state.
    ///
    /// The `visibility` parameter controls which hole cards are included:
    /// - [`CardVisibility::Hidden`] — `cards` is empty for every seat.
    /// - [`CardVisibility::Player`]`(seat)` — `cards` is populated only for `seat`.
    /// - [`CardVisibility::Spectator`] — `cards` is populated for every seat.
    fn build_table_status(session: &PokerSession, visibility: CardVisibility) -> TableStatus {
        let table = &session.table;
        let mut seats = Vec::new();

        for i in 0..table.seats.size() {
            if let Some(seat) = table.seats.get_seat(i)
                && !seat.is_empty()
            {
                let cards = match &visibility {
                    CardVisibility::Spectator => seat.cards.to_string(),
                    CardVisibility::Player(s) if *s == i => seat.cards.to_string(),
                    _ => String::new(),
                };
                seats.push(SeatInfo {
                    seat_number: u32::from(i),
                    player_name: seat.player.handle.clone(),
                    chips: seat.player.chips as u32,
                    cards,
                    state: format!("{:?}", seat.player.state),
                });
            }
        }

        TableStatus {
            seats,
            board: table.board.to_string(),
            pot: table.pot as u32,
            next_to_act: u32::from(table.next_to_act()),
            hand_in_progress: session.is_hand_in_progress(),
            game_over: table.is_game_over(),
        }
    }

    /// Builds a flat list of chip counts for all occupied seats.
    fn build_player_chips(session: &PokerSession) -> Vec<PlayerChips> {
        let table = &session.table;
        let mut result = Vec::new();
        for i in 0..table.seats.size() {
            if let Some(seat) = table.seats.get_seat(i)
                && !seat.is_empty()
            {
                result.push(PlayerChips {
                    seat: u32::from(i),
                    player_name: seat.player.handle.clone(),
                    chips: seat.player.chips as u32,
                });
            }
        }
        result
    }

    /// Returns the current UTC timestamp in milliseconds since the Unix epoch.
    fn now_unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Constructs and enqueues a [`TableEvent`] on the broadcast channel.
    ///
    /// Errors from `send` (no active subscribers) are silently discarded.
    fn emit_event(&self, event_type: EventType, description: String, status: TableStatus) {
        let event = TableEvent {
            timestamp: Self::now_unix_ms(),
            event_type: event_type as i32,
            description,
            current_status: Some(status),
        };
        let _ = self.event_tx.send(event);
    }

    /// Returns the spectator token, preferring the `PKDEALER_SPECTATOR_TOKEN`
    /// environment variable and falling back to [`DEFAULT_SPECTATOR_TOKEN`].
    fn spectator_token() -> String {
        env::var("PKDEALER_SPECTATOR_TOKEN").unwrap_or_else(|_| DEFAULT_SPECTATOR_TOKEN.to_owned())
    }

    /// Determines [`CardVisibility`] from the `x-player-token` gRPC metadata.
    ///
    /// - Spectator token → [`CardVisibility::Spectator`]
    /// - Valid player UUID → [`CardVisibility::Player`]`(seat)`
    /// - Missing or unrecognized token → [`CardVisibility::Hidden`]
    fn card_visibility_from_metadata(metadata: &MetadataMap, state: &TableState) -> CardVisibility {
        let Some(token_str) = metadata
            .get(PLAYER_TOKEN_METADATA_KEY)
            .and_then(|v| v.to_str().ok())
        else {
            return CardVisibility::Hidden;
        };

        if token_str == Self::spectator_token() {
            return CardVisibility::Spectator;
        }

        if let Ok(uuid) = token_str.parse::<Uuid>()
            && let Some(&seat) = state.token_to_seat.get(&uuid)
        {
            return CardVisibility::Player(seat);
        }

        CardVisibility::Hidden
    }
}

// ── gRPC trait implementation ─────────────────────────────────────────────────

#[tonic::async_trait]
#[allow(clippy::too_many_lines)] // tonic requires all RPCs in a single impl block
impl DealerServiceTrait for DealerService {
    // ── Ping ──────────────────────────────────────────────────────────────────

    /// Returns `"pong"` or `"pong:<client_id>"` to confirm the service is alive.
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingReply>, Status> {
        let client_id = request.into_inner().client_id;
        let message = if client_id.is_empty() {
            "pong".to_owned()
        } else {
            format!("pong:{client_id}")
        };
        Ok(Response::new(PingReply { message }))
    }

    // ── Seating ───────────────────────────────────────────────────────────────

    async fn seat_player(
        &self,
        request: Request<SeatPlayerRequest>,
    ) -> Result<Response<SeatPlayerResponse>, Status> {
        let req = request.into_inner();
        let chips = if req.chips == 0 {
            DEFAULT_CHIPS
        } else {
            req.chips as usize
        };

        let (response_result, player_token, maybe_event) = {
            let mut guard = self.lock()?;
            let size = guard.session.table.seats.size();
            let seat_num = (0..size).find(|&i| {
                guard
                    .session
                    .table
                    .seats
                    .get_seat(i)
                    .is_some_and(SeatNoCell::is_empty)
            });
            match seat_num {
                Some(i) => {
                    if let Some(s) = guard.session.table.seats.get_seat_mut(i) {
                        s.player = PlayerNoCell::new_with_chips(req.name.clone(), chips);
                    }
                    let token = Uuid::new_v4();
                    guard.token_to_seat.insert(token, i);
                    guard.seat_to_token.insert(i, token);
                    let status = Self::build_table_status(&guard.session, CardVisibility::Hidden);
                    let event = (
                        EventType::PlayerSeated,
                        format!("Player seated at seat {i}"),
                        status,
                    );
                    (
                        seat_player_response::Result::SeatNumber(u32::from(i)),
                        token.to_string(),
                        Some(event),
                    )
                }
                None => (
                    seat_player_response::Result::Error("no empty seat available".to_owned()),
                    String::new(),
                    None,
                ),
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(SeatPlayerResponse {
            result: Some(response_result),
            player_token,
        }))
    }

    async fn seat_player_at(
        &self,
        request: Request<SeatPlayerAtRequest>,
    ) -> Result<Response<SeatPlayerAtResponse>, Status> {
        let req = request.into_inner();
        let chips = if req.chips == 0 {
            DEFAULT_CHIPS
        } else {
            req.chips as usize
        };
        #[allow(clippy::cast_possible_truncation)]
        let seat_number = req.seat as u8;

        let (response_result, player_token, maybe_event) = {
            let mut guard = self.lock()?;
            let is_available = guard
                .session
                .table
                .seats
                .get_seat(seat_number)
                .is_some_and(SeatNoCell::is_empty);
            if is_available {
                if let Some(s) = guard.session.table.seats.get_seat_mut(seat_number) {
                    s.player = PlayerNoCell::new_with_chips(req.name.clone(), chips);
                }
                let token = Uuid::new_v4();
                guard.token_to_seat.insert(token, seat_number);
                guard.seat_to_token.insert(seat_number, token);
                let status = Self::build_table_status(&guard.session, CardVisibility::Hidden);
                let event = (
                    EventType::PlayerSeated,
                    format!("Player seated at seat {seat_number}"),
                    status,
                );
                (
                    seat_player_at_response::Result::Success(true),
                    token.to_string(),
                    Some(event),
                )
            } else {
                let msg = format!("seat {seat_number} is occupied or does not exist");
                (
                    seat_player_at_response::Result::Error(msg),
                    String::new(),
                    None,
                )
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(SeatPlayerAtResponse {
            result: Some(response_result),
            player_token,
        }))
    }

    async fn remove_player(
        &self,
        request: Request<RemovePlayerRequest>,
    ) -> Result<Response<RemovePlayerResponse>, Status> {
        #[allow(clippy::cast_possible_truncation)]
        let seat = request.into_inner().seat as u8;

        let (response_result, maybe_event) = {
            let mut guard = self.lock()?;
            let is_empty = guard
                .session
                .table
                .seats
                .get_seat(seat)
                .is_none_or(SeatNoCell::is_empty);
            if is_empty {
                let msg = format!("seat {seat} is empty or does not exist");
                return Ok(Response::new(RemovePlayerResponse {
                    result: Some(remove_player_response::Result::Error(msg)),
                }));
            }

            let name = guard
                .session
                .table
                .seats
                .get_seat_mut(seat)
                .map(|s| {
                    let n = s.player.handle.clone();
                    s.player = PlayerNoCell::default();
                    n
                })
                .unwrap_or_default();

            // Clean up the auth token for the removed seat.
            if let Some(uuid) = guard.seat_to_token.remove(&seat) {
                guard.token_to_seat.remove(&uuid);
            }

            let status = Self::build_table_status(&guard.session, CardVisibility::Hidden);
            let event = (
                EventType::PlayerRemoved,
                format!("Player '{name}' removed from seat {seat}"),
                status,
            );
            (
                remove_player_response::Result::PlayerName(name),
                Some(event),
            )
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(RemovePlayerResponse {
            result: Some(response_result),
        }))
    }

    // ── Hand lifecycle ────────────────────────────────────────────────────────

    async fn start_hand(
        &self,
        _request: Request<StartHandRequest>,
    ) -> Result<Response<StartHandResponse>, Status> {
        let (response_result, maybe_event) = {
            let mut guard = self.lock()?;
            if guard.session.count_funded() < 2 {
                return Ok(Response::new(StartHandResponse {
                    result: Some(start_hand_response::Result::Error(
                        "at least 2 players with chips are required to start a hand".to_owned(),
                    )),
                }));
            }
            match guard.session.start_hand() {
                Ok(()) => {
                    let status = Self::build_table_status(&guard.session, CardVisibility::Hidden);
                    let event = (
                        EventType::HandStarted,
                        "Hand started".to_owned(),
                        status.clone(),
                    );
                    (start_hand_response::Result::Status(status), Some(event))
                }
                Err(e) => (start_hand_response::Result::Error(e.to_string()), None),
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(StartHandResponse {
            result: Some(response_result),
        }))
    }

    async fn advance_street(
        &self,
        _request: Request<AdvanceStreetRequest>,
    ) -> Result<Response<AdvanceStreetResponse>, Status> {
        // Street advancement is now managed autonomously by the `act` handler
        // via `PokerSession::next_step()`. Calling this RPC is no longer needed.
        Ok(Response::new(AdvanceStreetResponse {
            result: Some(advance_street_response::Result::Error(
                "street advancement is managed autonomously; use Act".to_owned(),
            )),
        }))
    }

    async fn end_hand(
        &self,
        _request: Request<EndHandRequest>,
    ) -> Result<Response<EndHandResponse>, Status> {
        // Hand resolution is now managed autonomously by the `act` handler
        // via `PokerSession::next_step()`. Calling this RPC is no longer needed.
        Ok(Response::new(EndHandResponse {
            result: Some(end_hand_response::Result::Error(
                "hand resolution is managed autonomously; use Act".to_owned(),
            )),
        }))
    }

    // ── Player action ─────────────────────────────────────────────────────────

    async fn act(&self, request: Request<ActRequest>) -> Result<Response<ActResponse>, Status> {
        // Extract the player token from metadata before consuming the request.
        let token_str: Option<String> = request
            .metadata()
            .get(PLAYER_TOKEN_METADATA_KEY)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        let req = request.into_inner();
        let proto_action = req
            .action
            .ok_or_else(|| Status::invalid_argument("missing action field"))?;

        #[allow(clippy::cast_possible_truncation)]
        let seat = proto_action.seat as u8;
        let amount = proto_action.amount as usize;
        let action_type = ActionType::try_from(proto_action.action_type).map_err(|_| {
            Status::invalid_argument(format!(
                "unknown action_type value: {}",
                proto_action.action_type
            ))
        })?;

        // Verify the token authorizes this seat before acquiring the broader lock.
        {
            let guard = self.lock()?;
            match token_str.as_deref().and_then(|t| t.parse::<Uuid>().ok()) {
                Some(uuid) => match guard.token_to_seat.get(&uuid) {
                    Some(&token_seat) if token_seat == seat => {} // authorized
                    Some(&token_seat) => {
                        return Err(Status::permission_denied(format!(
                            "token belongs to seat {token_seat}, not seat {seat}"
                        )));
                    }
                    None => {
                        return Err(Status::permission_denied("unknown player token"));
                    }
                },
                None => {
                    return Err(Status::permission_denied(
                        "missing or invalid x-player-token metadata",
                    ));
                }
            }
        }

        let player_action = match action_type {
            ActionType::Bet => PlayerAction::Bet(amount),
            ActionType::Call => PlayerAction::Call,
            ActionType::Check => PlayerAction::Check,
            ActionType::Raise => PlayerAction::Raise(amount),
            ActionType::AllIn => PlayerAction::AllIn,
            ActionType::Fold => PlayerAction::Fold,
        };

        // Hold the lock for the full apply + advance loop to keep state atomic.
        // `emit_event` only sends on the broadcast channel — it never re-acquires
        // the state lock — so calling it while holding `guard` is safe.
        let mut guard = self.lock()?;

        match guard.session.apply_action(seat, player_action) {
            Ok(()) => {
                // Emit PlayerAction event for the triggering action.
                let status = Self::build_table_status(&guard.session, CardVisibility::Hidden);
                self.emit_event(
                    EventType::PlayerAction,
                    format!("Seat {seat}: {action_type:?}"),
                    status,
                );

                // Auto-advance: deal streets and/or end the hand as needed.
                let mut hand_complete = false;
                let mut next_to_act_seat = guard.session.table.next_to_act();

                loop {
                    match guard.session.next_step() {
                        SessionStep::PlayerToAct(s) => {
                            next_to_act_seat = s;
                            break;
                        }
                        SessionStep::StreetAdvanced => {
                            let board = guard.session.table.board.to_string();
                            let status =
                                Self::build_table_status(&guard.session, CardVisibility::Hidden);
                            self.emit_event(
                                EventType::StreetAdvanced,
                                format!("Street advanced. Board: {board}"),
                                status,
                            );
                        }
                        SessionStep::HandComplete => {
                            match guard.session.end_hand() {
                                Ok(winnings) => {
                                    hand_complete = true;
                                    let result_text = winnings.to_string();
                                    let status = Self::build_table_status(
                                        &guard.session,
                                        CardVisibility::Hidden,
                                    );
                                    self.emit_event(
                                        EventType::HandEnded,
                                        format!("Hand ended. {result_text}"),
                                        status,
                                    );
                                }
                                Err(e) => {
                                    return Ok(Response::new(ActResponse {
                                        result: Some(act_response::Result::Error(e.to_string())),
                                    }));
                                }
                            }
                            break;
                        }
                    }
                }

                let action_result = ActionResult {
                    next_to_act: u32::from(next_to_act_seat),
                    pot: guard.session.table.pot as u32,
                    hand_complete,
                };
                Ok(Response::new(ActResponse {
                    result: Some(act_response::Result::ActionResult(action_result)),
                }))
            }
            Err(e) => Ok(Response::new(ActResponse {
                result: Some(act_response::Result::Error(e.to_string())),
            })),
        }
    }

    // ── Read-only queries ─────────────────────────────────────────────────────

    async fn get_status(
        &self,
        request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let guard = self.lock()?;
        let visibility = Self::card_visibility_from_metadata(request.metadata(), &guard);
        let status = Self::build_table_status(&guard.session, visibility);
        Ok(Response::new(GetStatusResponse {
            status: Some(status),
        }))
    }

    async fn get_next_to_act(
        &self,
        _request: Request<GetNextToActRequest>,
    ) -> Result<Response<GetNextToActResponse>, Status> {
        let guard = self.lock()?;

        if !guard.session.is_hand_in_progress() {
            return Ok(Response::new(GetNextToActResponse {
                result: Some(get_next_to_act_response::Result::Message(
                    "No hand in progress".to_owned(),
                )),
            }));
        }

        let seat_num = guard.session.table.next_to_act();
        let result = if let Some(seat) = guard.session.table.seats.get_seat(seat_num) {
            if seat.is_empty() {
                get_next_to_act_response::Result::Message("No active player to act".to_owned())
            } else {
                get_next_to_act_response::Result::Info(NextToActInfo {
                    seat: u32::from(seat_num),
                    player_name: seat.player.handle.clone(),
                    chips: seat.player.chips as u32,
                    pot: guard.session.table.pot as u32,
                })
            }
        } else {
            get_next_to_act_response::Result::Message("Seat not found".to_owned())
        };

        Ok(Response::new(GetNextToActResponse {
            result: Some(result),
        }))
    }

    async fn get_board(
        &self,
        _request: Request<GetBoardRequest>,
    ) -> Result<Response<GetBoardResponse>, Status> {
        let guard = self.lock()?;
        Ok(Response::new(GetBoardResponse {
            board: guard.session.table.board.to_string(),
        }))
    }

    async fn get_chips(
        &self,
        _request: Request<GetChipsRequest>,
    ) -> Result<Response<GetChipsResponse>, Status> {
        let guard = self.lock()?;
        Ok(Response::new(GetChipsResponse {
            chips: Self::build_player_chips(&guard.session),
        }))
    }

    async fn get_pot(
        &self,
        _request: Request<GetPotRequest>,
    ) -> Result<Response<GetPotResponse>, Status> {
        let guard = self.lock()?;
        Ok(Response::new(GetPotResponse {
            pot: guard.session.table.pot as u32,
        }))
    }

    async fn get_event_log(
        &self,
        _request: Request<GetEventLogRequest>,
    ) -> Result<Response<GetEventLogResponse>, Status> {
        let guard = self.lock()?;
        let log = guard
            .session
            .table
            .event_log
            .iter()
            .enumerate()
            .map(|(i, a)| format!("{}: {a}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(Response::new(GetEventLogResponse { log }))
    }

    // ── Event stream ──────────────────────────────────────────────────────────

    type StreamEventsStream = tokio_stream::wrappers::ReceiverStream<Result<TableEvent, Status>>;

    async fn stream_events(
        &self,
        _request: Request<StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let mut broadcast_rx = self.event_tx.subscribe();
        let (mpsc_tx, mpsc_rx) = tokio::sync::mpsc::channel(EVENT_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            loop {
                match broadcast_rx.recv().await {
                    Ok(event) => {
                        if mpsc_tx.send(Ok(event)).await.is_err() {
                            // Client disconnected
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        // Warn but continue; the client will see a gap in events
                        let msg = format!("event stream lagged; {skipped} events skipped");
                        if mpsc_tx.send(Err(Status::data_loss(msg))).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Response::new(tokio_stream::wrappers::ReceiverStream::new(
            mpsc_rx,
        )))
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

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

    let service = DealerService::new();

    Server::builder()
        .add_service(DealerServiceServer::new(service))
        .serve(socket_addr)
        .await?;

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use pkdealer_proto::dealer::PlayerAction;

    fn make_service() -> DealerService {
        DealerService::new()
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Seats two players and returns a `seat → token` map for use in `act` calls.
    async fn seat_two_players(
        service: &DealerService,
    ) -> Result<HashMap<u8, String>, Box<dyn std::error::Error>> {
        let mut tokens: HashMap<u8, String> = HashMap::new();

        let r1 = service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Alice".to_owned(),
                chips: 1_000,
            }))
            .await?
            .into_inner();
        if let Some(seat_player_response::Result::SeatNumber(seat)) = r1.result {
            tokens.insert(seat as u8, r1.player_token);
        }

        let r2 = service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Bob".to_owned(),
                chips: 1_000,
            }))
            .await?
            .into_inner();
        if let Some(seat_player_response::Result::SeatNumber(seat)) = r2.result {
            tokens.insert(seat as u8, r2.player_token);
        }

        Ok(tokens)
    }

    /// Builds an `ActRequest` with the `x-player-token` metadata set.
    fn act_request_with_token(
        seat: u8,
        action_type: ActionType,
        tokens: &HashMap<u8, String>,
    ) -> Request<ActRequest> {
        let token = tokens.get(&seat).expect("token for seat");
        let mut req = Request::new(ActRequest {
            action: Some(PlayerAction {
                seat: u32::from(seat),
                action_type: action_type as i32,
                amount: 0,
            }),
        });
        req.metadata_mut().insert(
            PLAYER_TOKEN_METADATA_KEY,
            token.parse().expect("valid token"),
        );
        req
    }

    /// Dispatches `action_type` for whoever is currently next to act.
    async fn act_next(
        service: &DealerService,
        action_type: ActionType,
        tokens: &HashMap<u8, String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let seat = {
            let guard = service.lock().expect("lock");
            guard.session.table.next_to_act()
        };
        let response = service
            .act(act_request_with_token(seat, action_type, tokens))
            .await?;
        match response.into_inner().result {
            Some(act_response::Result::ActionResult(_)) => Ok(()),
            Some(act_response::Result::Error(e)) => Err(e.into()),
            None => Err("empty act response".into()),
        }
    }

    /// Folds on behalf of whoever is next to act.
    async fn fold_next_to_act(
        service: &DealerService,
        tokens: &HashMap<u8, String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        act_next(service, ActionType::Fold, tokens).await
    }

    /// Completes preflop betting for a two-player hand: UTG calls, BB checks.
    async fn complete_preflop_betting(
        service: &DealerService,
        tokens: &HashMap<u8, String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        act_next(service, ActionType::Call, tokens).await?;
        act_next(service, ActionType::Check, tokens).await?;
        Ok(())
    }

    // ── ping ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_ping_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let request = Request::new(PingRequest {
            client_id: "client-99".to_owned(),
        });
        let response = service.ping(request).await?;
        assert_eq!(response.into_inner().message, "pong:client-99");
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_ping_empty_client_id() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let request = Request::new(PingRequest {
            client_id: String::new(),
        });
        let response = service.ping(request).await?;
        assert_eq!(response.into_inner().message, "pong");
        Ok(())
    }

    // ── seat_player ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_seat_player_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let request = Request::new(SeatPlayerRequest {
            name: "Alice".to_owned(),
            chips: 1_000,
        });
        let inner = service.seat_player(request).await?.into_inner();
        match inner.result {
            Some(seat_player_response::Result::SeatNumber(n)) => {
                assert!(n < u32::from(DEFAULT_SEAT_COUNT));
            }
            other => panic!("unexpected result: {other:?}"),
        }
        // A UUID token must be issued on success.
        assert!(!inner.player_token.is_empty());
        assert!(inner.player_token.parse::<Uuid>().is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_seat_player_default_chips() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let request = Request::new(SeatPlayerRequest {
            name: "Bob".to_owned(),
            chips: 0, // should default to DEFAULT_CHIPS
        });
        let inner = service.seat_player(request).await?.into_inner();
        assert!(matches!(
            inner.result,
            Some(seat_player_response::Result::SeatNumber(_))
        ));
        assert!(!inner.player_token.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_seat_player_error_returns_empty_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        // Fill all 9 seats
        for i in 0..DEFAULT_SEAT_COUNT {
            let req = Request::new(SeatPlayerRequest {
                name: format!("Player{i}"),
                chips: 1_000,
            });
            service.seat_player(req).await?;
        }
        // One more should fail with an empty token.
        let inner = service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Extra".to_owned(),
                chips: 1_000,
            }))
            .await?
            .into_inner();
        assert!(matches!(
            inner.result,
            Some(seat_player_response::Result::Error(_))
        ));
        assert!(inner.player_token.is_empty());
        Ok(())
    }

    // ── seat_player_at ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_seat_player_at_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let request = Request::new(SeatPlayerAtRequest {
            seat: 3,
            name: "Carol".to_owned(),
            chips: 2_000,
        });
        let inner = service.seat_player_at(request).await?.into_inner();
        assert!(matches!(
            inner.result,
            Some(seat_player_at_response::Result::Success(true))
        ));
        assert!(!inner.player_token.is_empty());
        assert!(inner.player_token.parse::<Uuid>().is_ok());
        Ok(())
    }

    // ── remove_player ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_remove_player_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        service
            .seat_player_at(Request::new(SeatPlayerAtRequest {
                seat: 0,
                name: "Dave".to_owned(),
                chips: 1_000,
            }))
            .await?;
        let response = service
            .remove_player(Request::new(RemovePlayerRequest { seat: 0 }))
            .await?;
        match response.into_inner().result {
            Some(remove_player_response::Result::PlayerName(name)) => {
                assert_eq!(name, "Dave");
            }
            other => panic!("unexpected result: {other:?}"),
        }
        // Token must be cleaned up: the seat no longer holds a token.
        let guard = service.lock().expect("lock");
        assert!(!guard.seat_to_token.contains_key(&0u8));
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_remove_player_empty_seat() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let response = service
            .remove_player(Request::new(RemovePlayerRequest { seat: 5 }))
            .await?;
        assert!(matches!(
            response.into_inner().result,
            Some(remove_player_response::Result::Error(_))
        ));
        Ok(())
    }

    // ── get_status card visibility ────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_get_status_empty_table() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let response = service
            .get_status(Request::new(GetStatusRequest {}))
            .await?;
        let status = response
            .into_inner()
            .status
            .expect("status should be present");
        assert!(status.seats.is_empty());
        assert!(!status.hand_in_progress);
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_get_status_no_token_hides_all_cards()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        // No token — cards must be hidden.
        let status = service
            .get_status(Request::new(GetStatusRequest {}))
            .await?
            .into_inner()
            .status
            .expect("status present");
        for seat in &status.seats {
            assert!(
                seat.cards.is_empty(),
                "seat {} cards should be hidden without a token",
                seat.seat_number
            );
        }
        drop(tokens);
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_get_status_player_token_shows_own_cards_only()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        // Use the first seated player's token.
        let (&my_seat, my_token) = tokens.iter().next().expect("at least one token");

        let mut req = Request::new(GetStatusRequest {});
        req.metadata_mut().insert(
            PLAYER_TOKEN_METADATA_KEY,
            my_token.parse().expect("valid token"),
        );
        let status = service
            .get_status(req)
            .await?
            .into_inner()
            .status
            .expect("status present");

        for seat in &status.seats {
            if seat.seat_number == u32::from(my_seat) {
                assert!(
                    !seat.cards.is_empty(),
                    "own cards should be visible with player token"
                );
            } else {
                assert!(
                    seat.cards.is_empty(),
                    "opponent's cards must be hidden with player token"
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_get_status_spectator_token_shows_all_cards()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let spectator = DEFAULT_SPECTATOR_TOKEN;
        let mut req = Request::new(GetStatusRequest {});
        req.metadata_mut()
            .insert(PLAYER_TOKEN_METADATA_KEY, spectator.parse().expect("valid"));
        let status = service
            .get_status(req)
            .await?
            .into_inner()
            .status
            .expect("status present");

        for seat in &status.seats {
            assert!(
                !seat.cards.is_empty(),
                "spectator should see all cards, seat {} was empty",
                seat.seat_number
            );
        }
        drop(tokens);
        Ok(())
    }

    // ── start_hand ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_start_hand_not_enough_players() -> Result<(), Box<dyn std::error::Error>>
    {
        let service = make_service();
        service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Solo".to_owned(),
                chips: 1_000,
            }))
            .await?;
        let response = service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;
        assert!(matches!(
            response.into_inner().result,
            Some(start_hand_response::Result::Error(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_start_hand_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        seat_two_players(&service).await?;
        let response = service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;
        match response.into_inner().result {
            Some(start_hand_response::Result::Status(status)) => {
                assert!(status.hand_in_progress);
                // Cards are hidden in the start_hand response; players use
                // get_status with their token to see their own hole cards.
                for seat in &status.seats {
                    assert!(seat.cards.is_empty(), "start_hand response hides cards");
                }
            }
            other => panic!("unexpected result: {other:?}"),
        }
        Ok(())
    }

    // ── act ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_act_fold_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let next_seat = {
            let guard = service.lock().expect("lock");
            guard.session.table.next_to_act()
        };

        let response = service
            .act(act_request_with_token(next_seat, ActionType::Fold, &tokens))
            .await?;

        assert!(matches!(
            response.into_inner().result,
            Some(act_response::Result::ActionResult(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_act_missing_action_field() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        // The missing-action check happens before the token check, so no token
        // is needed here — we expect InvalidArgument regardless.
        let result = service.act(Request::new(ActRequest { action: None })).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_act_no_token_returns_permission_denied()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let next_seat = {
            let guard = service.lock().expect("lock");
            guard.session.table.next_to_act()
        };

        // Act without any token — must be rejected.
        let result = service
            .act(Request::new(ActRequest {
                action: Some(PlayerAction {
                    seat: u32::from(next_seat),
                    action_type: ActionType::Fold as i32,
                    amount: 0,
                }),
            }))
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
        drop(tokens);
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_act_wrong_seat_token_returns_permission_denied()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let next_seat = {
            let guard = service.lock().expect("lock");
            guard.session.table.next_to_act()
        };

        // Find the token that belongs to the *other* seat.
        let other_token = tokens
            .iter()
            .find(|&(&seat, _)| seat != next_seat)
            .map(|(_, token)| token.clone())
            .expect("other token");

        let mut req = Request::new(ActRequest {
            action: Some(PlayerAction {
                seat: u32::from(next_seat),
                action_type: ActionType::Fold as i32,
                amount: 0,
            }),
        });
        req.metadata_mut().insert(
            PLAYER_TOKEN_METADATA_KEY,
            other_token.parse().expect("valid"),
        );

        let result = service.act(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::PermissionDenied);
        Ok(())
    }

    // ── get_pot ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_get_pot_before_hand() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let response = service.get_pot(Request::new(GetPotRequest {})).await?;
        assert_eq!(response.into_inner().pot, 0);
        Ok(())
    }

    // ── get_board ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_get_board_before_hand() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let response = service.get_board(Request::new(GetBoardRequest {})).await?;
        let board = response.into_inner().board;
        // Board should be empty or very short (no community cards dealt yet)
        assert!(board.is_empty() || board.len() < 20);
        Ok(())
    }

    // ── get_next_to_act ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_get_next_to_act_no_hand() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let response = service
            .get_next_to_act(Request::new(GetNextToActRequest {}))
            .await?;
        assert!(matches!(
            response.into_inner().result,
            Some(get_next_to_act_response::Result::Message(_))
        ));
        Ok(())
    }

    // ── stream_events ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_stream_events_receives_seat_event()
    -> Result<(), Box<dyn std::error::Error>> {
        use tokio_stream::StreamExt;

        let service = make_service();
        let response = service
            .stream_events(Request::new(StreamEventsRequest {}))
            .await?;
        let mut stream = response.into_inner();

        // Seat a player — should trigger a broadcast event
        service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Eve".to_owned(),
                chips: 1_000,
            }))
            .await?;

        // Await the event with a short timeout
        let event = tokio::time::timeout(std::time::Duration::from_millis(200), stream.next())
            .await
            .expect("timeout waiting for event")
            .expect("stream ended")
            .expect("event error");

        assert_eq!(event.event_type, EventType::PlayerSeated as i32);
        Ok(())
    }

    // ── get_next_to_act with hand in progress ─────────────────────────────────

    #[tokio::test]
    async fn dealer_service_get_next_to_act_during_hand() -> Result<(), Box<dyn std::error::Error>>
    {
        let service = make_service();
        seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let response = service
            .get_next_to_act(Request::new(GetNextToActRequest {}))
            .await?;
        match response.into_inner().result {
            Some(get_next_to_act_response::Result::Info(info)) => {
                assert!(info.seat < u32::from(DEFAULT_SEAT_COUNT));
                assert!(!info.player_name.is_empty());
                assert!(info.chips > 0);
            }
            other => panic!("expected Info, got {other:?}"),
        }
        Ok(())
    }

    // ── get_chips ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_get_chips_with_players() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        seat_two_players(&service).await?;

        let chips = service
            .get_chips(Request::new(GetChipsRequest {}))
            .await?
            .into_inner()
            .chips;
        assert_eq!(chips.len(), 2);
        assert!(chips.iter().all(|p| p.chips == 1_000));
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_get_chips_after_blinds_posted() -> Result<(), Box<dyn std::error::Error>>
    {
        let service = make_service();
        seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let chips = service
            .get_chips(Request::new(GetChipsRequest {}))
            .await?
            .into_inner()
            .chips;
        // SB paid 50, BB paid 100 — chips in hand, not counted until pot is awarded
        let total: u32 = chips.iter().map(|p| p.chips).sum();
        assert_eq!(total, 1_850, "SB 950 + BB 900 = 1850 chips remaining");
        Ok(())
    }

    // ── get_event_log ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_get_event_log_grows_after_start_hand()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let log_before = service
            .get_event_log(Request::new(GetEventLogRequest {}))
            .await?
            .into_inner()
            .log;
        let lines_before = log_before.lines().count();

        seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let log_after = service
            .get_event_log(Request::new(GetEventLogRequest {}))
            .await?
            .into_inner()
            .log;
        let lines_after = log_after.lines().count();

        assert!(
            lines_after > lines_before,
            "log should grow after start_hand: before={lines_before}, after={lines_after}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_get_event_log_populated_after_start_hand()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let log = service
            .get_event_log(Request::new(GetEventLogRequest {}))
            .await?
            .into_inner()
            .log;
        assert!(
            !log.is_empty(),
            "event log should be populated after start_hand"
        );
        // Log lines are numbered; check there are at least a few entries
        let line_count = log.lines().count();
        assert!(line_count >= 3, "expected ≥3 log entries, got {line_count}");
        Ok(())
    }

    // ── end_hand (deprecated) ─────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_end_hand_returns_deprecated_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let response = service.end_hand(Request::new(EndHandRequest {})).await?;
        assert!(
            matches!(
                response.into_inner().result,
                Some(end_hand_response::Result::Error(_))
            ),
            "end_hand must return a deprecation error"
        );
        Ok(())
    }

    /// After a fold, `act` auto-ends the hand and chips are conserved.
    #[tokio::test]
    async fn dealer_service_fold_auto_ends_hand_chips_conserved()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        // Fold — the act handler auto-calls end_hand via next_step().
        let response = fold_next_to_act(&service, &tokens).await;
        // fold_next_to_act returns () on ActionResult; the hand is now over.
        assert!(response.is_ok(), "fold should succeed: {response:?}");

        let total: u32 = service
            .get_chips(Request::new(GetChipsRequest {}))
            .await?
            .into_inner()
            .chips
            .iter()
            .map(|p| p.chips)
            .sum();
        assert_eq!(total, 2_000, "chips must be conserved after auto-payout");
        Ok(())
    }

    // ── advance_street (deprecated) ───────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_advance_street_returns_deprecated_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;
        let response = service
            .advance_street(Request::new(AdvanceStreetRequest {}))
            .await?;
        assert!(
            matches!(
                response.into_inner().result,
                Some(advance_street_response::Result::Error(_))
            ),
            "advance_street must return a deprecation error"
        );
        Ok(())
    }

    // ── full hand sequence ────────────────────────────────────────────────────

    /// Plays a complete two-player hand via `Act` only (no `advance_street` or
    /// `end_hand`).  Verifies that:
    ///   - streets are auto-advanced by the `act` handler via `next_step()`
    ///   - `ActionResult.hand_complete` becomes `true` after the last river action
    ///   - total chips are conserved (no chips created or destroyed)
    #[tokio::test]
    async fn dealer_service_act_only_full_hand_chips_conserved()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        // Preflop: SB calls, BB checks — after BB checks, act auto-advances to flop.
        complete_preflop_betting(&service, &tokens).await?;

        // Post-preflop: check until hand_complete is signalled in ActionResult.
        let mut hand_complete = false;
        for _ in 0..(DEFAULT_SEAT_COUNT * 4) {
            // upper bound: 4 streets × 9 seats
            if hand_complete {
                break;
            }
            let seat = {
                let guard = service.lock().expect("lock");
                guard.session.table.next_to_act()
            };
            let resp = service
                .act(act_request_with_token(seat, ActionType::Check, &tokens))
                .await?
                .into_inner();
            match resp.result {
                Some(act_response::Result::ActionResult(r)) => {
                    hand_complete = r.hand_complete;
                }
                Some(act_response::Result::Error(e)) => return Err(e.into()),
                None => return Err("empty act response".into()),
            }
        }
        assert!(hand_complete, "hand should have completed by showdown");

        // Chips must be fully conserved.
        let total: u32 = service
            .get_chips(Request::new(GetChipsRequest {}))
            .await?
            .into_inner()
            .chips
            .iter()
            .map(|p| p.chips)
            .sum();
        assert_eq!(total, 2_000, "chips must be conserved through a full hand");

        Ok(())
    }

    // ── two-player interaction ────────────────────────────────────────────────

    /// Simulates two independent clients: each only knows its own seat and token.
    ///
    /// This mirrors real usage — a deployed client stores only the token issued
    /// to it and cannot act on behalf of any other seat.
    #[tokio::test]
    async fn dealer_service_two_players_each_know_only_own_token()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();

        // Player A seating — stores only its own token.
        let r_a = service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Alice".to_owned(),
                chips: 1_000,
            }))
            .await?
            .into_inner();
        let (seat_a, token_a) = match r_a.result {
            Some(seat_player_response::Result::SeatNumber(s)) => (s as u8, r_a.player_token),
            other => panic!("Alice seat failed: {other:?}"),
        };
        let _map_a: HashMap<u8, String> = HashMap::from([(seat_a, token_a.clone())]);

        // Player B seating — stores only its own token.
        let r_b = service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Bob".to_owned(),
                chips: 1_000,
            }))
            .await?
            .into_inner();
        let (seat_b, token_b) = match r_b.result {
            Some(seat_player_response::Result::SeatNumber(s)) => (s as u8, r_b.player_token),
            other => panic!("Bob seat failed: {other:?}"),
        };
        let _map_b: HashMap<u8, String> = HashMap::from([(seat_b, token_b)]);

        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        // Cards are dealt at start_hand time; no betting is required to assert
        // visibility — each player can already see (only) their own hole cards.

        // Each player can see their own hole cards via their token.
        let mut req_a = Request::new(GetStatusRequest {});
        req_a.metadata_mut().insert(
            PLAYER_TOKEN_METADATA_KEY,
            token_a.parse().expect("valid token"),
        );
        let status_a = service
            .get_status(req_a)
            .await?
            .into_inner()
            .status
            .expect("status");
        let seat_info_a = status_a
            .seats
            .iter()
            .find(|s| s.seat_number == u32::from(seat_a))
            .expect("Alice's seat in status");
        assert!(
            !seat_info_a.cards.is_empty(),
            "Alice should see her own hole cards"
        );
        let seat_info_b_from_a = status_a
            .seats
            .iter()
            .find(|s| s.seat_number == u32::from(seat_b))
            .expect("Bob's seat in Alice's status");
        assert!(
            seat_info_b_from_a.cards.is_empty(),
            "Alice must not see Bob's hole cards"
        );

        Ok(())
    }

    /// A player whose token is valid for their seat cannot act when it is not
    /// their turn.  Auth passes; the game engine rejects the out-of-turn action.
    ///
    /// This verifies the distinction between auth errors (`PermissionDenied` gRPC
    /// status) and game-state errors (Error variant in the result oneof).
    #[tokio::test]
    async fn dealer_service_act_for_own_seat_when_not_your_turn_is_game_error()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let next_seat = {
            let guard = service.lock().expect("lock");
            guard.session.table.next_to_act()
        };

        // Find the player who is NOT next to act.
        let &idle_seat = tokens.keys().find(|&&s| s != next_seat).expect("idle seat");

        let resp = service
            .act(act_request_with_token(idle_seat, ActionType::Fold, &tokens))
            .await?;

        // The request was authenticated (token matches seat), but the game engine
        // must reject the out-of-turn action — this is a domain error, not an
        // auth error, so it arrives as Ok(Response { result: Error(...) }).
        assert!(
            matches!(
                resp.into_inner().result,
                Some(act_response::Result::Error(_))
            ),
            "out-of-turn action must produce a game error, not a gRPC status error"
        );
        Ok(())
    }

    /// After `remove_player`, the seat's token is revoked.  Any subsequent `Act`
    /// with that token must return `PermissionDenied` even before a hand starts.
    #[tokio::test]
    async fn dealer_service_token_revoked_after_remove_player()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();

        let r = service
            .seat_player_at(Request::new(SeatPlayerAtRequest {
                seat: 4,
                name: "Dave".to_owned(),
                chips: 1_000,
            }))
            .await?
            .into_inner();
        let old_token = r.player_token;
        assert!(!old_token.is_empty());

        service
            .remove_player(Request::new(RemovePlayerRequest { seat: 4 }))
            .await?;

        // Auth runs before the game-engine check, so PermissionDenied is returned
        // immediately even though no hand is in progress.
        let mut req = Request::new(ActRequest {
            action: Some(PlayerAction {
                seat: 4,
                action_type: ActionType::Fold as i32,
                amount: 0,
            }),
        });
        req.metadata_mut()
            .insert(PLAYER_TOKEN_METADATA_KEY, old_token.parse().expect("valid"));

        let result = service.act(req).await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().code(),
            tonic::Code::PermissionDenied,
            "revoked token must not be accepted"
        );
        Ok(())
    }
}
