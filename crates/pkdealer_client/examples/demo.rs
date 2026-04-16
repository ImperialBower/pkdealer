//! # pkdealer demo
//!
//! Plays through one complete 9-player hand against the running service,
//! driving the entire hand with `Act` alone — no `advance_street` or
//! `end_hand` calls required.  Streets auto-advance and the hand resolves
//! inside the server's `Act` handler; the demo detects transitions by
//! watching the board string change between actions.
//!
//! Flow:
//!   1.  Ping the service
//!   2.  Seat nine players (Alice … Ivy) — capture their auth tokens
//!   3.  Start hand — show hole cards via spectator token
//!   4.  Loop: get next-to-act → Act (Call preflop / Check post-flop)
//!         • print each action with pot size
//!         • print board whenever a new street is dealt
//!         • stop when `ActionResult.hand_complete` is true
//!   5.  Final chip counts
//!
//! Run with the service already started in another terminal:
//!
//!   cargo run --bin pkdealer_service
//!   cargo run --example demo -p pkdealer_client

use std::collections::HashMap;

use pkdealer_proto::dealer::{
    ActRequest, ActionType, GetChipsRequest, GetStatusRequest, PlayerAction, SeatPlayerRequest,
    StartHandRequest, act_response, dealer_service_client::DealerServiceClient,
    get_next_to_act_response, seat_player_response, start_hand_response,
};
use tonic::{Request, metadata::MetadataValue};

const ENDPOINT: &str = "http://127.0.0.1:50051";
/// Default spectator token — must match `PKDEALER_SPECTATOR_TOKEN` on the server.
const SPECTATOR_TOKEN: &str = "spectator";
/// gRPC metadata key for the player auth token.
const PLAYER_TOKEN_KEY: &str = "x-player-token";

/// (name, starting chips)
const PLAYERS: &[(&str, u32)] = &[
    ("Alice", 1_500),
    ("Bob", 2_000),
    ("Carol", 1_200),
    ("Dave", 1_800),
    ("Eve", 2_500),
    ("Frank", 1_100),
    ("Grace", 1_700),
    ("Hank", 2_200),
    ("Ivy", 1_300),
];

type Client = DealerServiceClient<tonic::transport::Channel>;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = DealerServiceClient::connect(ENDPOINT).await?;

    // ── 1. Ping ───────────────────────────────────────────────────────────────
    section("PING");
    let pong = client
        .ping(Request::new(pkdealer_proto::new_ping_request("demo")))
        .await?
        .into_inner();
    println!("  {}", pong.message);

    // ── 2. Seat players ───────────────────────────────────────────────────────
    section("SEATING PLAYERS");
    let mut seat_tokens: HashMap<u32, String> = HashMap::new();
    for (name, chips) in PLAYERS {
        let (seat_number, token) = seat_player(&mut client, name, *chips).await?;
        println!("  {name:5}  →  seat {seat_number}  (chips={chips})");
        seat_tokens.insert(seat_number, token);
    }

    // ── 3. Start hand ─────────────────────────────────────────────────────────
    section("START HAND");
    let start = client
        .start_hand(Request::new(StartHandRequest {}))
        .await?
        .into_inner();
    match start.result {
        Some(start_hand_response::Result::Status(s)) => {
            println!("  pot         : {}", s.pot);
            println!("  next_to_act : seat {}", s.next_to_act);
        }
        Some(start_hand_response::Result::Error(e)) => {
            eprintln!("  Error starting hand: {e}");
            return Ok(());
        }
        None => {
            eprintln!("  empty start_hand response");
            return Ok(());
        }
    }

    // Show all hole cards via the spectator token.
    section("HOLE CARDS (spectator view)");
    show_all_cards(&mut client).await?;

    // ── 4. Drive the hand via Act alone ───────────────────────────────────────
    section("HAND IN PROGRESS");
    let mut board = String::new(); // track board to detect street changes

    'hand: loop {
        // Ask who is next to act.
        let next = client
            .get_next_to_act(Request::new(pkdealer_proto::dealer::GetNextToActRequest {}))
            .await?
            .into_inner();

        let (acting_seat, acting_name) = match next.result {
            Some(get_next_to_act_response::Result::Info(info)) => {
                (info.seat, info.player_name)
            }
            _ => break 'hand, // no hand in progress or no one to act
        };

        // Try Call first (correct for preflop); fall back to Check when Call
        // is illegal (post-flop with no bet outstanding).
        let (action_label, result) =
            match try_act(&mut client, acting_seat, ActionType::Call, &seat_tokens).await? {
                (_, Some(r)) => ("Call", r),
                (_, None) => {
                    match try_act(&mut client, acting_seat, ActionType::Check, &seat_tokens).await?
                    {
                        (_, Some(r)) => ("Check", r),
                        (_, None) => break 'hand, // neither action succeeded; round is over
                    }
                }
            };

        println!(
            "  seat {acting_seat} {acting_name:5}  pot={:4}  → {action_label}",
            result.pot,
        );

        // Detect a street change by re-reading the board.
        let new_board = current_board(&mut client).await?;
        if new_board != board && !new_board.is_empty() {
            let street = match new_board.split_whitespace().count() {
                3 => "FLOP",
                4 => "TURN",
                5 => "RIVER",
                _ => "STREET",
            };
            println!("  ── {street}: [{new_board}]");
            board = new_board;
        }

        if result.hand_complete {
            println!("  ── HAND COMPLETE");
            break 'hand;
        }
    }

    // ── 5. Final chip counts ──────────────────────────────────────────────────
    section("FINAL CHIP COUNTS");
    let chips = client
        .get_chips(Request::new(GetChipsRequest {}))
        .await?
        .into_inner();
    let total: u32 = chips.chips.iter().map(|p| p.chips).sum();
    for p in &chips.chips {
        println!("  seat {} {:<5}  chips={}", p.seat, p.player_name, p.chips);
    }
    println!("  total chips in play: {total}");

    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Attempts one `Act` RPC.  Returns `(action_type, Some(ActionResult))` on
