#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

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
//! | Variable          | Default           | Purpose                    |
//! |-------------------|-------------------|----------------------------|
//! | `PKDEALER_ADDR`   | 127.0.0.1:50051   | gRPC listen address        |

use std::{
    env,
    net::SocketAddr,
    process,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use pkcore::casino::{
    dealer::{Dealer, DealerAction, DealerError},
    game::ForcedBets,
    player::Player,
};
use pkdealer_proto::dealer::{
    ActionResult, ActionType, ActRequest, ActResponse, AdvanceStreetRequest,
    AdvanceStreetResponse, EndHandRequest, EndHandResponse, EventType, GetBoardRequest,
    GetBoardResponse, GetChipsRequest, GetChipsResponse, GetEventLogRequest,
    GetEventLogResponse, GetNextToActRequest, GetNextToActResponse, GetPotRequest,
    GetPotResponse, GetStatusRequest, GetStatusResponse, HandResult, NextToActInfo,
    PingReply, PingRequest, PlayerChips, RemovePlayerRequest, RemovePlayerResponse,
    SeatInfo, SeatPlayerAtRequest, SeatPlayerAtResponse, SeatPlayerRequest,
    SeatPlayerResponse, StartHandRequest, StartHandResponse, StreamEventsRequest,
    StreetResult, TableEvent, TableStatus,
    act_response, advance_street_response, end_hand_response, get_next_to_act_response,
    remove_player_response, seat_player_at_response, seat_player_response,
    start_hand_response,
    dealer_service_server::{DealerService as DealerServiceTrait, DealerServiceServer},
};
use tokio::sync::broadcast;
use tonic::{Request, Response, Status, transport::Server};

const DEFAULT_SERVICE_ADDR: &str = "127.0.0.1:50051";
const DEFAULT_CHIPS: usize = 10_000;
const DEFAULT_SMALL_BLIND: usize = 50;
const DEFAULT_BIG_BLIND: usize = 100;
const DEFAULT_SEAT_COUNT: u8 = 9;
const EVENT_CHANNEL_CAPACITY: usize = 64;

// ── TableState ───────────────────────────────────────────────────────────────

/// Wraps [`Dealer`] for use behind an `Arc<Mutex<_>>`.
///
/// # Safety
///
/// [`Dealer`] (and its inner [`Table`]) use `Cell`/`RefCell` for interior
/// mutability, which makes them `!Send` by default. We guarantee that every
/// access to this struct goes through the `Mutex` in `DealerService`, so only
/// one thread ever touches the `Dealer` at a time. That invariant makes the
/// `unsafe impl Send` sound.
struct TableState {
    dealer: Dealer,
}

// SAFETY: see doc-comment above.
unsafe impl Send for TableState {}

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
        let dealer = Dealer::new(
            ForcedBets::new(DEFAULT_SMALL_BLIND, DEFAULT_BIG_BLIND),
            DEFAULT_SEAT_COUNT,
        );
        let state = Arc::new(Mutex::new(TableState { dealer }));
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        DealerService { state, event_tx }
    }

    /// Acquires the state lock and returns an error `Status` if the lock is
    /// poisoned.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, TableState>, Status> {
        self.state
            .lock()
            .map_err(|_| Status::internal("table state lock is poisoned"))
    }

    /// Builds a [`TableStatus`] snapshot from the current dealer state.
    fn build_table_status(dealer: &Dealer) -> TableStatus {
        let table = &dealer.table;
        let mut seats = Vec::new();

        for i in 0..table.seats.size() {
            if let Some(seat) = table.seats.get_seat(i) {
                if !seat.is_empty() {
                    seats.push(SeatInfo {
                        seat_number: u32::from(i),
                        player_name: seat.player.handle.clone(),
                        chips: seat.player.chips.count() as u32,
                        cards: seat.cards.to_string(),
                        state: format!("{:?}", seat.player.state.get()),
                    });
                }
            }
        }

        TableStatus {
            seats,
            board: table.board.to_string(),
            pot: table.pot.count() as u32,
            next_to_act: u32::from(table.next_to_act()),
            hand_in_progress: dealer.is_hand_in_progress(),
            game_over: table.is_game_over(),
        }
    }

    /// Builds a flat list of chip counts for all occupied seats.
    fn build_player_chips(dealer: &Dealer) -> Vec<PlayerChips> {
        let table = &dealer.table;
        let mut result = Vec::new();
        for i in 0..table.seats.size() {
            if let Some(seat) = table.seats.get_seat(i) {
                if !seat.is_empty() {
                    result.push(PlayerChips {
                        seat: u32::from(i),
                        player_name: seat.player.handle.clone(),
                        chips: seat.player.chips.count() as u32,
                    });
                }
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
    fn emit_event(
        &self,
        event_type: EventType,
        description: String,
        status: TableStatus,
    ) {
        let event = TableEvent {
            timestamp: Self::now_unix_ms(),
            event_type: event_type as i32,
            description,
            current_status: Some(status),
        };
        let _ = self.event_tx.send(event);
    }
}

// ── gRPC trait implementation ─────────────────────────────────────────────────

#[tonic::async_trait]
impl DealerServiceTrait for DealerService {
    // ── Ping ──────────────────────────────────────────────────────────────────

    /// Returns `"pong"` or `"pong:<client_id>"` to confirm the service is alive.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// // gRPC ping — no doc-runnable example needed here; see unit tests.
    /// ```
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
        let player = Player::new_with_chips(req.name, chips);

        let (response_result, maybe_event) = {
            let guard = self.lock()?;
            match guard.dealer.seat_player(player) {
                Ok(seat_num) => {
                    let status = Self::build_table_status(&guard.dealer);
                    let event = (
                        EventType::PlayerSeated,
                        format!("Player seated at seat {seat_num}"),
                        status,
                    );
                    (
                        seat_player_response::Result::SeatNumber(u32::from(seat_num)),
                        Some(event),
                    )
                }
                Err(e) => (
                    seat_player_response::Result::Error(dealer_error_to_string(&e)),
                    None,
                ),
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(SeatPlayerResponse {
            result: Some(response_result),
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
        let player = Player::new_with_chips(req.name, chips);

        let (response_result, maybe_event) = {
            let guard = self.lock()?;
            match guard.dealer.seat_player_at(player, seat_number) {
                Ok(()) => {
                    let status = Self::build_table_status(&guard.dealer);
                    let event = (
                        EventType::PlayerSeated,
                        format!("Player seated at seat {seat_number}"),
                        status,
                    );
                    (seat_player_at_response::Result::Success(true), Some(event))
                }
                Err(e) => (
                    seat_player_at_response::Result::Error(dealer_error_to_string(&e)),
                    None,
                ),
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(SeatPlayerAtResponse {
            result: Some(response_result),
        }))
    }

    async fn remove_player(
        &self,
        request: Request<RemovePlayerRequest>,
    ) -> Result<Response<RemovePlayerResponse>, Status> {
        #[allow(clippy::cast_possible_truncation)]
        let seat = request.into_inner().seat as u8;

        let (response_result, maybe_event) = {
            let guard = self.lock()?;
            // Guard against removing from an already-empty seat, since the
            // library does not return an error in that case.
            let is_empty = guard
                .dealer
                .table
                .seats
                .get_seat(seat)
                .map_or(true, |s| s.is_empty());
            if is_empty {
                let msg = format!("seat {seat} is empty or does not exist");
                return Ok(Response::new(RemovePlayerResponse {
                    result: Some(remove_player_response::Result::Error(msg)),
                }));
            }
            match guard.dealer.remove_player(seat) {
                Ok(player) => {
                    let name = player.handle.clone();
                    let status = Self::build_table_status(&guard.dealer);
                    let event = (
                        EventType::PlayerRemoved,
                        format!("Player '{name}' removed from seat {seat}"),
                        status,
                    );
                    (
                        remove_player_response::Result::PlayerName(name),
                        Some(event),
                    )
                }
                Err(e) => (
                    remove_player_response::Result::Error(dealer_error_to_string(&e)),
                    None,
                ),
            }
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
            match guard.dealer.start_hand() {
                Ok(()) => {
                    let status = Self::build_table_status(&guard.dealer);
                    let event = (EventType::HandStarted, "Hand started".to_owned(), status.clone());
                    (start_hand_response::Result::Status(status), Some(event))
                }
                Err(e) => (
                    start_hand_response::Result::Error(dealer_error_to_string(&e)),
                    None,
                ),
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
        let (response_result, maybe_event) = {
            let mut guard = self.lock()?;
            match guard.dealer.advance_street() {
                Ok(()) => {
                    let table = &guard.dealer.table;
                    let board = table.board.to_string();
                    let next_to_act = u32::from(table.next_to_act());
                    let pot = table.pot.count() as u32;

                    let street_result = StreetResult {
                        board: board.clone(),
                        next_to_act,
                        pot,
                    };
                    let status = Self::build_table_status(&guard.dealer);
                    let event = (
                        EventType::StreetAdvanced,
                        format!("Street advanced. Board: {board}"),
                        status,
                    );
                    (
                        advance_street_response::Result::StreetResult(street_result),
                        Some(event),
                    )
                }
                Err(e) => (
                    advance_street_response::Result::Error(dealer_error_to_string(&e)),
                    None,
                ),
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(AdvanceStreetResponse {
            result: Some(response_result),
        }))
    }

    async fn end_hand(
        &self,
        _request: Request<EndHandRequest>,
    ) -> Result<Response<EndHandResponse>, Status> {
        let (response_result, maybe_event) = {
            let mut guard = self.lock()?;
            match guard.dealer.end_hand() {
                Ok(winnings) => {
                    let result_text = winnings.to_string();
                    let final_chips = Self::build_player_chips(&guard.dealer);
                    let hand_result = HandResult {
                        result_text: result_text.clone(),
                        final_chips,
                    };
                    let status = Self::build_table_status(&guard.dealer);
                    let event = (
                        EventType::HandEnded,
                        format!("Hand ended. {result_text}"),
                        status,
                    );
                    (end_hand_response::Result::HandResult(hand_result), Some(event))
                }
                Err(e) => (
                    end_hand_response::Result::Error(dealer_error_to_string(&e)),
                    None,
                ),
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(EndHandResponse {
            result: Some(response_result),
        }))
    }

    // ── Player action ─────────────────────────────────────────────────────────

    async fn act(&self, request: Request<ActRequest>) -> Result<Response<ActResponse>, Status> {
        let req = request.into_inner();
        let proto_action = req
            .action
            .ok_or_else(|| Status::invalid_argument("missing action field"))?;

        #[allow(clippy::cast_possible_truncation)]
        let seat = proto_action.seat as u8;
        let amount = proto_action.amount as usize;
        let action_type = ActionType::try_from(proto_action.action_type)
            .map_err(|_| {
                Status::invalid_argument(format!(
                    "unknown action_type value: {}",
                    proto_action.action_type
                ))
            })?;

        let dealer_action = match action_type {
            ActionType::Bet => DealerAction::Bet { seat, amount },
            ActionType::Call => DealerAction::Call { seat },
            ActionType::Check => DealerAction::Check { seat },
            ActionType::Raise => DealerAction::Raise { seat, amount },
            ActionType::AllIn => DealerAction::AllIn { seat },
            ActionType::Fold => DealerAction::Fold { seat },
        };

        let (response_result, maybe_event) = {
            let guard = self.lock()?;
            match guard.dealer.act(dealer_action) {
                Ok(()) => {
                    let table = &guard.dealer.table;
                    let action_result = ActionResult {
                        next_to_act: u32::from(table.next_to_act()),
                        pot: table.pot.count() as u32,
                        hand_complete: table.is_game_over(),
                    };
                    let status = Self::build_table_status(&guard.dealer);
                    let event = (
                        EventType::PlayerAction,
                        format!("Seat {seat}: {action_type:?}"),
                        status,
                    );
                    (act_response::Result::ActionResult(action_result), Some(event))
                }
                Err(e) => (act_response::Result::Error(dealer_error_to_string(&e)), None),
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(ActResponse {
            result: Some(response_result),
        }))
    }

    // ── Read-only queries ─────────────────────────────────────────────────────

    async fn get_status(
        &self,
        _request: Request<GetStatusRequest>,
    ) -> Result<Response<GetStatusResponse>, Status> {
        let guard = self.lock()?;
        let status = Self::build_table_status(&guard.dealer);
        Ok(Response::new(GetStatusResponse {
            status: Some(status),
        }))
    }

    async fn get_next_to_act(
        &self,
        _request: Request<GetNextToActRequest>,
    ) -> Result<Response<GetNextToActResponse>, Status> {
        let guard = self.lock()?;

        if !guard.dealer.is_hand_in_progress() {
            return Ok(Response::new(GetNextToActResponse {
                result: Some(get_next_to_act_response::Result::Message(
                    "No hand in progress".to_owned(),
                )),
            }));
        }

        let seat_num = guard.dealer.next_to_act();
        let result = if let Some(seat) = guard.dealer.table.seats.get_seat(seat_num) {
            if seat.is_empty() {
                get_next_to_act_response::Result::Message("No active player to act".to_owned())
            } else {
                get_next_to_act_response::Result::Info(NextToActInfo {
                    seat: u32::from(seat_num),
                    player_name: seat.player.handle.clone(),
                    chips: seat.player.chips.count() as u32,
                    pot: guard.dealer.pot() as u32,
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
            board: guard.dealer.table.board.to_string(),
        }))
    }

    async fn get_chips(
        &self,
        _request: Request<GetChipsRequest>,
    ) -> Result<Response<GetChipsResponse>, Status> {
        let guard = self.lock()?;
        Ok(Response::new(GetChipsResponse {
            chips: Self::build_player_chips(&guard.dealer),
        }))
    }

    async fn get_pot(
        &self,
        _request: Request<GetPotRequest>,
    ) -> Result<Response<GetPotResponse>, Status> {
        let guard = self.lock()?;
        Ok(Response::new(GetPotResponse {
            pot: guard.dealer.pot() as u32,
        }))
    }

    async fn get_event_log(
        &self,
        _request: Request<GetEventLogRequest>,
    ) -> Result<Response<GetEventLogResponse>, Status> {
        let guard = self.lock()?;
        Ok(Response::new(GetEventLogResponse {
            log: guard.dealer.event_log().to_string(),
        }))
    }

    // ── Event stream ──────────────────────────────────────────────────────────

    type StreamEventsStream =
        tokio_stream::wrappers::ReceiverStream<Result<TableEvent, Status>>;

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

        Ok(Response::new(
            tokio_stream::wrappers::ReceiverStream::new(mpsc_rx),
        ))
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Converts a [`DealerError`] to a human-readable string for proto error fields.
fn dealer_error_to_string(e: &DealerError) -> String {
    e.to_string()
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
mod tests {
    use super::*;
    use pkdealer_proto::dealer::PlayerAction;

    fn make_service() -> DealerService {
        DealerService::new()
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
        let response = service.seat_player(request).await?;
        match response.into_inner().result {
            Some(seat_player_response::Result::SeatNumber(n)) => {
                assert!(n < DEFAULT_SEAT_COUNT as u32);
            }
            other => panic!("unexpected result: {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_seat_player_default_chips() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let request = Request::new(SeatPlayerRequest {
            name: "Bob".to_owned(),
            chips: 0, // should default to DEFAULT_CHIPS
        });
        let response = service.seat_player(request).await?;
        assert!(matches!(
            response.into_inner().result,
            Some(seat_player_response::Result::SeatNumber(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn dealer_service_seat_player_table_full() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        // Fill all 9 seats
        for i in 0..DEFAULT_SEAT_COUNT {
            let req = Request::new(SeatPlayerRequest {
                name: format!("Player{i}"),
                chips: 1_000,
            });
            service.seat_player(req).await?;
        }
        // One more should fail
        let req = Request::new(SeatPlayerRequest {
            name: "Extra".to_owned(),
            chips: 1_000,
        });
        let response = service.seat_player(req).await?;
        assert!(matches!(
            response.into_inner().result,
            Some(seat_player_response::Result::Error(_))
        ));
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
        let response = service.seat_player_at(request).await?;
        assert!(matches!(
            response.into_inner().result,
            Some(seat_player_at_response::Result::Success(true))
        ));
        Ok(())
    }

    // ── remove_player ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_remove_player_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        // Seat then remove
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

    // ── get_status ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_get_status_empty_table() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let response = service
            .get_status(Request::new(GetStatusRequest {}))
            .await?;
        let status = response.into_inner().status.expect("status should be present");
        assert!(status.seats.is_empty());
        assert!(!status.hand_in_progress);
        Ok(())
    }

    // ── start_hand ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_start_hand_not_enough_players() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        // Only one player — start_hand should fail
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
            }
            other => panic!("unexpected result: {other:?}"),
        }
        Ok(())
    }

    // ── act ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn dealer_service_act_fold_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let next_seat = {
            let guard = service.lock().expect("lock");
            guard.dealer.next_to_act()
        };

        let response = service
            .act(Request::new(ActRequest {
                action: Some(PlayerAction {
                    seat: u32::from(next_seat),
                    action_type: ActionType::Fold as i32,
                    amount: 0,
                }),
            }))
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
        let result = service
            .act(Request::new(ActRequest { action: None }))
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::InvalidArgument);
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
    async fn dealer_service_stream_events_receives_seat_event() -> Result<(), Box<dyn std::error::Error>> {
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

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Seats two players so tests that need a startable hand can call `start_hand`.
    async fn seat_two_players(service: &DealerService) -> Result<(), Box<dyn std::error::Error>> {
        service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Alice".to_owned(),
                chips: 1_000,
            }))
            .await?;
        service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Bob".to_owned(),
                chips: 1_000,
            }))
            .await?;
        Ok(())
    }
}
