#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! # `pkdealer_agent_rules`
//!
//! A rule-based poker bot that drives decisions through a pkcore [`BotProfile`].
//! The profile controls aggression, bluff frequency, and bet sizing; the bot
//! uses [`RuleBasedDecider`] to translate profile parameters into concrete
//! poker actions.
//!
//! ## Usage
//!
//! ```text
//! # Named built-in profile (default: gto)
//! cargo run --bin pkdealer_agent_rules -- --name alice --profile gto
//!
//! # YAML file on disk
//! cargo run --bin pkdealer_agent_rules -- --name bob --profile data/bots/loose_aggressive.yaml
//!
//! # Override individual pkcore 0.3.0 (EPIC-36) decision-capability knobs on top
//! # of any profile — here: Monte-Carlo equity, position-aware ranges, draw outs.
//! cargo run --bin pkdealer_agent_rules -- --name carol --profile gto \
//!     --equity fast --equity-samples 4000 --ranges position-aware --outs on
//! ```
//!
//! ## Supported built-in profile names
//!
//! `gto`, `tight_passive` (`tp`), `loose_aggressive` (`lag`),
//! `tight_aggressive` (`tag`), `loose_passive` (`lp`), `maniac`, `abc`,
//! `short_stack_ninja` (`ssn`), `joker`, `strong_all_on` (`strong`),
//! `weak_all_off` (`weak`)
//!
//! `strong_all_on` and `weak_all_off` are the EPIC-36 reference profiles: the
//! same `gto` base with every graded [`pkcore::bot::decision_config::DecisionConfig`]
//! knob dialed all the way
//! up (exact-ish equity, position-aware ranges, strict pot odds) versus all the
//! way down (hand-rank proxy, flat ranges, pot odds ignored).
//!
//! ## Decision-capability overrides (EPIC-36)
//!
//! Each flag below overrides one knob of the loaded profile's
//! [`pkcore::bot::decision_config::DecisionConfig`]. Omitting a flag leaves that
//! knob at whatever the profile specified (its own `decision:` section, or the
//! historical default).
//!
//! | Flag | Values | Knob |
//! |------|--------|------|
//! | `--equity` | `off`, `fast`, `exact` | Postflop hand-strength source |
//! | `--equity-samples` | `u32` (default `2000`) | Monte-Carlo budget for `--equity fast` |
//! | `--ranges` | `flat`, `position-aware` | Preflop range source |
//! | `--pot-odds-discipline` | `0.0`–`1.0` | Call-threshold strictness |
//! | `--outs` | `off`, `on` | Draw/outs equity augmentation |
//! | `--exploit` | `off`, `light`, `heavy` | Opponent-adjusted exploitation |
//! | `--preflop-charts` | `off`, `hup`, `solver` | Preflop decision-chart source |
//!
//! ## Environment variables
//!
//! | Variable | Default | Purpose |
//! |----------|---------|---------|
//! | `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | Override `--endpoint` |

use std::process;

use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use pkcore::Forgiving;
use pkcore::bot::decider::{BotDecider, RuleBasedDecider};
use pkcore::bot::decision_config::{
    EquityMode, ExploitMode, PotOddsConfig, PreflopCharts, RangeMode, Toggle,
};
use pkcore::bot::player_action::PlayerAction as PkcoreAction;
use pkcore::bot::profile::BotProfile;
use pkcore::bot::table_snapshot::{SeatInfo, TableSnapshot};
use pkcore::cards::Cards;
use pkcore::games::GamePhase;
use pkcore::games::betting_structure::{BetTier, BettingStructure};
use pkdealer_agent_core::{AgentConfig, Decision, HandState, PokerAgent, run_agent};
use uuid::Uuid;

