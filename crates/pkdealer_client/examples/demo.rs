//! # pkdealer demo
//!
//! Plays through one complete 9-player hand against the running service:
//!
//!   1.  Ping the service
//!   2.  Seat nine players (Alice … Ivy) — capture their auth tokens
//!   3.  Show hole cards using the spectator token (all cards visible)
//!   4.  Preflop betting — everyone calls; BB checks their option
//!   5.  Advance to flop — show board
//!   6.  Flop betting — everyone checks
//!   7.  Advance to turn — show board
//!   8.  Turn betting — everyone checks
//!   9.  Advance to river — show board
//!  10.  River betting — everyone checks
//!  11.  EndHand — pay out, show result
//!  12.  Final chip counts
//!
//! Run with the service already started in another terminal:
//!
//!   cargo run --bin pkdealer_service
//!   cargo run --example demo -p pkdealer_client

use std::collections::HashMap;

use pkdealer_proto::dealer::{
    ActRequest, ActionType, AdvanceStreetRequest, EndHandRequest, GetChipsRequest,
    GetNextToActRequest, GetStatusRequest, PlayerAction, SeatPlayerRequest, StartHandRequest,
    advance_street_response, dealer_service_client::DealerServiceClient, end_hand_response,
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
    // seat_tokens maps seat_number → player_token for use in Act requests.
    let mut seat_tokens: HashMap<u32, String> = HashMap::new();
    for (name, chips) in PLAYERS {
        let (seat_number, token) = seat(&mut client, name, *chips).await?;
        println!("  {:5}  →  seat {seat_number}  (chips={chips})", name);
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
            println!("  pot          : {}", s.pot);
            println!("  next_to_act  : seat {}", s.next_to_act);
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

    // ── 3b. Show hole cards via spectator token ───────────────────────────────
    section("HOLE CARDS (spectator view — all cards visible)");
    show_all_cards(&mut client).await?;

    // ── 4. Preflop betting ────────────────────────────────────────────────────
    section("PREFLOP BETTING");
    let hand_over = run_betting_round(&mut client, ActionType::Call, &seat_tokens).await?;
    if hand_over {
        return finish(&mut client).await;
    }

    // ── 5. Flop ───────────────────────────────────────────────────────────────
    section("ADVANCE → FLOP");
    if !advance_street(&mut client).await? {
        eprintln!("  advance_street failed unexpectedly");
        return Ok(());
    }
    show_board(&mut client).await?;

    // ── 6. Flop betting ───────────────────────────────────────────────────────
    section("FLOP BETTING");
    let hand_over = run_betting_round(&mut client, ActionType::Check, &seat_tokens).await?;
    if hand_over {
        return finish(&mut client).await;
    }

    // ── 7. Turn ───────────────────────────────────────────────────────────────
    section("ADVANCE → TURN");
    if !advance_street(&mut client).await? {
        eprintln!("  advance_street failed unexpectedly");
        return Ok(());
    }
    show_board(&mut client).await?;

    // ── 8. Turn betting ───────────────────────────────────────────────────────
    section("TURN BETTING");
    let hand_over = run_betting_round(&mut client, ActionType::Check, &seat_tokens).await?;
    if hand_over {
        return finish(&mut client).await;
    }

    // ── 9. River ──────────────────────────────────────────────────────────────
    section("ADVANCE → RIVER");
    if !advance_street(&mut client).await? {
        eprintln!("  advance_street failed unexpectedly");
        return Ok(());
    }
    show_board(&mut client).await?;

    // ── 10. River betting ─────────────────────────────────────────────────────
    section("RIVER BETTING");
    run_betting_round(&mut client, ActionType::Check, &seat_tokens).await?;

    // ── 11. End hand / showdown ───────────────────────────────────────────────
    finish(&mut client).await
}

// ── betting helpers ───────────────────────────────────────────────────────────

/// Drive one full betting round.
///
/// Each player that is next-to-act receives `preferred_action` with their auth
/// token.  If that action is rejected (e.g. `Call` for BB who has already
/// matched the bet), we retry once with `Check`.  If `Check` also fails the
/// round is over.
///
/// Returns `true` when the hand ended early (`hand_complete` was set).
async fn run_betting_round(
    client: &mut Client,
    preferred_action: ActionType,
    seat_tokens: &HashMap<u32, String>,
) -> Result<bool, Box<dyn std::error::Error>> {
    loop {
        let (acting_seat, acting_name, pot) = match next_to_act(client).await? {
            Some(info) => info,
            None => break, // no one left to act
        };

        let outcome = try_act(client, acting_seat, preferred_action, seat_tokens).await?;
        let final_outcome = match outcome {
            ActOutcome::HandComplete => return Ok(true),
            ActOutcome::Continuing => {
                println!("  seat {acting_seat} ({acting_name})  pot={pot}  → {preferred_action:?}");
                ActOutcome::Continuing
            }
            ActOutcome::IllegalAction(ref msg) => {
                // Preferred action was rejected — try Check as a fallback.
                let fallback = try_act(client, acting_seat, ActionType::Check, seat_tokens).await?;
                match fallback {
                    ActOutcome::HandComplete => return Ok(true),
                    ActOutcome::Continuing => {
                        println!(
                            "  seat {acting_seat} ({acting_name})  pot={pot}  → Check  \
                             (preferred {preferred_action:?} rejected: {msg})"
                        );
                        ActOutcome::Continuing
                    }
                    ActOutcome::IllegalAction(ref fb_msg) => {
                        // Both actions rejected — round is over.
                        println!("  round complete ({fb_msg})");
                        break;
                    }
                    ActOutcome::RoundOver => break,
                }
            }
            ActOutcome::RoundOver => break,
        };
        let _ = final_outcome;
    }
    Ok(false)
}

enum ActOutcome {
    Continuing,
    HandComplete,
    IllegalAction(String),
    RoundOver,
}

async fn try_act(
    client: &mut Client,
    seat: u32,
    action_type: ActionType,
    seat_tokens: &HashMap<u32, String>,
) -> Result<ActOutcome, Box<dyn std::error::Error>> {
    let mut req = Request::new(ActRequest {
        action: Some(PlayerAction {
            seat,
            action_type: action_type as i32,
            amount: 0,
        }),
    });

    // Attach the player's auth token so the server can verify seat ownership.
    if let Some(token) = seat_tokens.get(&seat)
        && let Ok(mv) = token.parse::<MetadataValue<_>>()
    {
        req.metadata_mut().insert(PLAYER_TOKEN_KEY, mv);
    }

    let resp = client.act(req).await?.into_inner();

    match resp.result {
        Some(pkdealer_proto::dealer::act_response::Result::ActionResult(r)) => {
            if r.hand_complete {
                Ok(ActOutcome::HandComplete)
            } else {
                Ok(ActOutcome::Continuing)
            }
        }
        Some(pkdealer_proto::dealer::act_response::Result::Error(e)) => {
            // Distinguish "illegal action" (wrong action for this state) from
            // "no one left to act" (round is over).
            if e.to_lowercase().contains("illegal")
                || e.to_lowercase().contains("invalid")
                || e.to_lowercase().contains("cannot")
                || e.to_lowercase().contains("not allowed")
            {
                Ok(ActOutcome::IllegalAction(e))
            } else {
                println!("  act error: {e}");
                Ok(ActOutcome::RoundOver)
            }
        }
        None => Ok(ActOutcome::RoundOver),
    }
}

/// Returns `(seat, name, pot)` for the player who must act next, or `None`
/// when there is no hand in progress or no player is yet-to-act.
async fn next_to_act(
    client: &mut Client,
) -> Result<Option<(u32, String, u32)>, Box<dyn std::error::Error>> {
    let resp = client
        .get_next_to_act(Request::new(GetNextToActRequest {}))
        .await?
        .into_inner();
    match resp.result {
        Some(get_next_to_act_response::Result::Info(info)) => {
            Ok(Some((info.seat, info.player_name, info.pot)))
        }
        _ => Ok(None),
    }
}

// ── street helpers ────────────────────────────────────────────────────────────

/// Ask the service to advance to the next street.  Returns `true` on success.
async fn advance_street(client: &mut Client) -> Result<bool, Box<dyn std::error::Error>> {
    let resp = client
        .advance_street(Request::new(AdvanceStreetRequest {}))
        .await?
        .into_inner();
    match resp.result {
        Some(advance_street_response::Result::StreetResult(s)) => {
            println!("  board: {}", s.board);
            Ok(true)
        }
        Some(advance_street_response::Result::Error(e)) => {
            eprintln!("  advance_street error: {e}");
            Ok(false)
        }
        None => Ok(false),
    }
}

/// Print the current community cards using an unauthenticated status call.
/// (Board cards are always public; no token needed.)
async fn show_board(client: &mut Client) -> Result<(), Box<dyn std::error::Error>> {
    let status = client
        .get_status(Request::new(GetStatusRequest {}))
        .await?
        .into_inner()
        .status
        .unwrap_or_default();
    println!("  board : {}", status.board);
    println!("  pot   : {}", status.pot);
    Ok(())
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
            "  seat {} {:5}  chips={:5}  cards=[{}]  state={}",
            seat.seat_number, seat.player_name, seat.chips, seat.cards, seat.state,
        );
    }
    Ok(())
}

