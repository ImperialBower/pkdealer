//! Per-seat token rollup and notional costing over a recorded session.
//!
//! [`rollup`] flattens every action in a [`HandCollection`] and sums the LLM
//! token usage carried by each action's [`pkcore::hand_history::AgentFidelity`], grouped by seat.
//! [`cost_seats`] then joins that usage against a [`Pricing`] table (with
//! optional per-model overrides) to produce a costed leaderboard.

use std::collections::BTreeMap;
use std::collections::HashMap;

use pkcore::hand_history::{Action, HandCollection, HandHistory};
use tiktoken_rs::CoreBPE;

use crate::pricing::{Pricing, cost_usd, resolve_price};

/// Cumulative LLM token usage for one seat across an entire session.
///
/// Seats that never produced agent fidelity (rule/random bots) appear with
/// zero tokens and `model: None` — they took actions but spent no tokens.
///
/// # Examples
///
/// ```
/// use pkdealer_costsim::report::SeatUsage;
///
/// let usage = SeatUsage {
///     seat: 1,
///     name: "Opus".to_string(),
///     model: Some("claude-opus-4-8".to_string()),
///     input_tokens: 3300,
///     output_tokens: 33,
/// };
/// assert_eq!(usage.input_tokens, 3300);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeatUsage {
    /// Seat number (1-indexed), as recorded on each action.
    pub seat: u8,
    /// Player name/handle taken from the hand's player roster.
    pub name: String,
    /// Model id that acted in this seat, if any agent fidelity carried one.
    pub model: Option<String>,
    /// Cumulative prompt/input tokens over the session.
    pub input_tokens: u64,
    /// Cumulative completion/output tokens over the session.
    pub output_tokens: u64,
}

/// Aggregates per-seat LLM token usage across every hand in `collection`.
///
/// The result is sorted by seat ascending. Every seat that appears in any
/// hand's player roster is included, so bots show up with zero tokens.
///
/// # Examples
///
/// ```
/// use pkcore::hand_history::HandCollection;
/// use pkdealer_costsim::report::rollup;
///
/// let yaml = r#"
/// hands:
///   - hand: { id: "h1", game: holdem }
///     table: { stakes: { small_blind: 1.0, big_blind: 2.0 } }
///     players:
///       - { seat: 1, name: "Opus", stack: 200.0 }
///     streets:
///       preflop:
///         actions:
///           - seat: 1
///             action: raise
///             amount: 6.0
///             agent: { model: "claude-opus-4-8", input_tokens: 1000, output_tokens: 10 }
/// "#;
/// let collection = HandCollection::from_yaml(yaml).unwrap();
/// let seats = rollup(&collection);
/// assert_eq!(seats[0].input_tokens, 1000);
/// assert_eq!(seats[0].model.as_deref(), Some("claude-opus-4-8"));
/// ```
#[must_use]
pub fn rollup(collection: &HandCollection) -> Vec<SeatUsage> {
    rollup_inner(collection, None)
}

/// Like [`rollup`], but recomputes each decision's token counts by re-tokenizing
/// the recorded prompt and response with `encoder` instead of trusting the
/// backend's reported counts (EPIC-44 Phase 3, `--exact`).
///
/// Local Ollama reports tokens using its own tokenizer; pricing those at a
/// commercial model's rates carries a tokenizer-skew bias. Re-tokenizing the
/// recorded prompt (input) and `raw_response` (output) with the target model's
/// tokenizer removes that bias. Actions lacking a recorded prompt fall back to
/// the reported counts, so older recordings still contribute.
///
/// # Examples
///
/// ```
/// use pkcore::hand_history::HandCollection;
/// use pkdealer_costsim::report::rollup_exact;
///
/// let bpe = tiktoken_rs::o200k_base().expect("encoder");
/// let collection = HandCollection::default();
/// assert!(rollup_exact(&collection, &bpe).is_empty());
/// ```
#[must_use]
pub fn rollup_exact(collection: &HandCollection, encoder: &CoreBPE) -> Vec<SeatUsage> {
    rollup_inner(collection, Some(encoder))
}

/// Counts the tokens in `text` under `encoder`, saturating at `u64::MAX`.
fn token_count(encoder: &CoreBPE, text: &str) -> u64 {
    u64::try_from(encoder.encode_with_special_tokens(text).len()).unwrap_or(u64::MAX)
}