/// Rule-based poker bot connected to a pkdealer gRPC service.
#[derive(Debug, Parser)]
#[command(
    name = "pkdealer_agent_rules",
    about = "Rule-based poker agent driven by a pkcore BotProfile"
)]
struct Args {
    /// gRPC service address.
    #[arg(
        long,
        env = "PKDEALER_ENDPOINT",
        default_value = "http://127.0.0.1:50051"
    )]
    endpoint: String,

    /// Player name displayed at the table.
    #[arg(long, default_value = "rules")]
    name: String,

    /// Optional specific seat number (0–8). Omit to take the next available seat.
    #[arg(long)]
    seat: Option<u32>,

    /// Buy-in chip count.
    #[arg(long, default_value_t = 10_000)]
    chips: u32,

    /// Opaque seat-resume token. Empty (default) disables resume.
    #[arg(long, default_value = "")]
    client_secret: String,

    /// Profile name (`gto`, `tight_passive`, `lag`, …) or path to a YAML file.
    #[arg(long, default_value = "gto")]
    profile: String,

    /// Override the postflop equity source. Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    equity: Option<EquityArg>,

    /// Monte-Carlo sample budget used when `--equity fast` is selected.
    #[arg(long, default_value_t = 2_000)]
    equity_samples: u32,

    /// Override the preflop range source. Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    ranges: Option<RangesArg>,

    /// Override pot-odds discipline in `[0.0, 1.0]` (1.0 = strict break-even,
    /// 0.0 = ignore pot odds). Out-of-range values are clamped.
    #[arg(long)]
    pot_odds_discipline: Option<f64>,

    /// Override draw/outs equity augmentation. Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    outs: Option<ToggleArg>,

    /// Override opponent-adjusted exploitation. Engages only when the table
    /// snapshot carries opponent stats, so it is a no-op on the current
    /// pkdealer wire (which supplies none). Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    exploit: Option<ExploitArg>,

    /// Override the preflop decision-chart source. Omit to keep the profile's setting.
    #[arg(long, value_enum)]
    preflop_charts: Option<PreflopChartsArg>,
}

/// CLI mirror of [`EquityMode`] (the `samples` budget comes from `--equity-samples`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum EquityArg {
    /// Hand-rank proxy — the historical behavior.
    Off,
    /// Seeded Monte Carlo with the `--equity-samples` budget.
    Fast,
    /// Exact enumeration of remaining runouts.
    Exact,
}

/// CLI mirror of [`RangeMode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum RangesArg {
    /// Flat `range_strategy.open_raise` lookup — the historical behavior.
    Flat,
    /// Position-aware lookup via the profile's playbook.
    PositionAware,
}

/// CLI mirror of [`Toggle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ToggleArg {
    /// Capability disabled — the historical behavior.
    Off,
    /// Capability enabled.
    On,
}

/// CLI mirror of [`ExploitMode`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ExploitArg {
    /// No opponent adjustment — the historical behavior.
    Off,
    /// Light adjustment (higher sample gate before adjusting).
    Light,
    /// Heavy adjustment (lower sample gate; adjusts sooner).
    Heavy,
}

/// CLI mirror of [`PreflopCharts`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum PreflopChartsArg {
    /// No chart — the historical range-membership behavior.
    Off,
    /// Heads-up precomputed odds table.
    Hup,
    /// Offline-generated GTO charts.
    Solver,
}

/// Applies any `--equity` / `--ranges` / … CLI overrides onto a loaded
/// profile's [`pkcore::bot::decision_config::DecisionConfig`], leaving each knob
/// untouched when its flag was omitted.
///
/// Returns `true` when at least one knob was overridden, so the caller can log
/// the change.
///
/// # Examples
///
/// ```text
/// # gto profile, but force Monte-Carlo equity and looser pot-odds discipline:
/// pkdealer_agent_rules --profile gto --equity fast --pot-odds-discipline 0.5
/// ```
fn apply_decision_overrides(profile: &mut BotProfile, args: &Args) -> bool {
    let mut changed = false;
    if let Some(equity) = args.equity {
        profile.decision.equity = match equity {
            EquityArg::Off => EquityMode::Off,
            EquityArg::Fast => EquityMode::Fast {
                samples: args.equity_samples,
            },
            EquityArg::Exact => EquityMode::Exact,
        };
        changed = true;
    }
    if let Some(ranges) = args.ranges {
        profile.decision.ranges = match ranges {
            RangesArg::Flat => RangeMode::Flat,
            RangesArg::PositionAware => RangeMode::PositionAware,
        };
        changed = true;
    }
    if let Some(discipline) = args.pot_odds_discipline {
        profile.decision.pot_odds = PotOddsConfig {
            discipline: discipline.clamp(0.0, 1.0),
        };
        changed = true;
    }
    if let Some(outs) = args.outs {
        profile.decision.outs = match outs {
            ToggleArg::Off => Toggle::Off,
            ToggleArg::On => Toggle::On,
        };
        changed = true;
    }
    if let Some(exploit) = args.exploit {
        profile.decision.exploit = match exploit {
            ExploitArg::Off => ExploitMode::Off,
            ExploitArg::Light => ExploitMode::Light,
            ExploitArg::Heavy => ExploitMode::Heavy,
        };
        changed = true;
    }
    if let Some(charts) = args.preflop_charts {
        profile.decision.preflop_charts = match charts {
            PreflopChartsArg::Off => PreflopCharts::Off,
            PreflopChartsArg::Hup => PreflopCharts::Hup,
            PreflopChartsArg::Solver => PreflopCharts::Solver,
        };
        changed = true;
    }
    changed
}