// ── end-of-hand helpers ───────────────────────────────────────────────────────

/// Call EndHand, print result, then print final chip counts.
async fn finish(client: &mut Client) -> Result<(), Box<dyn std::error::Error>> {
    section("END HAND / SHOWDOWN");
    let end = client
        .end_hand(Request::new(EndHandRequest {}))
        .await?
        .into_inner();
    match end.result {
        Some(end_hand_response::Result::HandResult(r)) => {
            println!("  {}", r.result_text);
        }
        Some(end_hand_response::Result::Error(e)) => {
            eprintln!("  Error: {e}");
        }
        None => eprintln!("  empty end_hand response"),
    }

    section("FINAL CHIP COUNTS");
    let chips = client
        .get_chips(Request::new(GetChipsRequest {}))
        .await?
        .into_inner();
    let total: u32 = chips.chips.iter().map(|p| p.chips).sum();
    for p in &chips.chips {
        println!("  seat {} {:5}  chips={}", p.seat, p.player_name, p.chips);
    }
    println!("  total chips in play: {total}");

    section("TABLE STATUS");
    let status = client
        .get_status(Request::new(GetStatusRequest {}))
        .await?
        .into_inner()
        .status
        .unwrap_or_default();
    println!("  hand_in_progress : {}", status.hand_in_progress);
    println!("  game_over        : {}", status.game_over);
    println!("  pot              : {}", status.pot);

    println!();
    Ok(())
}

// ── misc helpers ──────────────────────────────────────────────────────────────

fn section(title: &str) {
    let dashes = 50usize.saturating_sub(title.len() + 4);
    println!("\n── {title} {}", "─".repeat(dashes));
}

/// Seats a player and returns `(seat_number, player_token)`.
async fn seat(
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