/// Shared body of [`rollup`] / [`rollup_exact`]. When `encoder` is `Some`, each
/// action's tokens come from re-tokenizing its recorded prompt/response (falling
/// back to the reported counts when the text is absent); when `None`, the
/// reported counts are used directly.
fn rollup_inner(collection: &HandCollection, encoder: Option<&CoreBPE>) -> Vec<SeatUsage> {
    // BTreeMap keeps the result deterministically ordered by seat.
    let mut by_seat: BTreeMap<u8, SeatUsage> = BTreeMap::new();

    // Seed every seat from the player rosters so bots appear with zero usage.
    for hand in collection.hands() {
        for player in &hand.players {
            by_seat.entry(player.seat).or_insert_with(|| SeatUsage {
                seat: player.seat,
                name: player.name.clone(),
                model: None,
                input_tokens: 0,
                output_tokens: 0,
            });
        }
    }

    // Accumulate token usage from each action's agent fidelity.
    for hand in collection.hands() {
        for action in hand_actions(hand) {
            let Some(agent) = action.agent.as_ref() else {
                continue;
            };
            let entry = by_seat.entry(action.seat).or_insert_with(|| SeatUsage {
                seat: action.seat,
                name: String::new(),
                model: None,
                input_tokens: 0,
                output_tokens: 0,
            });
            // Exact mode re-tokenizes the recorded text; fall back to the
            // reported count when the text (or the encoder) is absent.
            let input = match (encoder, agent.prompt.as_deref()) {
                (Some(enc), Some(prompt)) => Some(token_count(enc, prompt)),
                _ => agent.input_tokens.map(u64::from),
            };
            let output = match (encoder, agent.raw_response.as_deref()) {
                (Some(enc), Some(response)) => Some(token_count(enc, response)),
                _ => agent.output_tokens.map(u64::from),
            };
            if let Some(input) = input {
                entry.input_tokens += input;
            }
            if let Some(output) = output {
                entry.output_tokens += output;
            }
            if entry.model.is_none()
                && let Some(model) = agent.model.as_ref()
            {
                entry.model = Some(model.clone());
            }
        }
    }

    by_seat.into_values().collect()
}

/// A seat's token usage plus its resolved notional model and computed cost.
///
/// `notional_model` is the model used for *pricing*, after applying any
/// `--price-as` override; it may differ from the model that actually ran (every
/// arena seat is local Ollama, so "price seat 1 as Opus" is a config choice).
/// `cost_usd` is `None` when the seat has no model (a bot) or when its notional
/// model is absent from the pricing table — in the latter case `notional_model`
/// is still `Some`, so the renderer can flag an unpriced model rather than
/// silently showing `$0`.
///
/// # Examples
///
/// ```
/// use pkdealer_costsim::report::{SeatCost, SeatUsage};
///
/// let cost = SeatCost {
///     usage: SeatUsage {
///         seat: 1,
///         name: "Opus".to_string(),
///         model: Some("claude-opus-4-8".to_string()),
///         input_tokens: 1000,
///         output_tokens: 10,
///     },
///     notional_model: Some("claude-opus-4-8".to_string()),
///     cost_usd: Some(0.00525),
/// };
/// assert_eq!(cost.usage.seat, 1);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct SeatCost {
    /// The underlying token rollup for this seat.
    pub usage: SeatUsage,
    /// Model used for pricing after override resolution; `None` for bots.
    pub notional_model: Option<String>,
    /// Notional USD cost; `None` for bots and for unpriced notional models.
    pub cost_usd: Option<f64>,
}

/// Joins per-seat usage against a [`Pricing`] table to produce costed rows.
///
/// `overrides` maps an actual model id to a notional model id (the `--price-as
/// <model>=<notional>` flag): a seat whose recorded model is a key in
/// `overrides` is priced as the mapped model instead. Seats with no model (bots)
/// get `notional_model: None` and `cost_usd: None`. A seat whose notional model
/// is missing from `pricing` keeps `notional_model: Some(..)` but `cost_usd:
/// None`, so the count is preserved while the cost is flagged as unknown.
///
/// # Examples
///
/// ```
/// use std::collections::HashMap;
/// use pkdealer_costsim::pricing::Pricing;
/// use pkdealer_costsim::report::{cost_seats, SeatUsage};
///
/// let usages = vec![SeatUsage {
///     seat: 1,
///     name: "Opus".to_string(),
///     model: Some("claude-opus-4-8".to_string()),
///     input_tokens: 1_000_000,
///     output_tokens: 0,
/// }];
/// let pricing = Pricing::from_toml(
///     "[models.\"claude-opus-4-8\"]\ninput = 5.0\noutput = 25.0\n",
/// )
/// .unwrap();
/// let costs = cost_seats(&usages, &pricing, &HashMap::new());
/// assert_eq!(costs[0].cost_usd, Some(5.0));
/// ```
#[must_use]
pub fn cost_seats(
    usages: &[SeatUsage],
    pricing: &Pricing,
    overrides: &HashMap<String, String>,
) -> Vec<SeatCost> {
    usages
        .iter()
        .map(|usage| {
            // Resolve the notional model: the recorded model, remapped through
            // any --price-as override. A bot (no model) stays `None`. This string
            // is for display; the cost itself goes through the shared
            // `resolve_price` path (EPIC-44 Phase 2) so live and post-hoc agree.
            let notional_model = usage.model.as_ref().map(|actual| {
                overrides
                    .get(actual)
                    .cloned()
                    .unwrap_or_else(|| actual.clone())
            });
            // Cost only when the resolved model has a price; otherwise `None`
            // (tokens are still preserved in `usage`).
            let cost_usd = resolve_price(pricing, overrides, usage.model.as_deref())
                .map(|price| cost_usd(price, usage.input_tokens, usage.output_tokens));
            SeatCost {
                usage: usage.clone(),
                notional_model,
                cost_usd,
            }
        })
        .collect()
}

