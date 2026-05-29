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
//! | Variable                          | Default           | Purpose                                                                 |
//! |-----------------------------------|-------------------|-------------------------------------------------------------------------|
//! | `PKDEALER_ADDR`                   | 127.0.0.1:50051   | gRPC listen address                                                     |
//! | `PKDEALER_SPECTATOR_TOKEN`        | `spectator`       | Shared secret for full-table card visibility                            |
//! | `PKDEALER_REBUY_AMOUNT`           | 10000             | Default chips granted when `Rebuy.chips == 0`                           |
//! | `PKDEALER_REBUY_ON_BUST_ENABLED`  | false             | Auto-reload busted seats at hand-end; allow `Rebuy` when `chips == 0`   |
//! | `PKDEALER_TOPUP_ENABLED`          | false             | Allow `Rebuy` for healthy stacks (between hands only)                   |
//!
//! For the browser spectator UI, run [`pkspectator`](https://github.com/ImperialBower/pkspectator)
//! as a separate process. It subscribes to this service via gRPC `StreamEvents`.
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

use pkdealer_service::otel;

use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    process,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use opentelemetry::KeyValue;
use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};
use opentelemetry::trace::TraceContextExt;
use pkcore::analysis::name::HandRankName;
use pkcore::casino::table::seats::seatbit::Seatbit;
use pkcore::casino::table::winnings::Winnings;
use pkcore::casino::{
    action::PlayerAction,
    game::ForcedBets,
    session::{PokerSession, SessionStep},
    state::PlayerState,
    table_no_cell::{PlayerNoCell, SeatNoCell, SeatsNoCell, TableNoCell},
};
use pkdealer_proto::dealer::{
    ActRequest, ActResponse, ActionResult, ActionType, EventType, GetBoardRequest,
    GetBoardResponse, GetChipsRequest, GetChipsResponse, GetEventLogRequest, GetEventLogResponse,
    GetNextToActRequest, GetNextToActResponse, GetPlayerStatsRequest, GetPlayerStatsResponse,
    GetPotRequest, GetPotResponse, GetStatusRequest, GetStatusResponse, GetTableConfigRequest,
    GetTableConfigResponse, HandResult, NextToActInfo, PingReply, PingRequest, PlayerChips,
    PlayerState as ProtoPlayerState, PlayerStats, RebuyInfo, RebuyRequest, RebuyResponse,
    RemovePlayerRequest, RemovePlayerResponse, SeatInfo, SeatPlayerAtRequest, SeatPlayerAtResponse,
    SeatPlayerRequest, SeatPlayerResponse, StartHandRequest, StartHandResponse,
    StreamEventsRequest, Street, TableConfig, TableEvent, TableStatus, WinnerInfo, act_response,
    dealer_service_server::{DealerService as DealerServiceTrait, DealerServiceServer},
    get_next_to_act_response, rebuy_response, remove_player_response, seat_player_at_response,
    seat_player_response, start_hand_response,
};
use tokio::sync::broadcast;
use tonic::{Request, Response, Status, metadata::MetadataMap, transport::Server};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use uuid::Uuid;

const DEFAULT_SERVICE_ADDR: &str = "127.0.0.1:50051";
const DEFAULT_CHIPS: usize = 10_000;
const DEFAULT_SMALL_BLIND: usize = 50;
const DEFAULT_BIG_BLIND: usize = 100;
const DEFAULT_SEAT_COUNT: u8 = 9;
// Capacity of both the broadcast channel and each subscriber's forwarding mpsc.
// Sized generously so a briefly-slow spectator does not lag past the buffer
// during a busy multi-agent demo; on overflow `stream_events` skips the gap and
// resyncs from the next event's full snapshot rather than dropping the stream.
const EVENT_CHANNEL_CAPACITY: usize = 1024;
/// gRPC metadata key carrying the player's UUID auth token.
const PLAYER_TOKEN_METADATA_KEY: &str = "x-player-token";
/// Default spectator token used when `PKDEALER_SPECTATOR_TOKEN` is not set.
const DEFAULT_SPECTATOR_TOKEN: &str = "spectator";
/// Default chip amount granted on a `Rebuy` call when `chips == 0`.
const DEFAULT_REBUY_AMOUNT: usize = 10_000;

// ── DealerConfig ──────────────────────────────────────────────────────────────

/// Service-level toggles for the rebuy / top-up feature.
///
/// Populated from environment variables in [`DealerConfig::from_env`] when the
/// service boots, and overridable in tests via [`DealerService::new_with_config`].
///
/// # Examples
///
/// ```ignore
/// // (private type — used internally by the binary)
/// let cfg = DealerConfig {
///     default_rebuy_amount: 500,
///     rebuy_on_bust_enabled: true,
///     topup_enabled: false,
/// };
/// assert_eq!(500, cfg.default_rebuy_amount);
/// ```
#[derive(Clone, Debug)]
struct DealerConfig {
    /// Fallback chip amount used when a `Rebuy` request specifies `chips == 0`.
    default_rebuy_amount: usize,
    /// When true, the service auto-reloads any seat that finished a hand with
    /// `chips == 0`, and `Rebuy` is allowed for seats with `chips == 0`.
    rebuy_on_bust_enabled: bool,
    /// When true, `Rebuy` is allowed for seats that still have chips
    /// (between hands only; mid-hand top-ups are always rejected).
    topup_enabled: bool,
}

impl DealerConfig {
    /// Reads the three rebuy env vars with safe fallbacks. Unparseable values
    /// silently fall back to defaults so a typo doesn't crash boot.
    fn from_env() -> Self {
        DealerConfig {
            default_rebuy_amount: env::var("PKDEALER_REBUY_AMOUNT")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(DEFAULT_REBUY_AMOUNT),
            rebuy_on_bust_enabled: parse_env_bool("PKDEALER_REBUY_ON_BUST_ENABLED"),
            topup_enabled: parse_env_bool("PKDEALER_TOPUP_ENABLED"),
        }
    }
}

impl Default for DealerConfig {
    fn default() -> Self {
        DealerConfig {
            default_rebuy_amount: DEFAULT_REBUY_AMOUNT,
            rebuy_on_bust_enabled: false,
            topup_enabled: false,
        }
    }
}

/// Parses an env var as a boolean. Recognizes `"true"`, `"1"`, `"yes"` (case
/// insensitive) as true; everything else (including unset) as false.
fn parse_env_bool(key: &str) -> bool {
    match env::var(key) {
        Ok(s) => {
            let lower = s.to_lowercase();
            matches!(lower.as_str(), "true" | "1" | "yes")
        }
        Err(_) => false,
    }
}

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
#[allow(dead_code)]
struct TableState {
    session: PokerSession,
    /// Maps player UUID tokens → seat numbers.
    token_to_seat: HashMap<Uuid, u8>,
    /// Maps seat numbers → player UUID tokens (for O(1) cleanup on `remove_player`).
    seat_to_token: HashMap<u8, Uuid>,
    /// Maps client-chosen secrets → player UUID tokens for seat resume.
    /// See `SeatPlayerRequest.client_secret`. Entries are removed when a
    /// seat is vacated via `remove_player`. Empty when no resume hints are
    /// in play.
    secret_to_token: HashMap<String, Uuid>,
    /// Open while a hand is in progress (`start_hand` → `HandComplete`). `None` between hands.
    current_hand_span: Option<tracing::Span>,
    /// Open for the current street; replaced on every `StreetAdvanced`; cleared on `HandComplete`.
    current_street_span: Option<tracing::Span>,
    /// Set when `start_hand` succeeds; used to compute total hand duration.
    hand_started_at: Option<std::time::Instant>,
    /// Set whenever the auto-advance loop decides the next actor.
    /// Difference against `now` at the top of `act` is `action_duration_ms`.
    last_prompt_at: Option<std::time::Instant>,
}

// ── Metrics ───────────────────────────────────────────────────────────────────

/// Six `OTel` instruments emitted by the service. Construction reads the
/// global meter provider, which `init_otel` configures with an OTLP
/// periodic exporter. In tests (where `init_otel` was never called) the
/// global meter is the no-op default, so instrument construction is a
/// silent no-op too.
///
/// `ai_decision_latency_ms` is **reserved for agent clients (`EPIC-23`)** —
/// the service does not record into it. Declared here so the dashboard
/// can reference a stable instrument name from day one.
#[derive(Debug)]
struct Metrics {
    hands_played: Counter<u64>,
    pot_size: Histogram<u64>,
    action_duration_ms: Histogram<f64>,
    #[allow(dead_code)]
    ai_decision_latency_ms: Histogram<f64>,
    /// Total rebuys (auto-on-bust + explicit RPC). Labels: `reason`, `seat`.
    rebuys_total: Counter<u64>,
    /// Per-seat cumulative profit/loss recorded after every completed hand.
    /// Labels: `seat`, `handle`.
    player_profit_loss: Gauge<i64>,
}

impl Metrics {
    fn new(meter: &Meter) -> Self {
        Self {
            hands_played: meter
                .u64_counter("pkdealer.hands_played")
                .with_description("Total hands completed")
                .build(),
            pot_size: meter
                .u64_histogram("pkdealer.pot_size")
                .with_description("Final pot size per hand")
                .with_unit("chips")
                .build(),
            action_duration_ms: meter
                .f64_histogram("pkdealer.action_duration_ms")
                .with_description("Time from next_actor prompt to act receipt")
                .with_unit("ms")
                .build(),
            ai_decision_latency_ms: meter
                .f64_histogram("pkdealer.ai_decision_latency_ms")
                .with_description("Agent-side decision latency")
                .with_unit("ms")
                .build(),
            rebuys_total: meter
                .u64_counter("pkdealer.rebuys_total")
                .with_description("Total chip reloads (auto-on-bust + explicit Rebuy)")
                .build(),
            player_profit_loss: meter
                .i64_gauge("pkdealer.player.profit_loss")
                .with_description("Per-seat cumulative profit/loss in chips")
                .with_unit("chips")
                .build(),
        }
    }
}

