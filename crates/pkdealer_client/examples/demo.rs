//! # pkdealer bot arena
//!
//! A self-contained bot-vs-bot tournament that mirrors the pkarena0-web game
//! loop, run entirely through the pkcore session layer (no gRPC required).
//!
//! Nine bot profiles are shuffled and seated.  Hands are driven by the
//! EPIC-20 `next_step()` API: the loop handles `PlayerToAct`, `StreetAdvanced`,
//! and `HandComplete` one step at a time, with each bot deciding via
//! `BotProfile::decide()`.  Play continues until only one player has chips.
//!
//! Each completed hand is recorded as a [`HandHistory`] and the full session
//! is saved as `generated/demo_<timestamp>.yaml` on exit.
//!
//! Run:
//!
//!   cargo run --example demo -p pkdealer_client

use std::time::{SystemTime, UNIX_EPOCH};

use pkcore::PKError;
use pkcore::bot::profile::BotProfile;
use pkcore::card::Card;
use pkcore::casino::action::PlayerAction;
use pkcore::casino::game::ForcedBets;
use pkcore::casino::session::{PokerSession, SessionStep};
use pkcore::casino::table_no_cell::{PlayerNoCell, SeatNoCell, SeatsNoCell, TableNoCell};
use pkcore::hand_history::{HandCollection, HandHistory};
use rand::SeedableRng;
use rand::rngs::SmallRng;
use rand::seq::SliceRandom;

const STARTING_CHIPS: usize = 10_000;
const SMALL_BLIND: usize = 50;
const BIG_BLIND: usize = 100;