/// Returns every [`Action`] in a hand, flattened across all four streets in
/// betting order. Forced blind posts and bot actions are included; callers
/// filter on [`Action::agent`] as needed.
fn hand_actions(hand: &HandHistory) -> impl Iterator<Item = &Action> {
    hand.streets.iter().flat_map(|s| {
        s.preflop
            .iter()
            .flat_map(|x| x.actions.iter())
            .chain(s.flop.iter().flat_map(|x| x.actions.iter()))
            .chain(s.turn.iter().flat_map(|x| x.actions.iter()))
            .chain(s.river.iter().flat_map(|x| x.actions.iter()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-hand, three-seat session: seat 1 plays Opus, seat 2 is a rule bot
    /// (no agent fidelity), seat 3 plays Haiku.
    fn fixture() -> HandCollection {
        let yaml = r#"
hands:
  - hand: { id: "h1", game: holdem }
    table: { stakes: { small_blind: 1.0, big_blind: 2.0 } }
    players:
      - { seat: 1, name: "Opus", stack: 200.0 }
      - { seat: 2, name: "RuleBot", stack: 200.0 }
      - { seat: 3, name: "Haiku", stack: 200.0 }
    streets:
      preflop:
        actions:
          - { seat: 1, action: raise, amount: 6.0, agent: { model: "claude-opus-4-8", input_tokens: 1000, output_tokens: 10 } }
          - { seat: 2, action: call, amount: 6.0 }
          - { seat: 3, action: call, amount: 6.0, agent: { model: "claude-haiku-4-5", input_tokens: 800, output_tokens: 8 } }
      flop:
        cards: "9c 6d 5h"
        actions:
          - { seat: 1, action: bet, amount: 10.0, agent: { model: "claude-opus-4-8", input_tokens: 1200, output_tokens: 12 } }
          - { seat: 3, action: fold, agent: { model: "claude-haiku-4-5", input_tokens: 900, output_tokens: 5 } }
  - hand: { id: "h2", game: holdem }
    table: { stakes: { small_blind: 1.0, big_blind: 2.0 } }
    players:
      - { seat: 1, name: "Opus", stack: 200.0 }
      - { seat: 2, name: "RuleBot", stack: 200.0 }
    streets:
      preflop:
        actions:
          - { seat: 1, action: raise, amount: 6.0, agent: { model: "claude-opus-4-8", input_tokens: 1100, output_tokens: 11 } }
          - { seat: 2, action: fold }
"#;
        HandCollection::from_yaml(yaml).expect("fixture yaml parses")
    }

    #[test]
    fn rollup_sums_tokens_per_seat_across_hands() {
        let seats = rollup(&fixture());
        assert_eq!(seats.len(), 3, "all three seats present");

        // Seat 1 — Opus: 1000 + 1200 + 1100 input; 10 + 12 + 11 output.
        assert_eq!(seats[0].seat, 1);
        assert_eq!(seats[0].name, "Opus");
        assert_eq!(seats[0].model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(seats[0].input_tokens, 3300);
        assert_eq!(seats[0].output_tokens, 33);

        // Seat 3 — Haiku: 800 + 900 input; 8 + 5 output.
        assert_eq!(seats[2].seat, 3);
        assert_eq!(seats[2].model.as_deref(), Some("claude-haiku-4-5"));
        assert_eq!(seats[2].input_tokens, 1700);
        assert_eq!(seats[2].output_tokens, 13);
    }

    #[test]
    fn rollup_bot_seat_shows_zero_tokens_and_no_model() {
        let seats = rollup(&fixture());
        let bot = &seats[1];
        assert_eq!(bot.seat, 2);
        assert_eq!(bot.name, "RuleBot");
        assert_eq!(bot.model, None);
        assert_eq!(bot.input_tokens, 0);
        assert_eq!(bot.output_tokens, 0);
    }

    #[test]
    fn rollup_exact_recounts_from_recorded_text() {
        let prompt = "Hero holds AhKh on a Qd7s2c flop. Pot is 30, to call 10.";
        let response = "raise to 20";
        let yaml = format!(
            r#"
hands:
  - hand: {{ id: "h1", game: holdem }}
    table: {{ stakes: {{ small_blind: 1.0, big_blind: 2.0 }} }}
    players:
      - {{ seat: 1, name: "Opus", stack: 200.0 }}
    streets:
      preflop:
        actions:
          - {{ seat: 1, action: raise, amount: 6.0, agent: {{ model: "gpt-4.1", input_tokens: 9999, output_tokens: 9999, prompt: "{prompt}", raw_response: "{response}" }} }}
"#
        );
        let collection = HandCollection::from_yaml(&yaml).expect("yaml parses");
        let bpe = tiktoken_rs::o200k_base().expect("encoder");
        let seats = rollup_exact(&collection, &bpe);

        let want_in = u64::try_from(bpe.encode_with_special_tokens(prompt).len()).expect("fits");
        let want_out = u64::try_from(bpe.encode_with_special_tokens(response).len()).expect("fits");
        assert_eq!(seats[0].input_tokens, want_in);
        assert_eq!(seats[0].output_tokens, want_out);
        // The reported (Ollama-tokenizer) 9999 counts were replaced, not used.
        assert_ne!(seats[0].input_tokens, 9999);
    }

    #[test]
    fn rollup_exact_falls_back_to_reported_when_no_prompt() {
        // The standard fixture has no `prompt`/`raw_response`, so exact mode must
        // fall back to the reported counts — identical to plain rollup.
        let bpe = tiktoken_rs::o200k_base().expect("encoder");
        assert_eq!(rollup_exact(&fixture(), &bpe), rollup(&fixture()));
    }

    fn pricing() -> Pricing {
        Pricing::from_toml(
            r#"
[models."claude-opus-4-8"]
input = 5.00
output = 25.00

[models."claude-haiku-4-5"]
input = 1.00
output = 5.00
"#,
        )
        .expect("pricing parses")
    }

    fn approx(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|v| (v - b).abs() < 1e-9)
    }

    #[test]
    fn cost_seats_prices_each_seats_recorded_model() {
        let usages = rollup(&fixture());
        let costs = cost_seats(&usages, &pricing(), &HashMap::new());

        // Seat 1 — Opus: 3300/1e6*5 + 33/1e6*25 = 0.017325.
        assert_eq!(costs[0].notional_model.as_deref(), Some("claude-opus-4-8"));
        assert!(
            approx(costs[0].cost_usd, 0.017_325),
            "opus cost {:?}",
            costs[0].cost_usd
        );
        // Seat 3 — Haiku: 1700/1e6*1 + 13/1e6*5 = 0.001765.
        assert!(
            approx(costs[2].cost_usd, 0.001_765),
            "haiku cost {:?}",
            costs[2].cost_usd
        );
    }

    #[test]
    fn cost_seats_bot_has_no_model_and_no_cost() {
        let usages = rollup(&fixture());
        let costs = cost_seats(&usages, &pricing(), &HashMap::new());
        assert_eq!(costs[1].notional_model, None);
        assert_eq!(costs[1].cost_usd, None);
    }

    #[test]
    fn cost_seats_unpriced_model_is_reported_but_not_costed() {
        // Pricing table that omits Opus entirely.
        let pricing =
            Pricing::from_toml("[models.\"claude-haiku-4-5\"]\ninput = 1.0\noutput = 5.0\n")
                .unwrap();
        let usages = rollup(&fixture());
        let costs = cost_seats(&usages, &pricing, &HashMap::new());
        // Seat 1's model is still reported, but cost is unknown (not zero).
        assert_eq!(costs[0].notional_model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(costs[0].cost_usd, None);
        // Token counts are preserved regardless.
        assert_eq!(costs[0].usage.input_tokens, 3300);
    }

    #[test]
    fn cost_seats_applies_price_as_override() {
        let mut overrides = HashMap::new();
        overrides.insert(
            "claude-opus-4-8".to_string(),
            "claude-haiku-4-5".to_string(),
        );
        let usages = rollup(&fixture());
        let costs = cost_seats(&usages, &pricing(), &overrides);
        // Seat 1 now priced at Haiku rates: 3300/1e6*1 + 33/1e6*5 = 0.003465.
        assert_eq!(costs[0].notional_model.as_deref(), Some("claude-haiku-4-5"));
        assert!(
            approx(costs[0].cost_usd, 0.003_465),
            "overridden cost {:?}",
            costs[0].cost_usd
        );
    }
}