/// success, or `(action_type, None)` when the server returns an error (illegal
/// action, wrong turn, etc.).
async fn try_act(
    client: &mut Client,
    seat: u32,
    action_type: ActionType,
    seat_tokens: &HashMap<u32, String>,
) -> Result<(ActionType, Option<pkdealer_proto::dealer::ActionResult>), Box<dyn std::error::Error>>
{
    let mut req = Request::new(ActRequest {
        action: Some(PlayerAction {
            seat,
            action_type: action_type as i32,
            amount: 0,
        }),
    });
    if let Some(token) = seat_tokens.get(&seat)
        && let Ok(mv) = token.parse::<MetadataValue<_>>()
    {
        req.metadata_mut().insert(PLAYER_TOKEN_KEY, mv);
    }

    let resp = client.act(req).await?.into_inner();
    match resp.result {
        Some(act_response::Result::ActionResult(r)) => Ok((action_type, Some(r))),
        Some(act_response::Result::Error(_)) | None => Ok((action_type, None)),
    }
}

/// Returns the current board string (empty string between hands).
async fn current_board(client: &mut Client) -> Result<String, Box<dyn std::error::Error>> {
    let status = client
        .get_status(Request::new(GetStatusRequest {}))
        .await?
        .into_inner()
        .status
        .unwrap_or_default();
    Ok(status.board)
}

/// Seats a player and returns `(seat_number, player_token)`.
async fn seat_player(
    client: &mut Client,
    name: &str,
    chips: u32,
) -> Result<(u32, String), Box<dyn std::error::Error>> {
    let inner = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: name.to_owned(),
            chips,
        }))
        .await?
        .into_inner();
    match inner.result {
        Some(seat_player_response::Result::SeatNumber(n)) => Ok((n, inner.player_token)),
        Some(seat_player_response::Result::Error(e)) => Err(e.into()),
        None => Err("empty seat_player response".into()),
    }
}

/// Show all hole cards by calling `GetStatus` with the spectator token.
async fn show_all_cards(client: &mut Client) -> Result<(), Box<dyn std::error::Error>> {
    let mut req = Request::new(GetStatusRequest {});
    if let Ok(mv) = SPECTATOR_TOKEN.parse::<MetadataValue<_>>() {
        req.metadata_mut().insert(PLAYER_TOKEN_KEY, mv);
    }
    let status = client
        .get_status(req)
        .await?
        .into_inner()
        .status
        .unwrap_or_default();
    for seat in &status.seats {
        println!(
            "  seat {} {:<5}  chips={:5}  cards=[{}]  state={}",
            seat.seat_number, seat.player_name, seat.chips, seat.cards, seat.state,
        );
    }
    Ok(())
}

fn section(title: &str) {
    let dashes = 50usize.saturating_sub(title.len() + 4);
    println!("\n── {title} {}", "─".repeat(dashes));
}