fn main() {
    let mut rng = SmallRng::from_os_rng();

    // ── Build a shuffled lineup of 9 bots ─────────────────────────────────────
    let mut pool = BotProfile::default_profiles();
    pool.push(BotProfile::joker());
    pool.shuffle(&mut rng);
    let bots: Vec<BotProfile> = pool.into_iter().take(9).collect();

    section("BOT LINEUP");
    for (i, bot) in bots.iter().enumerate() {
        println!("  seat {i}  {}", bot.name);
    }

    // ── Seat all bots ─────────────────────────────────────────────────────────
    let seats_vec: Vec<SeatNoCell> = bots
        .iter()
        .map(|b| SeatNoCell::new(PlayerNoCell::new_with_chips(b.name.clone(), STARTING_CHIPS)))
        .collect();

    let table = TableNoCell::nlh_from_seats(
        SeatsNoCell::new(seats_vec),
        ForcedBets::new(SMALL_BLIND, BIG_BLIND),
    );
    let mut session = PokerSession::new(table);
    let mut collection = HandCollection::new();

    // ── Tournament loop ───────────────────────────────────────────────────────
    'tournament: loop {
        if session.count_funded() < 2 {
            break;
        }

        // Capture starting chip counts BEFORE start_hand() posts blinds.
        let starting_chips: Vec<(u8, usize)> = (0u8..9)
            .filter_map(|s| {
                session.table.seats.get_seat(s).and_then(|seat| {
                    if seat.is_empty() {
                        None
                    } else {
                        Some((s, seat.player.chips))
                    }
                })
            })
            .collect();

        if session.start_hand().is_err() {
            eprintln!("failed to start hand");
            break;
        }

        section(&format!("HAND #{}", session.hand_number));
        let ts_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut board = String::new();

        // ── Drive the hand one step at a time ─────────────────────────────────
        loop {
            match session.next_step() {
                SessionStep::PlayerToAct(seat) => {
                    let action = bots[seat as usize].decide(&session.table, seat, &mut rng);
                    let label = action_label(&action, &session, seat);
                    let pot = session.table.pot;
                    let name = player_name(&session, seat);
                    println!("  seat {seat} {name:<18}  pot={pot:>6}  {label}");

                    if session.apply_action(seat, action).is_err() {
                        let _ = session.apply_action(seat, PlayerAction::Fold);
                    }
                }

                SessionStep::StreetAdvanced => {
                    let new_board = session.table.board.to_string();
                    if new_board != board {
                        let street = match session.table.board.len() {
                            3 => "FLOP",
                            4 => "TURN",
                            5 => "RIVER",
                            _ => "STREET",
                        };
                        println!("  ── {street}: [{}]", new_board);
                        board = new_board;
                    }
                }

                SessionStep::HandComplete => {
                    // Snapshot everything that end_hand() will reset.
                    let hand_num = session.hand_number as usize;
                    let button = session.table.button;
                    let forced = session.table.forced;
                    let board_str = session.table.board.to_string();
                    let event_log = session.table.event_log.clone();

                    let player_snapshot: Vec<(u8, String, usize, Option<String>)> = (0u8..9)
                        .filter_map(|s| {
                            session.table.seats.get_seat(s).and_then(|seat| {
                                if seat.is_empty() {
                                    return None;
                                }
                                let start = starting_chips
                                    .iter()
                                    .find(|(n, _)| *n == s)
                                    .map_or(0, |(_, c)| *c);
                                let hole = {
                                    let cards: Vec<String> = seat
                                        .cards
                                        .as_slice()
                                        .iter()
                                        .filter(|c| **c != Card::BLANK)
                                        .map(|c| c.to_string())
                                        .collect();
                                    if cards.is_empty() {
                                        None
                                    } else {
                                        Some(cards.join(" "))
                                    }
                                };
                                Some((s, seat.player.handle.clone(), start, hole))
                            })
                        })
                        .collect();

                    let end_result = session.end_hand();

                    // If end_hand() failed before calling reset() internally
                    // (e.g. Fubar — zero active players), the table is still
                    // mid-hand. Force a reset so the next hand can start cleanly.
                    // ChipAuditFailed already called reset() internally, so
                    // is_hand_in_progress() will be false in that case.
                    if session.is_hand_in_progress() {
                        session.table.reset();
                    }

                    // Always clear the event log, success or failure.
                    // pkcore does not auto-clear it, and a stale log would
                    // contaminate the next hand's YAML record.
                    session.table.event_log.clear();

                    match end_result {
                        Ok(ref winnings) => {
                            for pot_win in winnings.vec() {
                                let winners: Vec<String> = (0u8..9)
                                    .filter(|&s| pot_win.equity.seats.contains(s))
                                    .map(|s| player_name(&session, s))
                                    .collect();
                                println!(
                                    "  ── POT ${}: {}",
                                    pot_win.equity.chips,
                                    winners.join(", ")
                                );
                            }

                            // Ending stacks after end_hand() settled chips.
                            let ending_stacks: Vec<(u8, usize)> = (0u8..9)
                                .filter_map(|s| {
                                    session.table.seats.get_seat(s).and_then(|seat| {
                                        if seat.is_empty() {
                                            None
                                        } else {
                                            Some((s, seat.player.chips))
                                        }
                                    })
                                })
                                .collect();

                            let hh = HandHistory::from_table_state(
                                hand_num,
                                ts_secs,
                                button,
                                &forced,
                                &player_snapshot,
                                &board_str,
                                winnings,
                                &event_log,
                                &ending_stacks,
                                "demo",
                            );
                            collection.push(hh);
                        }
                        Err(e) => {
                            eprintln!("  end_hand warning (hand {hand_num} skipped): {e}");
                            // Restore chips to pre-hand state so the pot is not silently
                            // destroyed when reset() cleared it without awarding a winner.
                            for (seat, chips) in &starting_chips {
                                if let Some(s) = session.table.seats.get_seat_mut(*seat) {
                                    s.player.chips = *chips;
                                }
                            }
                            // Stop the demo immediately on a chip audit failure so the
                            // collected hands can be audited to isolate the pkcore bug.
                            if matches!(e, PKError::ChipAuditFailed { .. }) {
                                eprintln!(
                                    "  !! ChipAuditFailed — stopping after {} recorded hands",
                                    collection.len()
                                );
                                break 'tournament;
                            }
                        }
                    }

                    break;
                }
            }
        }

        // ── Eliminate busted players, advance button ───────────────────────────
        session.eliminate_busted();
        session.table.button_up();

        // Print chip counts for surviving players.
        let survivors: Vec<_> = (0u8..9)
            .filter_map(|s| {
                session.table.seats.get_seat(s).and_then(|seat| {
                    if seat.is_empty() {
                        None
                    } else {
                        Some((seat.player.handle.clone(), seat.player.chips))
                    }
                })
            })
            .collect();
        print!("  chips:");
        for (name, chips) in &survivors {
            print!("  {name}=${chips}");
        }
        println!();
    }

    // ── Champion ──────────────────────────────────────────────────────────────
    section("TOURNAMENT RESULT");
    for s in 0u8..9 {
        if let Some(seat) = session.table.seats.get_seat(s)
            && !seat.is_empty()
        {
            println!(
                "  CHAMPION: {} with ${} chips after {} hands",
                seat.player.handle, seat.player.chips, session.hand_number,
            );
        }
    }

    // ── Save hand histories ────────────────────────────────────────────────────
    match collection.save("demo") {
        Ok(path) => println!("  saved: {path}"),
        Err(e) => eprintln!("  failed to save hand histories: {e}"),
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn player_name(session: &PokerSession, seat: u8) -> String {
    session
        .table
        .seats
        .get_seat(seat)
        .map(|s| s.player.handle.clone())
        .unwrap_or_else(|| format!("seat{seat}"))
}

fn action_label(action: &PlayerAction, session: &PokerSession, seat: u8) -> String {
    match action {
        PlayerAction::Fold => "folds".to_string(),
        PlayerAction::Check => "checks".to_string(),
        PlayerAction::Call => {
            let amount = session.table.to_call(seat);
            format!("calls ${amount}")
        }
        PlayerAction::Bet(n) => format!("bets ${n}"),
        PlayerAction::Raise(n) => format!("raises to ${n}"),
        PlayerAction::AllIn => {
            let chips = session
                .table
                .seats
                .get_seat(seat)
                .map_or(0, |s| s.player.chips);
            format!("ALL-IN ${chips}")
        }
    }
}

fn section(title: &str) {
    let dashes = 55usize.saturating_sub(title.len() + 4);
    println!("\n── {title} {}", "─".repeat(dashes));
}
