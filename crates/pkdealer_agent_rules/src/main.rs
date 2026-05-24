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
//! ```
//!
//! ## Supported built-in profile names
//!
//! `gto`, `tight_passive` (`tp`), `loose_aggressive` (`lag`),
//! `tight_aggressive` (`tag`), `loose_passive` (`lp`), `maniac`, `abc`,
//! `short_stack_ninja` (`ssn`), `joker`
//!
//! ## Environment variables
//!
//! | Variable | Default | Purpose |
//! |----------|---------|---------|
//! | `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | Override `--endpoint` |

use std::process;

use async_trait::async_trait;
use clap::Parser;
use pkcore::bot::decider::{BotDecider, RuleBasedDecider};
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

/// Converts a [`HandState`] into a pkcore [`TableSnapshot`].
///
/// `current_bet` is approximated as `to_call` and `min_raise` as `big_blind`;
/// the runner's floor-raise correction handles any undersized raise amounts.
/// `board` and `hole_cards` are left empty because [`RuleBasedDecider`] does
/// not use them for its probabilistic decisions.
fn hand_state_to_snapshot(state: &HandState) -> TableSnapshot<'static> {
    let stacks: Vec<SeatInfo> = state
        .stacks
        .iter()
        .map(|(seat, name, chips)| SeatInfo {
            id: Uuid::new_v4(),
            seat: *seat,
            name: name.clone(),
            #[allow(clippy::cast_possible_truncation)]
            chips: *chips as usize,
            bet: 0,
            is_active: true,
        })
        .collect();

    let phase = match state.street.as_str() {
        "flop" => GamePhase::BettingFlop,
        "turn" => GamePhase::BettingTurn,
        "river" => GamePhase::BettingRiver,
        _ => GamePhase::BettingPreFlop,
    };

    let seat_count = u8::try_from(stacks.len()).unwrap_or(u8::MAX);

    #[allow(clippy::cast_possible_truncation)]
    TableSnapshot {
        seat: state.seat,
        phase,
        board: Cards::default(),
        hole_cards: Cards::default(),
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
        dealer_button: None,
        seat_count,
        logical_seat: None,
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
/// `short_stack_ninja` / `ssn`, `joker`. Any other value is treated as a path
/// to a YAML file.
///
/// # Errors
///
/// Returns an error when the spec is not a known name and the file cannot be
/// read or parsed.
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

    let profile = match load_profile(&args.profile) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to load profile {:?}: {e}", args.profile);
            process::exit(1);
        }
    };
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

    fn sample_state() -> HandState {
        HandState {
            seat: 0,
            hole_cards: "Ah Kd".to_string(),
            board: String::new(),
            pot: 300,
            to_call: 100,
            my_chips: 9_900,
            stacks: vec![
                (0, "alice".to_string(), 9_900),
                (1, "bob".to_string(), 10_000),
            ],
            big_blind: 100,
            street: "preflop".to_string(),
            action_history: vec![],
        }
    }

    #[test]
    fn test_hand_state_to_snapshot_numeric_fields() {
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
    fn test_hand_state_to_snapshot_stacks() {
        let state = sample_state();
        let snap = hand_state_to_snapshot(&state);
        assert_eq!(snap.stacks.len(), 2);
        assert_eq!(snap.stacks[0].name, "alice");
        assert_eq!(snap.stacks[1].chips, 10_000);
    }

    #[test]
    fn test_hand_state_to_snapshot_phase_preflop() {
        let snap = hand_state_to_snapshot(&sample_state());
        assert_eq!(snap.phase, GamePhase::BettingPreFlop);
    }

    #[test]
    fn test_hand_state_to_snapshot_phase_flop() {
        let state = HandState {
            street: "flop".to_string(),
            ..sample_state()
        };
        assert_eq!(hand_state_to_snapshot(&state).phase, GamePhase::BettingFlop);
    }

    #[test]
    fn test_hand_state_to_snapshot_phase_turn() {
        let state = HandState {
            street: "turn".to_string(),
            ..sample_state()
        };
        assert_eq!(hand_state_to_snapshot(&state).phase, GamePhase::BettingTurn);
    }

    #[test]
    fn test_hand_state_to_snapshot_phase_river() {
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
    fn test_hand_state_to_snapshot_empty_board_is_default_cards() {
        let snap = hand_state_to_snapshot(&sample_state());
        assert!(snap.board.is_empty());
        assert!(snap.hole_cards.is_empty());
    }

    #[test]
    fn test_pkcore_action_to_decision_fold() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Fold),
            Decision::Fold
        );
    }

    #[test]
    fn test_pkcore_action_to_decision_check() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Check),
            Decision::Check
        );
    }

    #[test]
    fn test_pkcore_action_to_decision_call() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Call),
            Decision::Call
        );
    }

    #[test]
    fn test_pkcore_action_to_decision_bet() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Bet(200)),
            Decision::Bet(200)
        );
    }

    #[test]
    fn test_pkcore_action_to_decision_raise() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::Raise(400)),
            Decision::Raise(400)
        );
    }

    #[test]
    fn test_pkcore_action_to_decision_all_in() {
        assert_eq!(
            pkcore_action_to_decision(PkcoreAction::AllIn),
            Decision::AllIn
        );
    }

    #[test]
    fn test_load_profile_builtin_gto() {
        let p = load_profile("gto").expect("gto should load");
        assert_eq!(p.name, "gto");
    }

    #[test]
    fn test_load_profile_builtin_abbreviations() {
        assert_eq!(load_profile("tp").expect("tp").name, "tight_passive");
        assert_eq!(load_profile("lag").expect("lag").name, "loose_aggressive");
        assert_eq!(load_profile("tag").expect("tag").name, "tight_aggressive");
        assert_eq!(load_profile("lp").expect("lp").name, "loose_passive");
        assert_eq!(load_profile("ssn").expect("ssn").name, "short_stack_ninja");
    }

    #[test]
    fn test_load_profile_all_builtins() {
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
        ] {
            let p = load_profile(name).unwrap_or_else(|e| panic!("failed to load {name}: {e}"));
            assert!(!p.name.is_empty(), "{name} profile has empty name");
        }
    }

    #[test]
    fn test_load_profile_unknown_path_returns_error() {
        let result = load_profile("nonexistent_profile_xyz.yaml");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rules_agent_legal_action_with_to_call() {
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
    async fn test_rules_agent_legal_action_without_to_call() {
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
    async fn test_rules_agent_zero_chips_checks() {
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
    async fn test_rules_agent_all_profiles_produce_actions() {
        for profile in BotProfile::default_profiles() {
            let agent = RulesAgent { profile };
            let _ = agent.decide(&sample_state()).await;
        }
    }

    #[test]
    fn test_args_defaults() {
        let args =
            Args::try_parse_from(["pkdealer_agent_rules"]).expect("default args should parse");
        assert_eq!(args.endpoint, "http://127.0.0.1:50051");
        assert_eq!(args.name, "rules");
        assert!(args.seat.is_none());
        assert_eq!(args.chips, 10_000);
        assert_eq!(args.profile, "gto");
        assert!(args.client_secret.is_empty());
    }

    #[test]
    fn test_args_with_profile_name() {
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
    fn test_args_with_seat_and_chips() {
        let args = Args::try_parse_from(["pkdealer_agent_rules", "--seat", "2", "--chips", "5000"])
            .expect("seat/chips args should parse");
        assert_eq!(args.seat, Some(2));
        assert_eq!(args.chips, 5_000);
    }
}
