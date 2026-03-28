//! # pkdealer demo
//!
//! Plays through one complete hand against the running service:
//!
//!   1. Ping the service
//!   2. Seat two players (Alice and Bob)
//!   3. Start a hand
//!   4. Show who acts first and the pot
//!   5. UTG folds — hand ends immediately
//!   6. Call EndHand to pay out
//!   7. Show final chip counts
//!
//! Run with the service already started in another terminal:
//!
//!   cargo run --bin pkdealer_service
//!   cargo run --example demo -p pkdealer_client

use pkdealer_proto::dealer::{
    ActRequest, ActionType, EndHandRequest, GetChipsRequest, GetNextToActRequest, GetStatusRequest,
    PlayerAction, SeatPlayerRequest, StartHandRequest, dealer_service_client::DealerServiceClient,
    end_hand_response, get_next_to_act_response, seat_player_response, start_hand_response,
};
use tonic::Request;

const ENDPOINT: &str = "http://127.0.0.1:50051";

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
    let alice_seat = seat(&mut client, "Alice", 1_000).await?;
    let bob_seat = seat(&mut client, "Bob", 1_000).await?;
    println!("  Alice → seat {alice_seat}");
    println!("  Bob   → seat {bob_seat}");

    // ── 3. Start hand ─────────────────────────────────────────────────────────
    section("START HAND");
    let start = client
        .start_hand(Request::new(StartHandRequest {}))
        .await?
        .into_inner();
    match start.result {
        Some(start_hand_response::Result::Status(s)) => {
            println!("  hand_in_progress : {}", s.hand_in_progress);
            println!("  pot              : {}", s.pot);
            println!("  next_to_act      : seat {}", s.next_to_act);
            for seat in &s.seats {
                println!(
                    "  seat {} {:6}  chips={:5}  cards={}  state={}",
                    seat.seat_number, seat.player_name, seat.chips, seat.cards, seat.state
                );
            }
        }
        Some(start_hand_response::Result::Error(e)) => {
            eprintln!("  Error: {e}");
            return Ok(());
        }
        None => eprintln!("  empty response"),
    }

    // ── 4. Who acts first? ────────────────────────────────────────────────────
    section("NEXT TO ACT");
    let nta = client
        .get_next_to_act(Request::new(GetNextToActRequest {}))
        .await?
        .into_inner();
    let acting_seat = match nta.result {
        Some(get_next_to_act_response::Result::Info(info)) => {
            println!(
                "  seat {} ({})  chips={}  pot={}",
                info.seat, info.player_name, info.chips, info.pot
            );
            info.seat as u8
        }
        Some(get_next_to_act_response::Result::Message(m)) => {
            println!("  {m}");
            return Ok(());
        }
        None => {
            eprintln!("  empty response");
            return Ok(());
        }
    };

    // ── 5. UTG folds ──────────────────────────────────────────────────────────
    section("ACT — FOLD");
    println!("  seat {acting_seat} folds");
    let act_resp = client
        .act(Request::new(ActRequest {
            action: Some(PlayerAction {
                seat: u32::from(acting_seat),
                action_type: ActionType::Fold as i32,
                amount: 0,
            }),
        }))
        .await?
        .into_inner();
    match act_resp.result {
        Some(pkdealer_proto::dealer::act_response::Result::ActionResult(r)) => {
            println!("  next_to_act   : seat {}", r.next_to_act);
            println!("  pot           : {}", r.pot);
            println!("  hand_complete : {}", r.hand_complete);
        }
        Some(pkdealer_proto::dealer::act_response::Result::Error(e)) => {
            eprintln!("  Error: {e}");
        }
        None => eprintln!("  empty response"),
    }

    // ── 6. End hand ───────────────────────────────────────────────────────────
    section("END HAND");
    let end = client
        .end_hand(Request::new(EndHandRequest {}))
        .await?
        .into_inner();
    match end.result {
        Some(end_hand_response::Result::HandResult(r)) => {
            println!("  result : {}", r.result_text);
        }
        Some(end_hand_response::Result::Error(e)) => {
            eprintln!("  Error: {e}");
        }
        None => eprintln!("  empty response"),
    }

    // ── 7. Final chip counts ──────────────────────────────────────────────────
    section("FINAL CHIPS");
    let chips = client
        .get_chips(Request::new(GetChipsRequest {}))
        .await?
        .into_inner();
    for p in &chips.chips {
        println!("  seat {} {:6}  chips={}", p.seat, p.player_name, p.chips);
    }

    // ── 8. Table status ───────────────────────────────────────────────────────
    section("TABLE STATUS");
    let status = client
        .get_status(Request::new(GetStatusRequest {}))
        .await?
        .into_inner()
        .status
        .unwrap_or_default();
    println!("  hand_in_progress : {}", status.hand_in_progress);
    println!("  game_over        : {}", status.game_over);
    println!("  board            : {}", status.board);
    println!("  pot              : {}", status.pot);

    println!();
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn section(title: &str) {
    println!("\n── {title} {}", "─".repeat(50 - title.len() - 4));
}

async fn seat(
    client: &mut DealerServiceClient<tonic::transport::Channel>,
    name: &str,
    chips: u32,
) -> Result<u32, Box<dyn std::error::Error>> {
    let resp = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: name.to_owned(),
            chips,
        }))
        .await?
        .into_inner();
    match resp.result {
        Some(seat_player_response::Result::SeatNumber(n)) => Ok(n),
        Some(seat_player_response::Result::Error(e)) => Err(e.into()),
        None => Err("empty seat_player response".into()),
    }
}
