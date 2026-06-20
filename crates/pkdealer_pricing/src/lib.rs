//! Notional per-model token pricing and cost computation.
//!
//! Local Ollama inference has no dollar cost, so "spend" in pkdealer is a token
//! *count*. This crate turns a recorded token count into a *notional* USD
//! figure — what the same play would have cost on a commercial API — by joining
//! a model id against a [`Pricing`] table of per-million-token rates.
//!
//! It is a small, dependency-light leaf (only `serde` + `toml`) so it can be
//! shared by both the offline analysis tool (`pkdealer_costsim`) and the live
//! `pkdealer_service` without dragging CLI/analysis dependencies into the
//! service. Cost is a pure function of `(model, input_tokens, output_tokens)`,
//! so the same figures can be computed live or post-hoc and agree exactly.

use std::collections::HashMap;

use serde::Deserialize;

/// Per-million-token USD rates for a single model.
///
/// Both rates are expressed in **USD per 1,000,000 tokens**, matching how
/// commercial providers publish pricing.
///
/// # Examples
///
/// ```
/// use pkdealer_pricing::Price;
///
/// let opus = Price { input: 5.00, output: 25.00 };
/// assert_eq!(opus.input, 5.00);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Price {
    /// USD per 1,000,000 prompt/input tokens.
    pub input: f64,
    /// USD per 1,000,000 completion/output tokens.
    pub output: f64,
}

/// Computes the notional USD cost of a token count under a given [`Price`].
///
/// Cost is `input_tokens / 1e6 * price.input + output_tokens / 1e6 *
/// price.output`. The result is a floating-point dollar figure; callers that
/// need an exact wire representation should convert to integer micro-USD.
///
/// # Examples
///
/// ```
/// use pkdealer_pricing::{cost_usd, Price};
///
/// // 1M input + 1M output at $5 / $25 per million.
/// let price = Price { input: 5.00, output: 25.00 };
/// assert_eq!(cost_usd(&price, 1_000_000, 1_000_000), 30.0);
/// ```
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn cost_usd(price: &Price, input_tokens: u64, output_tokens: u64) -> f64 {
    (input_tokens as f64 / 1e6) * price.input + (output_tokens as f64 / 1e6) * price.output
}

/// A notional pricing table keyed by model id.
///
/// The id space matches `arena.toml` / `pkcore::hand_history::AgentFidelity`
/// `model`, so a model recorded in a session can be looked up directly.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Pricing {
    /// Per-model rates, keyed by model id (e.g. `"claude-opus-4-8"`).
    #[serde(default)]
    pub models: HashMap<String, Price>,
}

impl Pricing {
    /// Parses a `pricing.toml` document into a [`Pricing`] table.
    ///
    /// # Errors
    ///
    /// Returns [`toml::de::Error`] if the document is not valid TOML or does not
    /// match the expected `[models."<id>"]` schema.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_pricing::Pricing;
    ///
    /// let toml = r#"
    /// [models."claude-haiku-4-5"]
    /// input = 1.00
    /// output = 5.00
    /// "#;
    /// let pricing = Pricing::from_toml(toml).unwrap();
    /// assert_eq!(pricing.price("claude-haiku-4-5").unwrap().output, 5.00);
    /// ```
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// Looks up the [`Price`] for a model id, or `None` if it is not in the table.
    ///
    /// # Examples
    ///
    /// ```
    /// use pkdealer_pricing::Pricing;
    ///
    /// let pricing = Pricing::default();
    /// assert!(pricing.price("missing-model").is_none());
    /// ```
    #[must_use]
    pub fn price(&self, model: &str) -> Option<&Price> {
        self.models.get(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_usd_combines_input_and_output_rates() {
        // 500k input at $2/M = $1.00; 100k output at $8/M = $0.80; total $1.80.
        let price = Price {
            input: 2.00,
            output: 8.00,
        };
        let cost = cost_usd(&price, 500_000, 100_000);
        assert!((cost - 1.80).abs() < 1e-9, "expected 1.80, got {cost}");
    }

    #[test]
    fn cost_usd_zero_tokens_is_zero() {
        let price = Price {
            input: 5.00,
            output: 25.00,
        };
        assert_eq!(cost_usd(&price, 0, 0), 0.0);
    }

    #[test]
    fn from_toml_parses_per_model_subtables() {
        let toml = r#"
[models."claude-opus-4-8"]
input = 5.00
output = 25.00

[models."deepseek-v3.2"]
input = 0.14
output = 0.28
"#;
        let pricing = Pricing::from_toml(toml).expect("valid pricing toml");
        assert_eq!(
            pricing.price("claude-opus-4-8"),
            Some(&Price {
                input: 5.00,
                output: 25.00
            })
        );
        assert_eq!(
            pricing.price("deepseek-v3.2"),
            Some(&Price {
                input: 0.14,
                output: 0.28
            })
        );
    }

    #[test]
    fn from_toml_empty_document_is_empty_table() {
        let pricing = Pricing::from_toml("").expect("empty toml is valid");
        assert!(pricing.models.is_empty());
    }

    #[test]
    fn from_toml_rejects_malformed_document() {
        let err = Pricing::from_toml("this is = = not toml");
        assert!(err.is_err());
    }

    #[test]
    fn price_returns_none_for_unknown_model() {
        let pricing =
            Pricing::from_toml("[models.\"x\"]\ninput = 1.0\noutput = 2.0\n").expect("valid");
        assert!(pricing.price("not-in-table").is_none());
    }
}
