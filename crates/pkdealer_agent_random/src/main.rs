#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! # `pkdealer_agent_random`
//!
//! A random-action baseline bot that picks legal poker actions uniformly at
//! random. Establishes a performance floor against which rule-based and
//! LLM-driven agents are measured.
//!
//! ## Usage
//!
//! ```text
//! cargo run --bin pkdealer_agent_random -- --name alice
//! ```
//!
//! ## Environment variables
//!
//! | Variable | Default | Purpose |
//! |----------|---------|---------|
//! | `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | Override `--endpoint` |

use std::process;

use async_trait::async_trait;
use clap::Parser;
use pkdealer_agent_core::{AgentConfig, Decision, HandState, PokerAgent, run_agent};
use rand::Rng;

/// Random-action poker bot that connects to a pkdealer gRPC service.
#[derive(Debug, Parser)]
#[command(name = "pkdealer_agent_random", about = "Random baseline poker agent")]
struct Args {
    /// gRPC service address.
    #[arg(
        long,
        env = "PKDEALER_ENDPOINT",
        default_value = "http://127.0.0.1:50051"
    )]
    endpoint: String,

    /// Player name displayed at the table.
    #[arg(long, default_value = "rando")]
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
}

struct RandomAgent;

#[async_trait]
impl PokerAgent for RandomAgent {
    /// Pick a uniformly random legal action.
    ///
    /// - `to_call > 0`: fold / call / raise with equal 1/3 probability each
    /// - `to_call == 0`: check / bet with equal 1/2 probability each
    ///
    /// Raise amounts are a random 25–100% of the pot, clamped to at least
    /// `to_call`. Bet amounts are a random 25–100% of the pot, clamped to at
    /// least `big_blind`.
    async fn decide(&self, state: &HandState) -> Decision {
        let mut rng = rand::rng();
        // Use big_blind as the effective pot floor so raise/bet amounts are
        // always large enough to satisfy the min-raise rule (pot=0 at preflop
        // start before blinds are swept into the pot).
        let effective_pot = state.pot.max(state.big_blind);
        if state.to_call > 0 {
            match rng.random_range(0_u32..3) {
                0 => Decision::Fold,
                1 => Decision::Call,
                _ => {
                    let fraction = u64::from(rng.random_range(25_u32..=100));
                    #[allow(clippy::cast_possible_truncation)] // result ≤ effective_pot ≤ u32::MAX
                    let amount = (u64::from(effective_pot) * fraction / 100) as u32;
                    Decision::Raise(amount.max(state.to_call))
                }
            }
        } else if rng.random_bool(0.5) {
            Decision::Check
        } else {
            let fraction = u64::from(rng.random_range(25_u32..=100));
            #[allow(clippy::cast_possible_truncation)] // result ≤ effective_pot ≤ u32::MAX
            let amount = (u64::from(effective_pot) * fraction / 100) as u32;
            Decision::Bet(amount.max(state.big_blind))
        }
    }
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let config = AgentConfig {
        endpoint: args.endpoint,
        name: args.name,
        seat: args.seat,
        chips: args.chips,
        client_secret: args.client_secret,
    };