// ── DealerService ─────────────────────────────────────────────────────────────

/// gRPC service implementation for the poker dealer.
#[derive(Clone)]
struct DealerService {
    state: Arc<Mutex<TableState>>,
    event_tx: broadcast::Sender<TableEvent>,
    metrics: Arc<Metrics>,
    config: Arc<DealerConfig>,
}

impl DealerService {
    /// Creates a fresh table with default blind/seat configuration and
    /// rebuy/top-up settings sourced from environment variables.
    fn new() -> Self {
        Self::new_with_config(DealerConfig::from_env())
    }

    /// Creates a fresh table with explicit [`DealerConfig`]. Tests use this
    /// to avoid env-var races between parallel test cases.
    fn new_with_config(config: DealerConfig) -> Self {
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
            secret_to_token: HashMap::new(),
            current_hand_span: None,
            current_street_span: None,
            hand_started_at: None,
            last_prompt_at: None,
        }));
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let meter = opentelemetry::global::meter("pkdealer_service");
        let metrics = Arc::new(Metrics::new(&meter));
        DealerService {
            state,
            event_tx,
            metrics,
            config: Arc::new(config),
        }
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
                    state: Self::map_player_state(seat.player.state) as i32,
                    withdrawn: seat.player.withdrawn as u32,
                    chips_in_play: seat.player.chips_in_play as u32,
                    profit_loss: compute_profit_loss(&seat.player),
                    bet: seat.player.bet as u32,
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
            current_street: Self::map_game_phase_to_street(table) as i32,
        }
    }

    /// Returns a copy of `status` with `seat.cards` blanked according to `visibility`.
    ///
    /// - [`CardVisibility::Hidden`] → every seat's `cards` is cleared.
    /// - [`CardVisibility::Player`]`(s)` → only seat `s` keeps its cards.
    /// - [`CardVisibility::Spectator`] → returned unchanged.
    fn filter_cards(mut status: TableStatus, visibility: CardVisibility) -> TableStatus {
        match visibility {
            CardVisibility::Spectator => status,
            CardVisibility::Hidden => {
                for seat in &mut status.seats {
                    seat.cards.clear();
                }
                status
            }
            CardVisibility::Player(target) => {
                for seat in &mut status.seats {
                    if seat.seat_number != u32::from(target) {
                        seat.cards.clear();
                    }
                }
                status
            }
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

    /// Allocates a fresh seat for `SeatPlayerAt`, returning the response-tuple
    /// shape used by `seat_player_at`. Registers the player token and
    /// (when non-empty) the `client_secret → token` binding.
    fn fresh_seat_at_inner(
        state: &mut TableState,
        requested_seat: u8,
        name: &str,
        chips: usize,
        client_secret: &str,
    ) -> (
        seat_player_at_response::Result,
        String,
        bool,
        Option<(EventType, String, TableStatus)>,
    ) {
        let is_available = state
            .session
            .table
            .seats
            .get_seat(requested_seat)
            .is_some_and(SeatNoCell::is_empty);
        if !is_available {
            let msg = format!("seat {requested_seat} is occupied or does not exist");
            return (
                seat_player_at_response::Result::Error(msg),
                String::new(),
                false,
                None,
            );
        }
        if let Some(s) = state.session.table.seats.get_seat_mut(requested_seat) {
            s.player = PlayerNoCell::new_with_chips(name.to_owned(), chips);
        }
        let token = Uuid::new_v4();
        state.token_to_seat.insert(token, requested_seat);
        state.seat_to_token.insert(requested_seat, token);
        if !client_secret.is_empty() {
            state
                .secret_to_token
                .insert(client_secret.to_owned(), token);
        }
        let status = Self::build_table_status(&state.session, CardVisibility::Spectator);
        let event = (
            EventType::PlayerSeated,
            format!("Player seated at seat {requested_seat}"),
            status,
        );
        (
            seat_player_at_response::Result::Success(true),
            token.to_string(),
            false,
            Some(event),
        )
    }

    /// Maps a `pkcore` [`PlayerState`] variant to the corresponding proto [`ProtoPlayerState`].
    fn map_player_state(state: PlayerState) -> ProtoPlayerState {
        match state {
            PlayerState::Ready => ProtoPlayerState::Ready,
            PlayerState::YetToAct => ProtoPlayerState::YetToAct,
            PlayerState::Check => ProtoPlayerState::Checked,
            PlayerState::Blind(_) => ProtoPlayerState::Blind,
            PlayerState::Bet(_) => ProtoPlayerState::Bet,
            PlayerState::Call(_) => ProtoPlayerState::Called,
            PlayerState::Raise(_) | PlayerState::ReRaise(_) => ProtoPlayerState::Raised,
            PlayerState::AllIn(_) | PlayerState::Showdown(_) => ProtoPlayerState::AllIn,
            PlayerState::Fold => ProtoPlayerState::Folded,
            PlayerState::Out => ProtoPlayerState::Out,
        }
    }

    /// Maps the current game phase from [`TableNoCell`] to a proto [`Street`].
    fn map_game_phase_to_street(table: &TableNoCell) -> Street {
        if table.is_preflop() {
            Street::Preflop
        } else if table.is_flop() {
            Street::Flop
        } else if table.is_turn() {
            Street::Turn
        } else if table.is_river() {
            Street::River
        } else {
            Street::Unspecified
        }
    }

    /// Builds a structured [`HandResult`] from a completed [`Winnings`] value.
    ///
    /// Maps each `PotWin` to one [`WinnerInfo`] per winning seat. Split pots
    /// produce one entry per sharing seat with each receiving `chips / count`.
    fn build_hand_result(session: &PokerSession, winnings: &Winnings) -> HandResult {
        let table = &session.table;
        let mut winners = Vec::new();

        for pot_win in winnings.vec() {
            let seatbit = pot_win.equity.seats;
            let total_chips = pot_win.equity.chips;
            let (hand_description, winning_cards) = match pot_win.eval.hand_rank.name {
                HandRankName::Invalid => (String::new(), String::new()), // fold win
                ref name => (format!("{name:?}"), pot_win.eval.hand.to_string()),
            };

            let winning_seats: Vec<u8> = (0u8..Seatbit::CAPACITY)
                .filter(|&s| seatbit.contains(s))
                .collect();
            let per_seat = if winning_seats.is_empty() {
                0u32
            } else {
                (total_chips / winning_seats.len()) as u32
            };

            for seat_idx in winning_seats {
                let player_name = table
                    .seats
                    .get_seat(seat_idx)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.player.handle.clone())
                    .unwrap_or_default();
                winners.push(WinnerInfo {
                    seat: u32::from(seat_idx),
                    player_name,
                    amount_won: per_seat,
                    hand_description: hand_description.clone(),
                    winning_cards: winning_cards.clone(),
                });
            }
        }

        HandResult {
            winners,
            final_chips: Self::build_player_chips(session),
        }
    }

    /// Formats the human-readable `HandEnded` description from a [`HandResult`],
    /// naming each winner, the amount won, and (at showdown) the winning hand —
    /// e.g. `"Hand ended. gto wins 1500 with FullHouse (A♠ A♥ A♦ K♠ K♥)"`, or
    /// on a split pot `"Hand ended. gto wins 750, lag wins 750"`. Fold wins omit
    /// the hand; showdown wins append the winning five cards in parentheses.
    fn format_hand_end(result: &HandResult) -> String {
        if result.winners.is_empty() {
            return "Hand ended.".to_owned();
        }
        let parts: Vec<String> = result
            .winners
            .iter()
            .map(|w| {
                let who = if w.player_name.is_empty() {
                    format!("Seat {}", w.seat)
                } else {
                    w.player_name.clone()
                };
                if w.hand_description.is_empty() {
                    format!("{who} wins {}", w.amount_won)
                } else if w.winning_cards.is_empty() {
                    format!("{who} wins {} with {}", w.amount_won, w.hand_description)
                } else {
                    format!(
                        "{who} wins {} with {} ({})",
                        w.amount_won, w.hand_description, w.winning_cards
                    )
                }
            })
            .collect();
        format!("Hand ended. {}", parts.join(", "))
    }

    /// Returns the current UTC timestamp in milliseconds since the Unix epoch.
    fn now_unix_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }

    /// Auto-reloads any occupied seat whose `chips == 0` to the configured
    /// default rebuy amount, when `rebuy_on_bust_enabled` is true. Returns
    /// `(events_to_emit, seats_for_metrics)` — the caller emits events and
    /// records the `rebuys_total` counter outside this method. Mutates `state`
    /// in place.
    ///
    /// Trigger condition is `chips == 0` on an occupied seat (NOT
    /// `PlayerState::Out`, which pkcore only sets on empty seats — by the
    /// time this runs at end-of-hand, the seat's state is already
    /// `YetToAct`).
    fn run_auto_rebuy(
        &self,
        state: &mut TableState,
    ) -> (Vec<(EventType, String, TableStatus)>, Vec<u8>) {
        let mut events: Vec<(EventType, String, TableStatus)> = Vec::new();
        let mut labels: Vec<u8> = Vec::new();
        if !self.config.rebuy_on_bust_enabled {
            return (events, labels);
        }
        let reload_amount = self.config.default_rebuy_amount;
        let mut reloaded: Vec<(u8, String, usize)> = Vec::new();
        {
            let table = &mut state.session.table;
            let size = table.seats.size();
            for i in 0..size {
                if let Some(seat) = table.seats.get_seat_mut(i)
                    && !seat.is_empty()
                    && seat.player.chips == 0
                {
                    let new_total = seat.player.reload(reload_amount);
                    reloaded.push((i, seat.player.handle.clone(), new_total));
                }
            }
        }
        if reloaded.is_empty() {
            return (events, labels);
        }
        let status = Self::build_table_status(&state.session, CardVisibility::Spectator);
        for (seat_idx, handle, new_total) in reloaded {
            events.push((
                EventType::PlayerRebought,
                format!("Seat {seat_idx} ({handle}) auto-rebuy: +{reload_amount} → {new_total}"),
                status.clone(),
            ));
            labels.push(seat_idx);
        }
        (events, labels)
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

/// Computes a player's cumulative profit/loss as a signed `i32`.
///
/// Uses the invariant `profit = chips + chips_in_play - withdrawn` maintained
/// by `pkcore`. Values outside the `i32` range saturate at `i32::MIN` /
/// `i32::MAX` rather than panicking — `unwrap()` is forbidden by the project
/// lint set.
fn compute_profit_loss(player: &PlayerNoCell) -> i32 {
    let pl = i64::try_from(player.chips).unwrap_or(i64::MAX)
        + i64::try_from(player.chips_in_play).unwrap_or(i64::MAX)
        - i64::try_from(player.withdrawn).unwrap_or(i64::MAX);
    i32::try_from(pl).unwrap_or(if pl < 0 { i32::MIN } else { i32::MAX })
}

/// Returns a stable string label for the current street, suitable as a span
/// attribute. Uses the same phase-detection methods as
/// [`DealerService::map_game_phase_to_street`] but returns a fixed `&'static str`
/// so the telemetry vocabulary doesn't churn with proto bumps.
fn street_label(session: &PokerSession) -> &'static str {
    let table = &session.table;
    if table.is_preflop() {
        "preflop"
    } else if table.is_flop() {
        "flop"
    } else if table.is_turn() {
        "turn"
    } else if table.is_river() {
        "river"
    } else {
        "showdown"
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

    /// Seats a new player at the next available seat, OR re-attaches an
    /// existing seat if `client_secret` matches a previous call.
    ///
    /// # Resume contract
    ///
    /// If `request.client_secret` is non-empty and already bound to a live
    /// token, the response carries the original `seat_number` and
    /// `player_token` with `resumed = true`. The `name` and `chips` fields
    /// in the request are **ignored on the resume path** — the seat keeps
    /// its existing player handle and chip stack.
    ///
    /// Resume bindings are dropped automatically when the seat is removed
    /// via [`Self::remove_player`].
    ///
    /// # Errors
    ///
    /// Returns `Ok` with an error variant in the `result` oneof when no
    /// empty seat is available and no resume binding matched. The handler
    /// itself does not return `tonic::Status::Err`.
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

        let (response_result, player_token, resumed, maybe_event) = {
            let mut guard = self.lock()?;

            // Resume path: secret already bound to a token → return existing seat.
            if !req.client_secret.is_empty()
                && let Some(&token) = guard.secret_to_token.get(&req.client_secret)
                && let Some(&seat) = guard.token_to_seat.get(&token)
            {
                (
                    seat_player_response::Result::SeatNumber(u32::from(seat)),
                    token.to_string(),
                    true,
                    None,
                )
            } else {
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
                        if !req.client_secret.is_empty() {
                            guard
                                .secret_to_token
                                .insert(req.client_secret.clone(), token);
                        }
                        let status =
                            Self::build_table_status(&guard.session, CardVisibility::Spectator);
                        let event = (
                            EventType::PlayerSeated,
                            format!("Player seated at seat {i}"),
                            status,
                        );
                        (
                            seat_player_response::Result::SeatNumber(u32::from(i)),
                            token.to_string(),
                            false,
                            Some(event),
                        )
                    }
                    None => (
                        seat_player_response::Result::Error("no empty seat available".to_owned()),
                        String::new(),
                        false,
                        None,
                    ),
                }
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(SeatPlayerResponse {
            result: Some(response_result),
            player_token,
            resumed,
        }))
    }

    /// Seats a new player at a specific seat, OR re-attaches if
    /// `client_secret` matches a previous call to either seat-player RPC.
    ///
    /// # Resume contract
    ///
    /// Same as [`Self::seat_player`], with one extra constraint: the
    /// requested `seat` must equal the seat the secret was originally
    /// bound to. Mismatch returns an error in the response (not a
    /// `tonic::Status`); fresh-seat allocation is not attempted in the
    /// mismatch case so the caller learns about the conflict.
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
        let requested_seat = req.seat as u8;

        let (response_result, player_token, resumed, maybe_event) = {
            let mut guard = self.lock()?;

            // Resume path: secret bound to a token → require its seat to match.
            if !req.client_secret.is_empty()
                && let Some(&token) = guard.secret_to_token.get(&req.client_secret)
            {
                if let Some(&bound_seat) = guard.token_to_seat.get(&token) {
                    if bound_seat == requested_seat {
                        (
                            seat_player_at_response::Result::Success(true),
                            token.to_string(),
                            true,
                            None,
                        )
                    } else {
                        (
                            seat_player_at_response::Result::Error(format!(
                                "client_secret already bound to seat {bound_seat}; \
                                 requested seat {requested_seat} mismatch",
                            )),
                            String::new(),
                            false,
                            None,
                        )
                    }
                } else {
                    // Secret known but token no longer maps to a seat — stale entry.
                    // Drop it and fall through to fresh-seat allocation.
                    guard.secret_to_token.remove(&req.client_secret);
                    Self::fresh_seat_at_inner(
                        &mut guard,
                        requested_seat,
                        &req.name,
                        chips,
                        &req.client_secret,
                    )
                }
            } else {
                Self::fresh_seat_at_inner(
                    &mut guard,
                    requested_seat,
                    &req.name,
                    chips,
                    &req.client_secret,
                )
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(SeatPlayerAtResponse {
            result: Some(response_result),
            player_token,
            resumed,
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

            // Clean up the auth token AND any resume binding for the removed seat.
            if let Some(uuid) = guard.seat_to_token.remove(&seat) {
                guard.token_to_seat.remove(&uuid);
                guard.secret_to_token.retain(|_, t| *t != uuid);
            }

            let status = Self::build_table_status(&guard.session, CardVisibility::Spectator);
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
                // Genuine table-empty condition: fewer than two seats can post.
                // Logged at warn so a stalled demo is diagnosable from service
                // logs rather than appearing as a silent freeze (agents swallow
                // this error in `try_start_hand`).
                tracing::warn!(
                    funded = guard.session.count_funded(),
                    "start_hand refused: fewer than 2 funded players"
                );
                return Ok(Response::new(StartHandResponse {
                    result: Some(start_hand_response::Result::Error(
                        "at least 2 players with chips are required to start a hand".to_owned(),
                    )),
                }));
            }
            match guard.session.start_hand() {
                Ok(()) => {
                    // Open the hand span for the full hand lifecycle.
                    let hand_id = uuid::Uuid::new_v4();
                    let span = tracing::info_span!(
                        "hand",
                        hand_id          = %hand_id,
                        player_count     = guard.session.count_funded(),
                        starting_pot     = guard.session.table.pot,
                        final_pot        = tracing::field::Empty,
                        hand_duration_ms = tracing::field::Empty,
                    );
                    guard.current_hand_span = Some(span);
                    guard.current_street_span = None;
                    guard.hand_started_at = Some(std::time::Instant::now());
                    guard.last_prompt_at = Some(std::time::Instant::now());

                    // Broadcast events carry full-visibility snapshots; per-subscriber
                    // filtering happens in `stream_events`.  The unauthenticated
                    // `StartHandResponse` itself must hide hole cards.
                    let event_status =
                        Self::build_table_status(&guard.session, CardVisibility::Spectator);
                    let response_status =
                        Self::filter_cards(event_status.clone(), CardVisibility::Hidden);
                    let event = (
                        EventType::HandStarted,
                        "Hand started".to_owned(),
                        event_status,
                    );
                    (
                        start_hand_response::Result::Status(response_status),
                        Some(event),
                    )
                }
                Err(e) => {
                    // Distinguish the benign multi-agent race (a hand is already
                    // running — every agent calls start_hand after HandEnded, only
                    // one wins) from a real refusal that wedges the table, e.g.
                    // pkcore's "Insufficient chips Error" when an occupied seat sits
                    // at 0 chips and rebuy-on-bust is disabled. The latter is a
                    // demo-killer and must be visible in logs.
                    if guard.session.is_hand_in_progress() {
                        tracing::debug!(error = %e, "start_hand ignored: hand already in progress");
                    } else {
                        tracing::warn!(
                            error = %e,
                            "start_hand refused while table idle — table may be wedged \
                             (enable PKDEALER_REBUY_ON_BUST_ENABLED to top up busted seats)"
                        );
                    }
                    (start_hand_response::Result::Error(e.to_string()), None)
                }
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(StartHandResponse {
            result: Some(response_result),
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

        // Extract W3C trace context from incoming gRPC metadata. When no
        // `traceparent` header is present (current state — no EPIC-23 agents
        // yet), this returns an empty context that produces an invalid span
        // context, signalling the service-internal-parent fallback below.
        let parent_cx = opentelemetry::global::get_text_map_propagator(|p| {
            p.extract(&otel::MetadataExtractor(request.metadata()))
        });

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

        // Compute action latency: time from the most recent NextActor prompt
        // to this `act` arrival. Recorded as f64 ms with attributes.
        let action_latency_ms: Option<f64> = {
            let guard = self.lock()?;
            guard
                .last_prompt_at
                .map(|t| t.elapsed().as_secs_f64() * 1000.0)
        };

        let player_action = match action_type {
            ActionType::Unspecified => {
                return Err(Status::invalid_argument(
                    "action_type must not be UNSPECIFIED",
                ));
            }
            ActionType::Bet => PlayerAction::Bet(amount),
            ActionType::Call => PlayerAction::Call,
            ActionType::Check => PlayerAction::Check,
            ActionType::Raise => PlayerAction::Raise(amount),
            ActionType::AllIn => PlayerAction::AllIn,
            ActionType::Fold => PlayerAction::Fold,
        };

        if let Some(ms) = action_latency_ms {
            self.metrics.action_duration_ms.record(
                ms,
                &[
                    KeyValue::new("action_type", format!("{action_type:?}")),
                    KeyValue::new("seat", i64::from(seat)),
                ],
            );
        }

        // Hold the lock for the full apply + advance loop to keep state atomic.
        // `emit_event` only sends on the broadcast channel — it never re-acquires
        // the state lock — so calling it while holding `guard` is safe.
        let mut guard = self.lock()?;

        // Open the action span before mutating session state, so the span
        // covers the entire apply + auto-advance work. Parent selection:
        //   - traceparent present (agent) -> parent = remote ctx; record
        //     `linked_hand_trace` for cross-reference to in-process tree.
        //   - traceparent absent           -> parent = current_street_span
        //     (or current_hand_span as final fallback).
        let action_span = if parent_cx.span().span_context().is_valid() {
            let span = tracing::info_span!(
                "action",
                seat = seat,
                action_type = tracing::field::Empty,
                amount = tracing::field::Empty,
                pot_after = tracing::field::Empty,
                linked_hand_trace = tracing::field::Empty,
            );
            span.set_parent(parent_cx.clone());
            // If there's an in-process hand span open, record its trace_id
            // as a field so debuggers can cross-reference.
            if let Some(hand) = guard.current_hand_span.as_ref() {
                let sc = hand.context().span().span_context().clone();
                if sc.is_valid() {
                    span.record("linked_hand_trace", sc.trace_id().to_string().as_str());
                }
            }
            span
        } else {
            let parent = guard
                .current_street_span
                .as_ref()
                .or(guard.current_hand_span.as_ref());
            if let Some(parent_span) = parent {
                tracing::info_span!(
                    parent: parent_span,
                    "action",
                    seat              = seat,
                    action_type       = tracing::field::Empty,
                    amount            = tracing::field::Empty,
                    pot_after         = tracing::field::Empty,
                    linked_hand_trace = tracing::field::Empty,
                )
            } else {
                tracing::info_span!(
                    "action",
                    seat = seat,
                    action_type = tracing::field::Empty,
                    amount = tracing::field::Empty,
                    pot_after = tracing::field::Empty,
                    linked_hand_trace = tracing::field::Empty,
                )
            }
        };
        let _action_guard = action_span.enter();

        match guard.session.apply_action(seat, player_action) {
            Ok(()) => {
                // Record action attributes now that the action has been accepted.
                action_span.record("action_type", format!("{action_type:?}").as_str());
                action_span.record("amount", i64::try_from(amount).unwrap_or(i64::MAX));
                action_span.record(
                    "pot_after",
                    i64::try_from(guard.session.table.pot).unwrap_or(i64::MAX),
                );

                // Emit PlayerAction event for the triggering action. Include the
                // acting player's name (looked up from the snapshot we just
                // built) so log lines read "Seat 2 gto: Call" rather than bare
                // "Seat 2: Call". Falls back to no name if the seat is unnamed.
                let status = Self::build_table_status(&guard.session, CardVisibility::Spectator);
                let actor = status
                    .seats
                    .iter()
                    .find(|s| s.seat_number == u32::from(seat))
                    .filter(|s| !s.player_name.is_empty())
                    .map_or_else(String::new, |s| format!(" {}", s.player_name));
                self.emit_event(
                    EventType::PlayerAction,
                    format!("Seat {seat}{actor}: {action_type:?}"),
                    status,
                );

                // Auto-advance: deal streets and/or end the hand as needed.
                let mut hand_complete = false;
                let mut hand_result: Option<HandResult> = None;
                let mut next_to_act_seat = guard.session.table.next_to_act();

                loop {
                    match guard.session.next_step() {
                        SessionStep::PlayerToAct(s) => {
                            next_to_act_seat = s;
                            guard.last_prompt_at = Some(std::time::Instant::now());
                            break;
                        }
                        SessionStep::StreetAdvanced => {
                            // Close prior street span (if any) and open a fresh one
                            // parented to the current hand span.
                            guard.current_street_span = None;

                            let board = guard.session.table.board.to_string();
                            let street_name = street_label(&guard.session);
                            let street_span = if let Some(ref hand_span) = guard.current_hand_span {
                                tracing::info_span!(
                                    parent: hand_span,
                                    "street",
                                    street_name = %street_name,
                                    board_cards = %board,
                                )
                            } else {
                                tracing::info_span!(
                                    "street",
                                    street_name = %street_name,
                                    board_cards = %board,
                                )
                            };
                            guard.current_street_span = Some(street_span);

                            let status =
                                Self::build_table_status(&guard.session, CardVisibility::Spectator);
                            self.emit_event(
                                EventType::StreetAdvanced,
                                format!("Street advanced. Board: {board}"),
                                status,
                            );
                        }
                        SessionStep::HandComplete => {
                            // `end_hand()` calls `TableNoCell::reset()` which zeroes
                            // `table.pot` before returning. Snapshot pot + duration
                            // BEFORE the call so the metric and span attribute see
                            // the real final values.
                            let final_pot = guard.session.table.pot;
                            let hand_duration_ms = guard
                                .hand_started_at
                                .map(|t| t.elapsed().as_secs_f64() * 1000.0);

                            match guard.session.end_hand() {
                                Ok(winnings) => {
                                    hand_complete = true;
                                    let result = Self::build_hand_result(&guard.session, &winnings);
                                    // Describe the result by winner name(s) rather
                                    // than the raw seat/chip `Winnings` dump, so the
                                    // event log reads "Hand ended. gto wins 1500…".
                                    let desc = Self::format_hand_end(&result);
                                    hand_result = Some(result);
                                    let status = Self::build_table_status(
                                        &guard.session,
                                        CardVisibility::Spectator,
                                    );
                                    self.emit_event(EventType::HandEnded, desc, status);
                                    self.metrics.hands_played.add(1, &[]);
                                    self.metrics.pot_size.record(final_pot as u64, &[]);

                                    // Auto-rebuy any busted seats when the flag is on. Trigger
                                    // condition is `chips == 0` on an occupied seat (NOT
                                    // PlayerState::Out, which pkcore only sets on empty seats —
                                    // end_hand has already reset state to YetToAct here).
                                    let (rebuy_events, rebuy_labels) =
                                        self.run_auto_rebuy(&mut guard);

                                    // Record per-seat cumulative profit/loss gauge for every
                                    // occupied seat (uses post-rebuy state, which is invariant
                                    // under reload anyway).
                                    {
                                        let table = &guard.session.table;
                                        for i in 0..table.seats.size() {
                                            if let Some(seat) = table.seats.get_seat(i)
                                                && !seat.is_empty()
                                            {
                                                let pl =
                                                    i64::from(compute_profit_loss(&seat.player));
                                                self.metrics.player_profit_loss.record(
                                                    pl,
                                                    &[
                                                        KeyValue::new("seat", i64::from(i)),
                                                        KeyValue::new(
                                                            "handle",
                                                            seat.player.handle.clone(),
                                                        ),
                                                    ],
                                                );
                                            }
                                        }
                                    }

                                    // Emit any auto-rebuy events + counters now (still under the
                                    // lock — `emit_event` and metric recording are non-blocking).
                                    for (et, desc, status) in rebuy_events {
                                        self.emit_event(et, desc, status);
                                    }
                                    {
                                        for seat_idx in rebuy_labels {
                                            self.metrics.rebuys_total.add(
                                                1,
                                                &[
                                                    KeyValue::new("reason", "bust"),
                                                    KeyValue::new("seat", i64::from(seat_idx)),
                                                ],
                                            );
                                        }
                                    }

                                    // Record the captured values on the closing hand span.
                                    if let Some(hand_span) = guard.current_hand_span.as_ref() {
                                        hand_span.record(
                                            "final_pot",
                                            i64::try_from(final_pot).unwrap_or(i64::MAX),
                                        );
                                        if let Some(ms) = hand_duration_ms {
                                            hand_span.record("hand_duration_ms", ms);
                                        }
                                    }

                                    // Tear down the hand span and timing state.
                                    let _ = guard.current_street_span.take();
                                    let _ = guard.current_hand_span.take();
                                    guard.hand_started_at = None;
                                    guard.last_prompt_at = None;
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
                    hand_result,
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

        let table = &guard.session.table;
        let seat_num = table.next_to_act();
        let result = if let Some(seat) = table.seats.get_seat(seat_num) {
            if seat.is_empty() {
                get_next_to_act_response::Result::Message("No active player to act".to_owned())
            } else {
                get_next_to_act_response::Result::Info(NextToActInfo {
                    seat: u32::from(seat_num),
                    player_name: seat.player.handle.clone(),
                    chips: seat.player.chips as u32,
                    pot: table.pot as u32,
                    amount_to_call: table.to_call(seat_num) as u32,
                    min_raise: table.min_raise() as u32,
                    current_bet: table.bet as u32,
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

    // ── Table config ──────────────────────────────────────────────────────────

    async fn get_table_config(
        &self,
        _request: Request<GetTableConfigRequest>,
    ) -> Result<Response<GetTableConfigResponse>, Status> {
        Ok(Response::new(GetTableConfigResponse {
            config: Some(TableConfig {
                seat_count: u32::from(DEFAULT_SEAT_COUNT),
                small_blind: DEFAULT_SMALL_BLIND as u32,
                big_blind: DEFAULT_BIG_BLIND as u32,
                variant: "No-Limit Hold'em".to_owned(),
                default_chips: DEFAULT_CHIPS as u32,
                default_rebuy_amount: self.config.default_rebuy_amount as u32,
                rebuy_on_bust_enabled: self.config.rebuy_on_bust_enabled,
                topup_enabled: self.config.topup_enabled,
            }),
        }))
    }

    // ── Rebuy ─────────────────────────────────────────────────────────────────

    /// Adds chips to the caller's seat.
    ///
    /// Auth: requires a valid `x-player-token` metadata value bound to a seat.
    ///
    /// Flag gating (based on the seat's *current* chips, not its `state`):
    /// - `chips == 0` (busted) → requires `rebuy_on_bust_enabled` AND the
    ///   table must not have a hand in progress (an all-in busted seat has
    ///   `chips_in_play > 0`; reloading mid-hand would corrupt accounting).
    /// - `chips  > 0` (top-up) → requires `topup_enabled` AND the table must
    ///   not have a hand in progress (mid-hand top-ups would corrupt
    ///   `chips_in_play` accounting).
    ///
    /// Amount: `request.chips == 0` falls back to `config.default_rebuy_amount`.
    ///
    /// On success, both `player.chips` and `player.withdrawn` increase by the
    /// reload amount (pkcore invariant `profit = chips + chips_in_play - withdrawn`
    /// is preserved). After a bust, `end_hand` already reset the seat's state
    /// to `YetToAct`, so no manual state fixup is needed — the seat is ready
    /// for the next `StartHand`.
    async fn rebuy(
        &self,
        request: Request<RebuyRequest>,
    ) -> Result<Response<RebuyResponse>, Status> {
        // Resolve the auth token before consuming the request body.
        let token_str: Option<String> = request
            .metadata()
            .get(PLAYER_TOKEN_METADATA_KEY)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let Some(uuid) = token_str.as_deref().and_then(|t| t.parse::<Uuid>().ok()) else {
            return Err(Status::permission_denied(
                "missing or invalid x-player-token metadata",
            ));
        };

        let req = request.into_inner();
        let requested = req.chips as usize;
        let amount = if requested == 0 {
            self.config.default_rebuy_amount
        } else {
            requested
        };

        let span = tracing::info_span!(
            "rebuy",
            seat = tracing::field::Empty,
            reason = tracing::field::Empty,
            amount = i64::try_from(amount).unwrap_or(i64::MAX),
        );
        let _enter = span.enter();

        let (response_result, maybe_event, recorded_reason, recorded_seat) = {
            let mut guard = self.lock()?;

            let Some(&seat_idx) = guard.token_to_seat.get(&uuid) else {
                return Err(Status::permission_denied("unknown player token"));
            };

            let hand_in_progress = guard.session.is_hand_in_progress();

            let Some(seat) = guard.session.table.seats.get_seat_mut(seat_idx) else {
                return Err(Status::internal("token mapped to missing seat"));
            };

            let reason: &'static str = if seat.player.chips == 0 {
                if !self.config.rebuy_on_bust_enabled {
                    return Ok(Response::new(RebuyResponse {
                        result: Some(rebuy_response::Result::Error(
                            "rebuy-on-bust is disabled".to_owned(),
                        )),
                    }));
                }
                if hand_in_progress {
                    // An all-in player can have chips == 0 while chips_in_play > 0.
                    // Reloading mid-hand would corrupt the same invariant the
                    // top-up branch guards against.
                    return Ok(Response::new(RebuyResponse {
                        result: Some(rebuy_response::Result::Error(
                            "cannot rebuy during a hand".to_owned(),
                        )),
                    }));
                }
                "bust"
            } else {
                if !self.config.topup_enabled {
                    return Ok(Response::new(RebuyResponse {
                        result: Some(rebuy_response::Result::Error(
                            "top-up is disabled".to_owned(),
                        )),
                    }));
                }
                if hand_in_progress {
                    return Ok(Response::new(RebuyResponse {
                        result: Some(rebuy_response::Result::Error(
                            "cannot top up during a hand".to_owned(),
                        )),
                    }));
                }
                "topup"
            };

            let new_chips = seat.player.reload(amount);
            let new_withdrawn = seat.player.withdrawn;
            span.record("seat", i64::from(seat_idx));
            span.record("reason", reason);

            let status = Self::build_table_status(&guard.session, CardVisibility::Spectator);
            let info = RebuyInfo {
                seat: u32::from(seat_idx),
                new_chips: new_chips as u32,
                new_withdrawn: new_withdrawn as u32,
                reason: reason.to_owned(),
            };
            let event = (
                EventType::PlayerRebought,
                format!("Seat {seat_idx} {reason}: +{amount} → {new_chips}"),
                status,
            );
            (
                rebuy_response::Result::Info(info),
                Some(event),
                reason,
                seat_idx,
            )
        };

        // Lock is dropped at this point; emit event + metric outside it.
        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }
        self.metrics.rebuys_total.add(
            1,
            &[
                KeyValue::new("reason", recorded_reason),
                KeyValue::new("seat", i64::from(recorded_seat)),
            ],
        );

        Ok(Response::new(RebuyResponse {
            result: Some(response_result),
        }))
    }

    // ── GetPlayerStats ────────────────────────────────────────────────────────

    /// Returns per-seat aggregates (`chips`, `withdrawn`, `chips_in_play`, profit/loss)
    /// for every occupied seat. Useful for tracking cumulative win/loss across
    /// long bot sessions. Profit/loss is computed via the pkcore invariant
    /// `profit = chips + chips_in_play - withdrawn`.
    async fn get_player_stats(
        &self,
        _request: Request<GetPlayerStatsRequest>,
    ) -> Result<Response<GetPlayerStatsResponse>, Status> {
        let guard = self.lock()?;
        let table = &guard.session.table;
        let mut stats = Vec::new();
        for i in 0..table.seats.size() {
            if let Some(seat) = table.seats.get_seat(i)
                && !seat.is_empty()
            {
                stats.push(PlayerStats {
                    seat: u32::from(i),
                    player_name: seat.player.handle.clone(),
                    chips: seat.player.chips as u32,
                    chips_in_play: seat.player.chips_in_play as u32,
                    withdrawn: seat.player.withdrawn as u32,
                    profit_loss: compute_profit_loss(&seat.player),
                });
            }
        }
        Ok(Response::new(GetPlayerStatsResponse { stats }))
    }

    // ── Event stream ──────────────────────────────────────────────────────────

    type StreamEventsStream = tokio_stream::wrappers::ReceiverStream<Result<TableEvent, Status>>;

    async fn stream_events(
        &self,
        request: Request<StreamEventsRequest>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        // Resolve per-subscriber card visibility from the token.  Broadcast events
        // carry full-visibility snapshots; the bridge below filters each one before
        // forwarding it to this subscriber.
        let token_str = request.into_inner().player_token;
        let visibility = {
            let guard = self.lock()?;
            if token_str == Self::spectator_token() {
                CardVisibility::Spectator
            } else if let Ok(uuid) = token_str.parse::<Uuid>()
                && let Some(&seat) = guard.token_to_seat.get(&uuid)
            {
                CardVisibility::Player(seat)
            } else {
                CardVisibility::Hidden
            }
        };

        let mut broadcast_rx = self.event_tx.subscribe();
        let (mpsc_tx, mpsc_rx) = tokio::sync::mpsc::channel(EVENT_CHANNEL_CAPACITY);

        tokio::spawn(async move {
            loop {
                match broadcast_rx.recv().await {
                    Ok(mut event) => {
                        if let Some(status) = event.current_status.take() {
                            event.current_status = Some(Self::filter_cards(status, visibility));
                        }
                        if mpsc_tx.send(Ok(event)).await.is_err() {
                            // Client disconnected
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        // A slow subscriber (e.g. the spectator UI) fell behind the
                        // broadcast buffer. Do NOT propagate an Err here: yielding
                        // Err(Status) terminates the gRPC stream, which froze the
                        // spectator mid-hand once it lagged past the channel
                        // capacity. Every TableEvent carries a full `current_status`
                        // snapshot, so simply skipping the gap is self-healing — the
                        // next delivered event resyncs the client.
                        tracing::warn!(
                            skipped,
                            "event stream lagged; skipping gap and resyncing on next event"
                        );
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
    let addr = if let Ok(port) = env::var("PKDEALER_PORT") {
        format!("127.0.0.1:{port}")
    } else {
        env::var("PKDEALER_ADDR").unwrap_or_else(|_| DEFAULT_SERVICE_ADDR.to_owned())
    };
    run(&addr).await
}

async fn run(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    // Install OpenTelemetry tracing + metrics. Held for the lifetime of
    // main so OtelGuards::drop flushes batched exports at shutdown.
    let _otel_guards = match otel::init_otel() {
        Ok(guards) => guards,
        Err(err) => {
            eprintln!(
                "warning: OpenTelemetry initialisation failed ({err}); \
                 continuing without telemetry"
            );
            None
        }
    };

    let socket_addr: SocketAddr = addr.parse()?;

    println!("Poker Dealer Service v{}", env!("CARGO_PKG_VERSION"));
    println!("Starting gRPC server on {socket_addr}...");

    let service = DealerService::new();

    // gRPC reflection so `grpcurl` (and other dynamic clients) can
    // introspect the API without a local copy of the .proto file.
    // Register both v1 and v1alpha — different grpcurl versions probe
    // different paths first.
    let reflection_v1 = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(pkdealer_proto::DEALER_FILE_DESCRIPTOR_SET)
        .build_v1()?;
    let reflection_v1alpha = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(pkdealer_proto::DEALER_FILE_DESCRIPTOR_SET)
        .build_v1alpha()?;

    Server::builder()
        .add_service(DealerServiceServer::new(service))
        .add_service(reflection_v1)
        .add_service(reflection_v1alpha)
        .serve(socket_addr)
        .await
        .map_err(Into::into)
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

    #[test]
    fn format_hand_end_names_single_showdown_winner_with_cards() {
        let result = HandResult {
            winners: vec![WinnerInfo {
                seat: 2,
                player_name: "gto".to_owned(),
                amount_won: 1_500,
                hand_description: "FullHouse".to_owned(),
                winning_cards: "A♠ A♥ A♦ K♠ K♥".to_owned(),
            }],
            final_chips: Vec::new(),
        };
        assert_eq!(
            DealerService::format_hand_end(&result),
            "Hand ended. gto wins 1500 with FullHouse (A♠ A♥ A♦ K♠ K♥)"
        );
    }

    #[test]
    fn format_hand_end_fold_win_omits_hand_and_cards() {
        let result = HandResult {
            winners: vec![WinnerInfo {
                seat: 0,
                player_name: "lag".to_owned(),
                amount_won: 300,
                hand_description: String::new(),
                winning_cards: String::new(),
            }],
            final_chips: Vec::new(),
        };
        assert_eq!(
            DealerService::format_hand_end(&result),
            "Hand ended. lag wins 300"
        );
    }

    #[test]
    fn format_hand_end_split_pot_lists_each_winner_with_cards() {
        let result = HandResult {
            winners: vec![
                WinnerInfo {
                    seat: 1,
                    player_name: "gto".to_owned(),
                    amount_won: 750,
                    hand_description: "Flush".to_owned(),
                    winning_cards: "A♠ K♠ Q♠ J♠ T♠".to_owned(),
                },
                WinnerInfo {
                    seat: 4,
                    player_name: "tag".to_owned(),
                    amount_won: 750,
                    hand_description: "Flush".to_owned(),
                    winning_cards: "A♠ K♠ Q♠ J♠ T♠".to_owned(),
                },
            ],
            final_chips: Vec::new(),
        };
        assert_eq!(
            DealerService::format_hand_end(&result),
            "Hand ended. gto wins 750 with Flush (A♠ K♠ Q♠ J♠ T♠), \
             tag wins 750 with Flush (A♠ K♠ Q♠ J♠ T♠)"
        );
    }

    #[test]
    fn format_hand_end_unnamed_seat_falls_back_to_seat_number() {
        let result = HandResult {
            winners: vec![WinnerInfo {
                seat: 5,
                player_name: String::new(),
                amount_won: 200,
                hand_description: String::new(),
                winning_cards: String::new(),
            }],
            final_chips: Vec::new(),
        };
        assert_eq!(
            DealerService::format_hand_end(&result),
            "Hand ended. Seat 5 wins 200"
        );
    }

    #[test]
    fn format_hand_end_with_rank_but_no_cards_omits_parens() {
        // Defensive: a hand_description without winning_cards should not render
        // empty parentheses.
        let result = HandResult {
            winners: vec![WinnerInfo {
                seat: 1,
                player_name: "gto".to_owned(),
                amount_won: 400,
                hand_description: "Pair".to_owned(),
                winning_cards: String::new(),
            }],
            final_chips: Vec::new(),
        };
        assert_eq!(
            DealerService::format_hand_end(&result),
            "Hand ended. gto wins 400 with Pair"
        );
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
                client_secret: String::new(),
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
                client_secret: String::new(),
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
            client_secret: String::new(),
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
            client_secret: String::new(),
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
                client_secret: String::new(),
            });
            service.seat_player(req).await?;
        }
        // One more should fail with an empty token.
        let inner = service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Extra".to_owned(),
                chips: 1_000,
                client_secret: String::new(),
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
            client_secret: String::new(),
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
                client_secret: String::new(),
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
                client_secret: String::new(),
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
            .stream_events(Request::new(StreamEventsRequest {
                player_token: String::new(),
            }))
            .await?;
        let mut stream = response.into_inner();

        // Seat a player — should trigger a broadcast event
        service
            .seat_player(Request::new(SeatPlayerRequest {
                name: "Eve".to_owned(),
                chips: 1_000,
                client_secret: String::new(),
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

    /// A subscriber holding the spectator token must see hole cards on every
    /// seated player in the broadcast snapshot for `HandStarted`.
    #[tokio::test]
    async fn dealer_service_stream_events_spectator_token_sees_all_cards()
    -> Result<(), Box<dyn std::error::Error>> {
        use tokio_stream::StreamExt;

        let service = make_service();

        // Subscribe with the spectator token before any state changes so we
        // capture every event that follows.
        let response = service
            .stream_events(Request::new(StreamEventsRequest {
                player_token: DEFAULT_SPECTATOR_TOKEN.to_owned(),
            }))
            .await?;
        let mut stream = response.into_inner();

        seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        // Drain events until we see HandStarted, then assert visibility.
        let mut saw_hand_started = false;
        for _ in 0..6 {
            let event = tokio::time::timeout(std::time::Duration::from_millis(200), stream.next())
                .await
                .expect("timeout waiting for event")
                .expect("stream ended")
                .expect("event error");
            if event.event_type == EventType::HandStarted as i32 {
                let status = event.current_status.expect("HandStarted carries status");
                assert!(
                    !status.seats.is_empty(),
                    "HandStarted snapshot has at least one seat"
                );
                for seat in &status.seats {
                    assert!(
                        !seat.cards.is_empty(),
                        "spectator must see cards for seat {}",
                        seat.seat_number
                    );
                }
                saw_hand_started = true;
                break;
            }
        }
        assert!(saw_hand_started, "did not observe HandStarted event");
        Ok(())
    }

    /// A subscriber with no token must see hole cards blanked on every event.
    #[tokio::test]
    async fn dealer_service_stream_events_no_token_hides_cards()
    -> Result<(), Box<dyn std::error::Error>> {
        use tokio_stream::StreamExt;

        let service = make_service();

        let response = service
            .stream_events(Request::new(StreamEventsRequest {
                player_token: String::new(),
            }))
            .await?;
        let mut stream = response.into_inner();

        seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let mut saw_hand_started = false;
        for _ in 0..6 {
            let event = tokio::time::timeout(std::time::Duration::from_millis(200), stream.next())
                .await
                .expect("timeout waiting for event")
                .expect("stream ended")
                .expect("event error");
            if event.event_type == EventType::HandStarted as i32 {
                let status = event.current_status.expect("HandStarted carries status");
                for seat in &status.seats {
                    assert!(
                        seat.cards.is_empty(),
                        "no-token subscriber must not see cards for seat {}",
                        seat.seat_number
                    );
                }
                saw_hand_started = true;
                break;
            }
        }
        assert!(saw_hand_started, "did not observe HandStarted event");
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
                // Betting context: preflop, the SB (first to act heads-up) has posted 50
                // and must call 50 more to match the BB's 100.
                let expected_to_call = (DEFAULT_BIG_BLIND - DEFAULT_SMALL_BLIND) as u32;
                assert_eq!(
                    info.amount_to_call, expected_to_call,
                    "SB needs to call BB - SB = {expected_to_call} more"
                );
                assert!(info.min_raise > 0, "min_raise must be positive");
                assert_eq!(
                    info.current_bet, DEFAULT_BIG_BLIND as u32,
                    "current_bet must equal the posted big blind"
                );
            }
            other => panic!("expected Info, got {other:?}"),
        }
        Ok(())
    }

    // ── get_table_config ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_table_config_happy_path() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service();
        let response = service
            .get_table_config(Request::new(GetTableConfigRequest {}))
            .await?
            .into_inner();
        let config = response.config.expect("config must be present");
        assert_eq!(config.seat_count, u32::from(DEFAULT_SEAT_COUNT));
        assert_eq!(config.small_blind, DEFAULT_SMALL_BLIND as u32);
        assert_eq!(config.big_blind, DEFAULT_BIG_BLIND as u32);
        assert!(!config.variant.is_empty());
        assert_eq!(config.default_chips, DEFAULT_CHIPS as u32);
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
                    if r.hand_complete {
                        hand_complete = true;
                        let hr = r
                            .hand_result
                            .expect("hand_result must be present when hand_complete");
                        assert!(
                            !hr.winners.is_empty(),
                            "hand_result must have at least one winner"
                        );
                        assert!(
                            !hr.final_chips.is_empty(),
                            "hand_result must include final chip counts"
                        );
                    }
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
                client_secret: String::new(),
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
                client_secret: String::new(),
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
                client_secret: String::new(),
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

    // ── hand span lifecycle ───────────────────────────────────────────────────

    /// Records span open/close events into a shared `Vec`.  Used to assert
    /// hand-span lifecycle without depending on `tracing-test`'s formatted
    /// log buffer.
    #[derive(Default, Clone)]
    struct SpanCounter {
        events: std::sync::Arc<std::sync::Mutex<Vec<(String, &'static str)>>>,
    }

    impl SpanCounter {
        fn count(&self, span_name: &str, event: &str) -> usize {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, e)| n == span_name && *e == event)
                .count()
        }
    }

    struct SpanCounterLayer(SpanCounter);

    impl<S> tracing_subscriber::Layer<S> for SpanCounterLayer
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            _id: &tracing::span::Id,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            self.0
                .events
                .lock()
                .unwrap()
                .push((attrs.metadata().name().to_owned(), "new"));
        }

        fn on_close(&self, id: tracing::span::Id, ctx: tracing_subscriber::layer::Context<'_, S>) {
            if let Some(s) = ctx.span(&id) {
                self.0
                    .events
                    .lock()
                    .unwrap()
                    .push((s.metadata().name().to_owned(), "close"));
            }
        }
    }

    /// Plays a complete two-player hand to completion via the Act-only autonomous
    /// flow, reusing the existing `seat_two_players` + token-map helpers.
    ///
    /// Strategy: preflop SB calls / BB checks, then every subsequent street
    /// both players check until `hand_complete` is signalled.
    async fn play_hand_to_completion(
        service: &DealerService,
        tokens: &HashMap<u8, String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Preflop: SB calls, BB checks → auto-advances to flop.
        complete_preflop_betting(service, tokens).await?;

        // Post-preflop: check down through flop / turn / river.
        for _ in 0..(DEFAULT_SEAT_COUNT * 4) {
            let seat = {
                let guard = service.lock().expect("lock");
                guard.session.table.next_to_act()
            };
            let resp = service
                .act(act_request_with_token(seat, ActionType::Check, tokens))
                .await?
                .into_inner();
            match resp.result {
                Some(act_response::Result::ActionResult(r)) if r.hand_complete => break,
                Some(act_response::Result::ActionResult(_)) => {}
                Some(act_response::Result::Error(e)) => return Err(e.into()),
                None => return Err("empty act response".into()),
            }
        }
        Ok(())
    }

    /// The `hand` span must be opened exactly once when `start_hand` succeeds
    /// and closed exactly once when the hand reaches `HandComplete`.
    // `flavor = "current_thread"` keeps the tokio future on a single thread,
    // and `#[serial]` ensures no concurrent `#[tokio::test]` with a multi-
    // thread runtime can land workers on this thread and create spans that
    // bypass the test's thread-local SpanCounter subscriber.
    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn hand_span_spans_full_hand_lifecycle() -> Result<(), Box<dyn std::error::Error>> {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let counter = SpanCounter::default();
        let _guard = tracing_subscriber::registry()
            .with(SpanCounterLayer(counter.clone()))
            .set_default();

        let service = make_service();
        let tokens = seat_two_players(&service).await?;

        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        play_hand_to_completion(&service, &tokens).await?;

        assert_eq!(
            counter.count("hand", "new"),
            1,
            "hand span must open exactly once"
        );
        assert_eq!(
            counter.count("hand", "close"),
            1,
            "hand span must close exactly once"
        );

        // A full heads-up check-down covers 4 streets (preflop -> flop -> turn -> river).
        // Expect at least 3 StreetAdvanced events (flop, turn, river); showdown is the
        // 4th street advance in some engines and adds a 4th span.
        let opened = counter.count("street", "new");
        let closed = counter.count("street", "close");
        assert!(
            opened >= 3,
            "expected >=3 street spans opened, got {opened}"
        );
        assert_eq!(
            opened, closed,
            "every street span must be closed (got {opened} open, {closed} close)"
        );

        // Action spans: at least one per act call in the hand (preflop SB call,
        // BB check; then checks on each post-flop street).
        let action_opens = counter.count("action", "new");
        assert!(
            action_opens >= 1,
            "expected >=1 action span during full hand, got {action_opens}"
        );
        Ok(())
    }

    /// Verifies that an injected `traceparent` causes the `act` handler to
    /// open an action span (soft assertion via span counter — the strict
    /// trace-id parentage check requires a registry-aware `OTel` layer that is
    /// not wired in tests, as noted in the task spec).
    #[serial_test::serial]
    #[tokio::test(flavor = "current_thread")]
    async fn action_span_inherits_agent_context() -> Result<(), Box<dyn std::error::Error>> {
        use opentelemetry::global;
        use opentelemetry_sdk::propagation::TraceContextPropagator;
        use tonic::Request;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        // Install the W3C propagator globally so the act handler can extract it.
        global::set_text_map_propagator(TraceContextPropagator::new());

        let counter = SpanCounter::default();
        let _guard = tracing_subscriber::registry()
            .with(SpanCounterLayer(counter.clone()))
            .set_default();

        let service = make_service();
        let tokens = seat_two_players(&service).await?;
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        // Find which seat is next to act.
        let next_seat = {
            let guard = service.lock().expect("lock");
            guard.session.table.next_to_act()
        };

        // Build an ActRequest carrying a synthetic `traceparent` header.
        let trace_id = "0af7651916cd43dd8448eb211c80319c";
        let span_id = "b7ad6b7169203331";
        let traceparent = format!("00-{trace_id}-{span_id}-01");
        let token = tokens.get(&next_seat).expect("token for seat").clone();

        let mut req = Request::new(ActRequest {
            action: Some(pkdealer_proto::dealer::PlayerAction {
                seat: u32::from(next_seat),
                // Heads-up preflop: SB must call to continue.
                action_type: ActionType::Call as i32,
                amount: 0,
            }),
        });
        req.metadata_mut().insert(
            "traceparent",
            tonic::metadata::MetadataValue::try_from(traceparent).unwrap(),
        );
        req.metadata_mut().insert(
            PLAYER_TOKEN_METADATA_KEY,
            tonic::metadata::MetadataValue::try_from(token).unwrap(),
        );

        let _ = service.act(req).await?;

        // Soft assertion: at least one action span was opened (proves the span
        // construction path ran for the injected-traceparent branch).
        let action_opens = counter.count("action", "new");
        assert!(
            action_opens >= 1,
            "expected >=1 action span, got {action_opens}"
        );

        Ok(())
    }

    // ── Rebuy / GetPlayerStats ────────────────────────────────────────────────

    fn make_service_with_config(config: DealerConfig) -> DealerService {
        DealerService::new_with_config(config)
    }

    /// Builds a `Rebuy` request with the `x-player-token` metadata set.
    fn rebuy_request_with_token(chips: u32, token: &str) -> Request<RebuyRequest> {
        let mut req = Request::new(RebuyRequest { chips });
        req.metadata_mut().insert(
            PLAYER_TOKEN_METADATA_KEY,
            token.parse().expect("valid token"),
        );
        req
    }

    /// Reaches into the locked state to set a seat's chips to zero, simulating
    /// a bust without playing a hand to completion.
    fn zero_seat_chips(service: &DealerService, seat: u8) {
        let mut guard = service.lock().expect("lock");
        if let Some(s) = guard.session.table.seats.get_seat_mut(seat) {
            s.player.chips = 0;
        }
    }

    /// Reads a seat's `(chips, withdrawn)` for an assertion.
    fn read_chip_state(service: &DealerService, seat: u8) -> (usize, usize) {
        let guard = service.lock().expect("lock");
        let s = guard.session.table.seats.get_seat(seat).expect("seat");
        (s.player.chips, s.player.withdrawn)
    }

    #[tokio::test]
    async fn rebuy_on_bust_disabled_rejects() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service_with_config(DealerConfig::default()); // both flags off
        let tokens = seat_two_players(&service).await?;
        let (&seat, token) = tokens.iter().next().expect("token");
        zero_seat_chips(&service, seat);

        let resp = service
            .rebuy(rebuy_request_with_token(0, token))
            .await?
            .into_inner();
        assert!(
            matches!(resp.result, Some(rebuy_response::Result::Error(_))),
            "expected Error variant, got {:?}",
            resp.result
        );
        Ok(())
    }

    #[tokio::test]
    async fn rebuy_on_bust_enabled_reloads_default() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service_with_config(DealerConfig {
            default_rebuy_amount: 5_000,
            rebuy_on_bust_enabled: true,
            topup_enabled: false,
        });
        let tokens = seat_two_players(&service).await?;
        let (&seat, token) = tokens.iter().next().expect("token");

        // After seat_two_players, withdrawn == 1_000 (initial stack). Then bust.
        let (_, w_before) = read_chip_state(&service, seat);
        assert_eq!(1_000, w_before);
        zero_seat_chips(&service, seat);

        let resp = service
            .rebuy(rebuy_request_with_token(0, token))
            .await?
            .into_inner();
        let info = match resp.result {
            Some(rebuy_response::Result::Info(i)) => i,
            other => panic!("expected Info, got {other:?}"),
        };
        assert_eq!("bust", info.reason);
        assert_eq!(5_000, info.new_chips);
        assert_eq!(6_000, info.new_withdrawn);

        let (chips, withdrawn) = read_chip_state(&service, seat);
        assert_eq!(5_000, chips);
        assert_eq!(6_000, withdrawn);
        Ok(())
    }

    #[tokio::test]
    async fn rebuy_on_bust_enabled_reloads_custom_amount() -> Result<(), Box<dyn std::error::Error>>
    {
        let service = make_service_with_config(DealerConfig {
            default_rebuy_amount: 5_000,
            rebuy_on_bust_enabled: true,
            topup_enabled: false,
        });
        let tokens = seat_two_players(&service).await?;
        let (&seat, token) = tokens.iter().next().expect("token");
        zero_seat_chips(&service, seat);

        let resp = service
            .rebuy(rebuy_request_with_token(250, token))
            .await?
            .into_inner();
        let info = match resp.result {
            Some(rebuy_response::Result::Info(i)) => i,
            other => panic!("expected Info, got {other:?}"),
        };
        assert_eq!(250, info.new_chips);
        assert_eq!(1_250, info.new_withdrawn);
        Ok(())
    }

    #[tokio::test]
    async fn topup_disabled_rejects_with_healthy_stack() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service_with_config(DealerConfig {
            default_rebuy_amount: 5_000,
            rebuy_on_bust_enabled: true, // bust flag on, topup flag off
            topup_enabled: false,
        });
        let tokens = seat_two_players(&service).await?;
        let (_, token) = tokens.iter().next().expect("token");

        let resp = service
            .rebuy(rebuy_request_with_token(100, token))
            .await?
            .into_inner();
        assert!(matches!(
            resp.result,
            Some(rebuy_response::Result::Error(_))
        ));
        Ok(())
    }

    #[tokio::test]
    async fn topup_enabled_reloads_between_hands() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service_with_config(DealerConfig {
            default_rebuy_amount: 500,
            rebuy_on_bust_enabled: false,
            topup_enabled: true,
        });
        let tokens = seat_two_players(&service).await?;
        let (&seat, token) = tokens.iter().next().expect("token");

        let (chips_before, withdrawn_before) = read_chip_state(&service, seat);
        assert_eq!(1_000, chips_before);

        let resp = service
            .rebuy(rebuy_request_with_token(0, token))
            .await?
            .into_inner();
        let info = match resp.result {
            Some(rebuy_response::Result::Info(i)) => i,
            other => panic!("expected Info, got {other:?}"),
        };
        assert_eq!("topup", info.reason);
        assert_eq!(1_500, info.new_chips);
        assert_eq!(withdrawn_before + 500, info.new_withdrawn as usize);
        Ok(())
    }

    #[tokio::test]
    async fn topup_rejected_mid_hand() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service_with_config(DealerConfig {
            default_rebuy_amount: 500,
            rebuy_on_bust_enabled: false,
            topup_enabled: true,
        });
        let tokens = seat_two_players(&service).await?;
        let (_, token) = tokens.iter().next().expect("token");
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;

        let resp = service
            .rebuy(rebuy_request_with_token(100, token))
            .await?
            .into_inner();
        match resp.result {
            Some(rebuy_response::Result::Error(msg)) => {
                assert!(msg.contains("during a hand"), "unexpected error msg: {msg}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn bust_rebuy_rejected_mid_hand() -> Result<(), Box<dyn std::error::Error>> {
        // An all-in busted seat (chips == 0, chips_in_play > 0) must not be
        // reloaded while a hand is in progress, even with rebuy_on_bust_enabled.
        let service = make_service_with_config(DealerConfig {
            default_rebuy_amount: 500,
            rebuy_on_bust_enabled: true,
            topup_enabled: false,
        });
        let tokens = seat_two_players(&service).await?;
        let (&seat, token) = tokens.iter().next().expect("token");
        service
            .start_hand(Request::new(StartHandRequest {}))
            .await?;
        zero_seat_chips(&service, seat);

        let resp = service
            .rebuy(rebuy_request_with_token(0, token))
            .await?
            .into_inner();
        match resp.result {
            Some(rebuy_response::Result::Error(msg)) => {
                assert!(msg.contains("during a hand"), "unexpected error msg: {msg}");
            }
            other => panic!("expected Error, got {other:?}"),
        }
        let (chips, _) = read_chip_state(&service, seat);
        assert_eq!(0, chips, "seat must not be reloaded mid-hand");
        Ok(())
    }

    #[tokio::test]
    async fn rebuy_missing_token_permission_denied() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service_with_config(DealerConfig {
            default_rebuy_amount: 500,
            rebuy_on_bust_enabled: true,
            topup_enabled: true,
        });
        let _ = seat_two_players(&service).await?;
        let result = service.rebuy(Request::new(RebuyRequest { chips: 0 })).await;
        match result {
            Err(s) => assert_eq!(s.code(), tonic::Code::PermissionDenied),
            Ok(_) => panic!("expected permission_denied"),
        }
        Ok(())
    }

    /// Exercises the auto-rebuy helper directly against a manipulated state,
    /// avoiding pkcore's `start_hand` chip-validity checks. Verifies that:
    /// - flag off → busted seats stay at 0
    /// - flag on  → busted seats are reloaded to `default_rebuy_amount` and
    ///   their `withdrawn` ledger is bumped
    #[tokio::test]
    async fn auto_rebuy_at_hand_end_only_when_flag_on() -> Result<(), Box<dyn std::error::Error>> {
        // Flag OFF.
        let service_off = make_service_with_config(DealerConfig {
            default_rebuy_amount: 999,
            rebuy_on_bust_enabled: false,
            topup_enabled: false,
        });
        let tokens_off = seat_two_players(&service_off).await?;
        let busted_off = *tokens_off.keys().next().expect("seat");
        zero_seat_chips(&service_off, busted_off);
        {
            let mut guard = service_off.lock().expect("lock");
            let (events, labels) = service_off.run_auto_rebuy(&mut guard);
            assert!(events.is_empty());
            assert!(labels.is_empty());
        }
        let (chips_off, _) = read_chip_state(&service_off, busted_off);
        assert_eq!(0, chips_off, "flag-off busted seat must not be reloaded");

        // Flag ON.
        let service_on = make_service_with_config(DealerConfig {
            default_rebuy_amount: 999,
            rebuy_on_bust_enabled: true,
            topup_enabled: false,
        });
        let tokens_on = seat_two_players(&service_on).await?;
        let busted_on = *tokens_on.keys().next().expect("seat");
        zero_seat_chips(&service_on, busted_on);
        let (events, labels) = {
            let mut guard = service_on.lock().expect("lock");
            service_on.run_auto_rebuy(&mut guard)
        };
        assert_eq!(1, labels.len());
        assert_eq!(busted_on, labels[0]);
        assert_eq!(1, events.len());
        let (chips_on, withdrawn_on) = read_chip_state(&service_on, busted_on);
        assert_eq!(999, chips_on, "flag-on busted seat reloaded to default");
        // Initial withdrawn = 1_000 (buy-in); after auto-reload of 999, == 1_999.
        assert_eq!(1_999, withdrawn_on);
        Ok(())
    }

    #[tokio::test]
    async fn get_player_stats_returns_signed_pl_after_loss()
    -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service_with_config(DealerConfig::default());
        let _ = seat_two_players(&service).await?;
        // Force a known loss state directly: chips=0, chips_in_play=0,
        // withdrawn=1000 (initial). chips_in_play is zeroed explicitly so the
        // profit_loss assertion below stays valid regardless of seat_two_players
        // internals (profit = chips + chips_in_play - withdrawn).
        {
            let mut guard = service.lock().expect("lock");
            for i in 0..guard.session.table.seats.size() {
                if let Some(s) = guard.session.table.seats.get_seat_mut(i)
                    && !s.is_empty()
                {
                    s.player.chips = 0;
                    s.player.chips_in_play = 0;
                }
            }
        }
        let resp = service
            .get_player_stats(Request::new(GetPlayerStatsRequest {}))
            .await?
            .into_inner();
        assert!(!resp.stats.is_empty());
        for s in resp.stats {
            assert_eq!(-1_000, s.profit_loss, "seat {}", s.seat);
            assert_eq!(1_000, s.withdrawn);
            assert_eq!(0, s.chips);
        }
        Ok(())
    }

    #[tokio::test]
    async fn seat_info_includes_pl_fields() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service_with_config(DealerConfig::default());
        let _ = seat_two_players(&service).await?;
        let resp = service
            .get_status(Request::new(GetStatusRequest {}))
            .await?
            .into_inner();
        let status = resp.status.expect("status present");
        assert!(!status.seats.is_empty());
        for seat in status.seats {
            assert_eq!(1_000, seat.chips);
            assert_eq!(1_000, seat.withdrawn);
            assert_eq!(0, seat.chips_in_play);
            assert_eq!(0, seat.profit_loss);
        }
        Ok(())
    }

    #[tokio::test]
    async fn get_table_config_includes_rebuy_fields() -> Result<(), Box<dyn std::error::Error>> {
        let service = make_service_with_config(DealerConfig {
            default_rebuy_amount: 777,
            rebuy_on_bust_enabled: true,
            topup_enabled: true,
        });
        let resp = service
            .get_table_config(Request::new(GetTableConfigRequest {}))
            .await?
            .into_inner();
        let cfg = resp.config.expect("config");
        assert_eq!(777, cfg.default_rebuy_amount);
        assert!(cfg.rebuy_on_bust_enabled);
        assert!(cfg.topup_enabled);
        Ok(())
    }
}