struct RulesAgent {
    profile: BotProfile,
}

#[async_trait]
impl PokerAgent for RulesAgent {
    /// Converts the gRPC-derived [`HandState`] into a pkcore [`TableSnapshot`]
    /// and delegates the decision to [`RuleBasedDecider`].
    async fn decide(&self, state: &HandState) -> Decision {
        let snapshot = hand_state_to_snapshot(state);
        pkcore_action_to_decision(RuleBasedDecider.decide(&self.profile, &snapshot))
    }
}

/// Returns the logical position (0-based index) of `seat` within the sorted
/// list of occupied `seats`, or `None` when `seat` is not occupied.
///
/// pkcore's [`TableSnapshot::position`] expresses `dealer_button` and
/// `logical_seat` as indices into the sorted occupied-seat list (`0` = earliest
/// occupied seat), *not* as raw seat numbers, so raw seat numbers must be
/// projected through this before they are stored on the snapshot.
fn logical_index(sorted_seats: &[u8], seat: u8) -> Option<u8> {
    sorted_seats
        .iter()
        .position(|&s| s == seat)
        .and_then(|i| u8::try_from(i).ok())
}

/// Converts a [`HandState`] into a pkcore [`TableSnapshot`].
///
/// `current_bet` is approximated as `to_call` and `min_raise` as `big_blind`;
/// the runner's floor-raise correction handles any undersized raise amounts.
///
/// Each seat's `bet` and `is_active` flags are carried straight through from the
/// [`HandState`]. `is_active` matters for the equity path: pkcore counts *live*
/// opponents (folded / busted seats excluded) when it estimates multi-way
/// equity, so passing the real flag keeps `--equity fast|exact` from
/// systematically understating the hero's equity.
///
/// `dealer_button` and `logical_seat` are derived from `state.button_seat` and
/// `state.seat` by projecting both raw seat numbers into logical positions over
/// the sorted occupied-seat list (see [`logical_index`]). This makes
/// `TableSnapshot::position` resolve, which is what `ranges: position_aware`
/// consults. When `button_seat` is `None` (no button known), `dealer_button`
/// stays `None` and the decider falls back to the flat range.
///
/// `hole_cards` and `board` are parsed from their space-separated index
/// notation (e.g. `"Ah Kd"`) so that [`RuleBasedDecider`] runs its
/// hand-strength *equity* path. When a card field is empty (e.g. `board`
/// before the flop) or contains an unparseable token, [`Cards::forgiving_from_str`]
/// yields what it can — an empty [`Cards`] in the empty case — and the decider
/// falls back to its aggression-factor path for that decision.
fn hand_state_to_snapshot(state: &HandState) -> TableSnapshot<'static> {
    let stacks: Vec<SeatInfo> = state
        .stacks
        .iter()
        .map(|s| SeatInfo {
            id: Uuid::new_v4(),
            seat: s.seat,
            name: s.name.clone(),
            #[allow(clippy::cast_possible_truncation)]
            chips: s.chips as usize,
            #[allow(clippy::cast_possible_truncation)]
            bet: s.bet as usize,
            is_active: s.is_active,
        })
        .collect();

    let phase = match state.street.as_str() {
        "flop" => GamePhase::BettingFlop,
        "turn" => GamePhase::BettingTurn,
        "river" => GamePhase::BettingRiver,
        _ => GamePhase::BettingPreFlop,
    };

    let seat_count = u8::try_from(stacks.len()).unwrap_or(u8::MAX);

    // Logical positions index into the sorted list of occupied seats, which is
    // what `TableSnapshot::position()` expects — not raw seat numbers.
    let mut occupied: Vec<u8> = state.stacks.iter().map(|s| s.seat).collect();
    occupied.sort_unstable();
    let logical_seat = logical_index(&occupied, state.seat);
    let dealer_button = state
        .button_seat
        .and_then(|button| logical_index(&occupied, button));

    #[allow(clippy::cast_possible_truncation)]
    TableSnapshot {
        seat: state.seat,
        phase,
        board: Cards::forgiving_from_str(&state.board),
        hole_cards: Cards::forgiving_from_str(&state.hole_cards),
        pot: state.pot as usize,
        to_call: state.to_call as usize,
        current_bet: state.to_call as usize,
        min_raise: state.big_blind as usize,
        my_chips: state.my_chips as usize,
        stacks,
        big_blind: state.big_blind as usize,
        betting_structure: BettingStructure::default(),
        bet_tier: BetTier::default(),
        checked_this_street: false,
        dealer_button,
        seat_count,
        logical_seat,
        opponent_stats: None,
    }
}