    if let Err(e) = run_agent(RandomAgent, config).await {
        eprintln!("Agent error: {e}");
        process::exit(1);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn state_with_call() -> HandState {
        HandState {
            seat: 0,
            hole_cards: "2c 7d".to_string(),
            board: String::new(),
            pot: 300,
            to_call: 100,
            my_chips: 9_900,
            stacks: vec![],
            big_blind: 100,
            street: "preflop".to_string(),
            action_history: vec![],
            button_seat: None,
            hand_no: 0,
        }
    }

    fn state_no_call() -> HandState {
        HandState {
            to_call: 0,
            pot: 200,
            ..state_with_call()
        }
    }

    #[tokio::test]
    async fn random_agent_legal_actions_with_to_call() {
        let agent = RandomAgent;
        let state = state_with_call();
        let decision = agent.decide(&state).await;
        assert!(
            matches!(
                decision,
                Decision::Fold | Decision::Call | Decision::Raise(_)
            ),
            "unexpected decision when to_call > 0: {decision:?}"
        );
    }

    #[tokio::test]
    async fn random_agent_legal_actions_without_to_call() {
        let agent = RandomAgent;
        let state = state_no_call();
        let decision = agent.decide(&state).await;
        assert!(
            matches!(decision, Decision::Check | Decision::Bet(_)),
            "unexpected decision when to_call == 0: {decision:?}"
        );
    }

    #[tokio::test]
    async fn random_agent_raise_at_least_to_call() {
        let agent = RandomAgent;
        let state = state_with_call();
        for _ in 0..200 {
            if let Decision::Raise(amount) = agent.decide(&state).await {
                assert!(
                    amount >= state.to_call,
                    "raise {amount} < to_call {}",
                    state.to_call
                );
            }
        }
    }

    #[tokio::test]
    async fn random_agent_bet_at_least_big_blind() {
        let agent = RandomAgent;
        let state = state_no_call();
        for _ in 0..200 {
            if let Decision::Bet(amount) = agent.decide(&state).await {
                assert!(
                    amount >= state.big_blind,
                    "bet {amount} < big_blind {}",
                    state.big_blind
                );
            }
        }
    }

    #[tokio::test]
    async fn random_agent_all_three_actions_appear_with_call() {
        let agent = RandomAgent;
        let state = state_with_call();
        let (mut saw_fold, mut saw_call, mut saw_raise) = (false, false, false);
        for _ in 0..300 {
            match agent.decide(&state).await {
                Decision::Fold => saw_fold = true,
                Decision::Call => saw_call = true,
                Decision::Raise(_) => saw_raise = true,
                _ => {}
            }
            if saw_fold && saw_call && saw_raise {
                break;
            }
        }
        assert!(saw_fold, "never saw Fold in 300 trials");
        assert!(saw_call, "never saw Call in 300 trials");
        assert!(saw_raise, "never saw Raise in 300 trials");
    }

    #[tokio::test]
    async fn random_agent_both_actions_appear_without_call() {
        let agent = RandomAgent;
        let state = state_no_call();
        let (mut saw_check, mut saw_bet) = (false, false);
        for _ in 0..200 {
            match agent.decide(&state).await {
                Decision::Check => saw_check = true,
                Decision::Bet(_) => saw_bet = true,
                _ => {}
            }
            if saw_check && saw_bet {
                break;
            }
        }
        assert!(saw_check, "never saw Check in 200 trials");
        assert!(saw_bet, "never saw Bet in 200 trials");
    }

    #[test]
    fn args_defaults() {
        let args =
            Args::try_parse_from(["pkdealer_agent_random"]).expect("default args should parse");
        assert_eq!(args.endpoint, "http://127.0.0.1:50051");
        assert_eq!(args.name, "rando");
        assert!(args.seat.is_none());
        assert_eq!(args.chips, 10_000);
        assert!(args.client_secret.is_empty());
    }

    #[test]
    fn args_with_name_and_seat() {
        let args = Args::try_parse_from([
            "pkdealer_agent_random",
            "--name",
            "alice",
            "--seat",
            "3",
            "--chips",
            "5000",
        ])
        .expect("named args should parse");
        assert_eq!(args.name, "alice");
        assert_eq!(args.seat, Some(3));
        assert_eq!(args.chips, 5_000);
    }

    #[test]
    fn args_with_endpoint() {
        let args = Args::try_parse_from([
            "pkdealer_agent_random",
            "--endpoint",
            "http://10.0.0.1:50051",
        ])
        .expect("endpoint arg should parse");
        assert_eq!(args.endpoint, "http://10.0.0.1:50051");
    }
}