/// Converts a pkcore [`PkcoreAction`] into an agent-core [`Decision`].
///
/// Bet and raise amounts are cast from `usize` to `u32`. In practice they
/// are bounded by the chip counts that originated as `u32` values, so no
/// truncation occurs.
#[allow(clippy::cast_possible_truncation)]
fn pkcore_action_to_decision(action: PkcoreAction) -> Decision {
    match action {
        PkcoreAction::Fold => Decision::Fold,
        PkcoreAction::Check => Decision::Check,
        PkcoreAction::Call => Decision::Call,
        PkcoreAction::Bet(n) => Decision::Bet(n as u32),
        PkcoreAction::Raise(n) => Decision::Raise(n as u32),
        PkcoreAction::AllIn => Decision::AllIn,
    }
}

/// Resolves a [`BotProfile`] from a short name or a YAML file path.
///
/// Recognized short names: `gto`, `tight_passive` / `tp`, `loose_aggressive` /
/// `lag`, `tight_aggressive` / `tag`, `loose_passive` / `lp`, `maniac`, `abc`,
/// `short_stack_ninja` / `ssn`, `joker`, `strong_all_on` / `strong`,
/// `weak_all_off` / `weak`. Any other value is treated as a path to a YAML file.
///
/// The `strong_all_on` / `weak_all_off` reference profiles are embedded into the
/// binary via [`include_str!`], so they resolve regardless of the working
/// directory (unlike a bare file path).
///
/// # Errors
///
/// Returns an error when the spec is not a known name and the file cannot be
/// read or parsed, or when an embedded reference profile fails to deserialize.
///
/// # Examples
///
/// ```
/// let p = pkdealer_agent_rules_load_profile("gto").unwrap();
/// ```
fn load_profile(spec: &str) -> Result<BotProfile, Box<dyn std::error::Error>> {
    let built_in = match spec {
        "gto" => Some(BotProfile::gto()),
        "tight_passive" | "tp" => Some(BotProfile::tight_passive()),
        "loose_aggressive" | "lag" => Some(BotProfile::loose_aggressive()),
        "tight_aggressive" | "tag" => Some(BotProfile::tight_aggressive()),
        "loose_passive" | "lp" => Some(BotProfile::loose_passive()),
        "maniac" => Some(BotProfile::maniac()),
        "abc" => Some(BotProfile::abc()),
        "short_stack_ninja" | "ssn" => Some(BotProfile::short_stack_ninja()),
        "joker" => Some(BotProfile::joker()),
        "strong_all_on" | "strong" => Some(BotProfile::from_yaml_str(include_str!(
            "../../../data/bots/strong_all_on.yaml"
        ))?),
        "weak_all_off" | "weak" => Some(BotProfile::from_yaml_str(include_str!(
            "../../../data/bots/weak_all_off.yaml"
        ))?),
        _ => None,
    };
    if let Some(p) = built_in {
        return Ok(p);
    }
    Ok(BotProfile::from_file(spec)?)
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let mut profile = match load_profile(&args.profile) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to load profile {:?}: {e}", args.profile);
            process::exit(1);
        }
    };
    if apply_decision_overrides(&mut profile, &args) {
        eprintln!(
            "[{}] decision overrides applied: {:?}",
            args.name, profile.decision
        );
    }
    eprintln!("[{}] profile: {profile}", args.name);

    let config = AgentConfig {
        endpoint: args.endpoint,
        name: args.name,
        seat: args.seat,
        chips: args.chips,
        client_secret: args.client_secret,
    };

    if let Err(e) = run_agent(RulesAgent { profile }, config).await {
        eprintln!("Agent error: {e}");
        process::exit(1);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use pkdealer_agent_core::SeatSnapshot;

    fn sample_state() -> HandState {
        HandState {
            seat: 0,
            hole_cards: "Ah Kd".to_string(),
            board: String::new(),
            pot: 300,
            to_call: 100,
            my_chips: 9_900,
            stacks: vec![
                SeatSnapshot {
                    seat: 0,
                    name: "alice".to_string(),
                    chips: 9_900,
                    bet: 100,
                    is_active: true,
                },
                SeatSnapshot {
                    seat: 1,
                    name: "bob".to_string(),
                    chips: 10_000,
                    bet: 0,
                    is_active: true,
                },
            ],
            big_blind: 100,
            street: "preflop".to_string(),
            action_history: vec![],
            button_seat: Some(0),
        }
    }

    #[test]
    fn hand_state_to_snapshot_numeric_fields() {
        let state = sample_state();
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.seat, 0);
        assert_eq!(snap.pot, 300);
        assert_eq!(snap.to_call, 100);
        assert_eq!(snap.my_chips, 9_900);
        assert_eq!(snap.big_blind, 100);
        assert_eq!(snap.min_raise, 100);
    }

    #[test]
    fn logical_index_found_and_missing() {
        let occupied = [2u8, 4, 5, 7];
        assert_eq!(logical_index(&occupied, 2), Some(0));
        assert_eq!(logical_index(&occupied, 5), Some(2));
        assert_eq!(logical_index(&occupied, 7), Some(3));
        assert_eq!(logical_index(&occupied, 3), None);
    }

    #[test]
    fn hand_state_to_snapshot_threads_bet_and_active() {
        // Seat 1 has folded; seat 0 has 100 in the pot this street.
        let state = HandState {
            stacks: vec![
                SeatSnapshot {
                    seat: 0,
                    name: "alice".to_string(),
                    chips: 9_900,
                    bet: 100,
                    is_active: true,
                },
                SeatSnapshot {
                    seat: 1,
                    name: "bob".to_string(),
                    chips: 10_000,
                    bet: 0,
                    is_active: false,
                },
            ],
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.stacks[0].bet, 100);
        assert!(snap.stacks[0].is_active);
        assert_eq!(snap.stacks[1].bet, 0);
        assert!(!snap.stacks[1].is_active, "folded seat must not be active");
    }

    #[test]
    fn hand_state_to_snapshot_derives_logical_positions() {
        // Non-contiguous occupied seats: 3 (hero) and 6 (button).
        let state = HandState {
            seat: 3,
            button_seat: Some(6),
            stacks: vec![
                SeatSnapshot {
                    seat: 3,
                    name: "hero".to_string(),
                    chips: 9_000,
                    bet: 0,
                    is_active: true,
                },
                SeatSnapshot {
                    seat: 6,
                    name: "villain".to_string(),
                    chips: 9_000,
                    bet: 0,
                    is_active: true,
                },
            ],
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        // Sorted occupied = [3, 6]: hero is logical 0, button is logical 1.
        assert_eq!(snap.logical_seat, Some(0));
        assert_eq!(snap.dealer_button, Some(1));
        // With both set, position() resolves (enables position_aware ranges).
        assert!(snap.position().is_some());
    }

    #[test]
    fn hand_state_to_snapshot_no_button_leaves_position_unresolved() {
        let state = HandState {
            button_seat: None,
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.dealer_button, None);
        assert!(snap.position().is_none());
    }

    #[test]
    fn hand_state_to_snapshot_stacks() {
        let state = sample_state();
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.stacks.len(), 2);
        assert_eq!(snap.stacks[0].name, "alice");
        assert_eq!(snap.stacks[1].chips, 10_000);
    }

    #[test]
    fn hand_state_to_snapshot_phase_preflop() {
        let snap = hand_state_to_snapshot(&sample_state());
        assert_eq!(snap.phase, GamePhase::BettingPreFlop);
    }

    #[test]
    fn hand_state_to_snapshot_phase_flop() {
        let state = HandState {
            street: "flop".to_string(),
            ..sample_state()
        };
        assert_eq!(hand_state_to_snapshot(&state).phase, GamePhase::BettingFlop);
    }

    #[test]
    fn hand_state_to_snapshot_phase_turn() {
        let state = HandState {
            street: "turn".to_string(),
            ..sample_state()
        };
        assert_eq!(hand_state_to_snapshot(&state).phase, GamePhase::BettingTurn);
    }

    #[test]
    fn hand_state_to_snapshot_phase_river() {
        let state = HandState {
            street: "river".to_string(),
            ..sample_state()
        };
        assert_eq!(
            hand_state_to_snapshot(&state).phase,
            GamePhase::BettingRiver
        );
    }

    #[test]
    fn hand_state_to_snapshot_empty_board_is_default_cards() {
        let snap = hand_state_to_snapshot(&sample_state());
        assert!(snap.board.is_empty());
    }

    #[test]
    fn hand_state_to_snapshot_parses_hole_cards() {
        // sample_state() has hole_cards "Ah Kd"; the snapshot must carry both
        // cards so RuleBasedDecider runs its equity path rather than the
        // card-blind fallback path.
        let snap = hand_state_to_snapshot(&sample_state());
        assert_eq!(snap.hole_cards.len(), 2);
    }

    #[test]
    fn hand_state_to_snapshot_parses_board() {
        let state = HandState {
            board: "Ts 9s 8c".to_string(),
            street: "flop".to_string(),
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.board.len(), 3);
    }

    #[test]
    fn hand_state_to_snapshot_empty_board_stays_empty() {
        let state = HandState {
            board: String::new(),
            street: "preflop".to_string(),
            ..sample_state()
        };
        let snap = hand_state_to_snapshot(&state);
        assert!(snap.board.is_empty());
    }

    #[test]
    fn pkcore_action_to_decision_fold() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Fold),
            Decision::Fold
        );
    }

    #[test]
    fn pkcore_action_to_decision_check() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Check),
            Decision::Check
        );
    }

    #[test]
    fn pkcore_action_to_decision_call() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Call),
            Decision::Call
        );
    }

    #[test]
    fn pkcore_action_to_decision_bet() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Bet(200)),
            Decision::Bet(200)
        );
    }

    #[test]
    fn pkcore_action_to_decision_raise() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Raise(400)),
            Decision::Raise(400)
        );
    }

    #[test]
    fn pkcore_action_to_decision_all_in() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::AllIn),
            Decision::AllIn
        );
    }

    #[test]
    fn load_profile_builtin_gto() {
        let p = load_profile("gto").expect("gto should load");
        assert_eq!(p.name, "gto");
    }

    #[test]
    fn load_profile_builtin_abbreviations() {
        assert_eq!(load_profile("tp").expect("tp").name, "tight_passive");
        assert_eq!(load_profile("lag").expect("lag").name, "loose_aggressive");
        assert_eq!(load_profile("tag").expect("tag").name, "tight_aggressive");
        assert_eq!(load_profile("lp").expect("lp").name, "loose_passive");
        assert_eq!(load_profile("ssn").expect("ssn").name, "short_stack_ninja");
    }

    #[test]
    fn load_profile_all_builtins() {
        for name in [
            "gto",
            "tight_passive",
            "loose_aggressive",
            "tight_aggressive",
            "loose_passive",
            "maniac",
            "abc",
            "short_stack_ninja",
            "joker",
            "strong_all_on",
            "weak_all_off",
        ] {
            let p = load_profile(name).unwrap_or_else(|e| panic!("failed to load {name}: {e}"));
            assert!(!p.name.is_empty(), "{name} profile has empty name");
        }
    }

    #[test]
    fn load_profile_unknown_path_returns_error() {
        let result = load_profile("nonexistent_profile_xyz.yaml");
        assert!(result.is_err());
    }

    #[test]
    fn load_profile_strong_all_on_has_decision_knobs_on() {
        let p = load_profile("strong_all_on").expect("strong_all_on should load");
        assert_eq!(p.name, "strong_all_on");
        assert_eq!(p.decision.ranges, RangeMode::PositionAware);
        assert!(matches!(p.decision.equity, EquityMode::Fast { .. }));
        assert!(!p.decision.is_default());
    }

    #[test]
    fn load_profile_weak_all_off_has_default_decision() {
        let p = load_profile("weak_all_off").expect("weak_all_off should load");
        assert_eq!(p.name, "weak_all_off");
        assert_eq!(p.decision.equity, EquityMode::Off);
        assert_eq!(p.decision.ranges, RangeMode::Flat);
        assert!((p.decision.pot_odds.discipline - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn load_profile_strong_weak_aliases() {
        assert_eq!(
            load_profile("strong").expect("strong").name,
            "strong_all_on"
        );
        assert_eq!(load_profile("weak").expect("weak").name, "weak_all_off");
    }

    /// Builds an `Args` from a bare arg list; panics on parse failure.
    fn args_from(argv: &[&str]) -> Args {
        Args::try_parse_from(argv).expect("args should parse")
    }

    #[test]
    fn apply_decision_overrides_none_leaves_profile_unchanged() {
        let mut profile = BotProfile::gto();
        let before = profile.decision.clone();
        let args = args_from(["pkdealer_agent_rules"].as_slice());
        assert!(!apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision, before);
    }

    #[test]
    fn apply_decision_overrides_equity_fast_uses_samples() {
        let mut profile = BotProfile::gto();
        let args = args_from(&[
            "pkdealer_agent_rules",
            "--equity",
            "fast",
            "--equity-samples",
            "4000",
        ]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision.equity, EquityMode::Fast { samples: 4_000 });
    }

    #[test]
    fn apply_decision_overrides_ranges_position_aware() {
        let mut profile = BotProfile::gto();
        let args = args_from(&["pkdealer_agent_rules", "--ranges", "position-aware"]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision.ranges, RangeMode::PositionAware);
    }

    #[test]
    fn apply_decision_overrides_pot_odds_clamped() {
        let mut profile = BotProfile::gto();
        let args = args_from(&["pkdealer_agent_rules", "--pot-odds-discipline", "5.0"]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert!((profile.decision.pot_odds.discipline - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn apply_decision_overrides_all_knobs() {
        let mut profile = BotProfile::gto();
        let args = args_from(&[
            "pkdealer_agent_rules",
            "--equity",
            "exact",
            "--ranges",
            "position-aware",
            "--pot-odds-discipline",
            "0.25",
            "--outs",
            "on",
            "--exploit",
            "heavy",
            "--preflop-charts",
            "hup",
        ]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision.equity, EquityMode::Exact);
        assert_eq!(profile.decision.ranges, RangeMode::PositionAware);
        assert!((profile.decision.pot_odds.discipline - 0.25).abs() < f64::EPSILON);
        assert_eq!(profile.decision.outs, Toggle::On);
        assert_eq!(profile.decision.exploit, ExploitMode::Heavy);
        assert_eq!(profile.decision.preflop_charts, PreflopCharts::Hup);
    }

    #[test]
    fn apply_decision_overrides_can_disable_profile_knobs() {
        // strong_all_on ships with position-aware ranges + Monte-Carlo equity;
        // overrides must be able to force them back off.
        let mut profile = load_profile("strong_all_on").expect("strong_all_on");
        let args = args_from(&[
            "pkdealer_agent_rules",
            "--equity",
            "off",
            "--ranges",
            "flat",
        ]);
        assert!(apply_decision_overrides(&mut profile, &args));
        assert_eq!(profile.decision.equity, EquityMode::Off);
        assert_eq!(profile.decision.ranges, RangeMode::Flat);
    }

    #[tokio::test]
    async fn rules_agent_legal_action_with_to_call() {
        let agent = RulesAgent {
            profile: BotProfile::gto(),
        };
        let state = sample_state();
        let decision = agent.decide(&state).await;
        assert!(
            matches!(
                decision,
                Decision::Fold | Decision::Call | Decision::Raise(_) | Decision::AllIn
            ),
            "unexpected decision when to_call > 0: {decision:?}"
        );
    }

    #[tokio::test]
    async fn rules_agent_legal_action_without_to_call() {
        let agent = RulesAgent {
            profile: BotProfile::gto(),
        };
        let state = HandState {
            to_call: 0,
            ..sample_state()
        };
        let decision = agent.decide(&state).await;
        assert!(
            matches!(decision, Decision::Check | Decision::Bet(_)),
            "unexpected decision when to_call == 0: {decision:?}"
        );
    }

    #[tokio::test]
    async fn rules_agent_zero_chips_checks() {
        let agent = RulesAgent {
            profile: BotProfile::gto(),
        };
        let state = HandState {
            my_chips: 0,
            to_call: 0,
            ..sample_state()
        };
        let decision = agent.decide(&state).await;
        assert_eq!(decision, Decision::Check);
    }

    #[tokio::test]
    async fn rules_agent_all_profiles_produce_actions() {
        for profile in BotProfile::default_profiles() {
            let agent = RulesAgent { profile };
            let _ = agent.decide(&sample_state()).await;
        }
    }

    #[test]
    fn args_defaults() {
        let args =
            Args::try_parse_from(["pkdealer_agent_rules"]).expect("default args should parse");
        assert_eq!(args.endpoint, "http://127.0.0.1:50051");
        assert_eq!(args.name, "rules");
        assert!(args.seat.is_none());
        assert_eq!(args.chips, 10_000);
        assert_eq!(args.profile, "gto");
        assert!(args.client_secret.is_empty());
        // EPIC-36 override knobs default to "leave the profile alone".
        assert!(args.equity.is_none());
        assert_eq!(args.equity_samples, 2_000);
        assert!(args.ranges.is_none());
        assert!(args.pot_odds_discipline.is_none());
        assert!(args.outs.is_none());
        assert!(args.exploit.is_none());
        assert!(args.preflop_charts.is_none());
    }

    #[test]
    fn args_parse_decision_knobs() {
        let args = Args::try_parse_from([
            "pkdealer_agent_rules",
            "--equity",
            "fast",
            "--ranges",
            "position-aware",
            "--exploit",
            "light",
            "--preflop-charts",
            "solver",
        ])
        .expect("decision-knob args should parse");
        assert_eq!(args.equity, Some(EquityArg::Fast));
        assert_eq!(args.ranges, Some(RangesArg::PositionAware));
        assert_eq!(args.exploit, Some(ExploitArg::Light));
        assert_eq!(args.preflop_charts, Some(PreflopChartsArg::Solver));
    }

    #[test]
    fn args_with_profile_name() {
        let args = Args::try_parse_from([
            "pkdealer_agent_rules",
            "--name",
            "gto-bot",
            "--profile",
            "tight_passive",
        ])
        .expect("named args should parse");
        assert_eq!(args.name, "gto-bot");
        assert_eq!(args.profile, "tight_passive");
    }

    #[test]
    fn args_with_seat_and_chips() {
        let args = Args::try_parse_from(["pkdealer_agent_rules", "--seat", "2", "--chips", "5000"])
            .expect("seat/chips args should parse");
        assert_eq!(args.seat, Some(2));
        assert_eq!(args.chips, 5_000);
    }
}
