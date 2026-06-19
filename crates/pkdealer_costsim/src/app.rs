//! CLI-support logic: input loading, override/scenario resolution, and table
//! rendering. Kept separate from `main` so each piece is unit-testable.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pkcore::hand_history::HandCollection;

use crate::pricing::Pricing;
use crate::report::{SeatCost, cost_seats, rollup};

/// Errors surfaced by the `pkdealer_costsim` tool.
#[derive(Debug)]
pub enum CostsimError {
    /// An input file could not be read.
    Io(std::io::Error),
    /// A YAML/TOML document failed to parse; the wrapped string is the
    /// underlying parser message.
    Parse(String),
    /// A `--price-as` argument was not of the form `model=notional_model`.
    BadOverride(String),
    /// A `--scenario` name is not recognized.
    UnknownScenario(String),
}

impl std::fmt::Display for CostsimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CostsimError::Io(e) => write!(f, "I/O error: {e}"),
            CostsimError::Parse(msg) => write!(f, "parse error: {msg}"),
            CostsimError::BadOverride(arg) => {
                write!(
                    f,
                    "invalid --price-as '{arg}' (expected model=notional_model)"
                )
            }
            CostsimError::UnknownScenario(name) => {
                write!(
                    f,
                    "unknown --scenario '{name}' (try: mixed, all-opus, all-haiku)"
                )
            }
        }
    }
}

impl std::error::Error for CostsimError {}

/// Parses `--price-as model=notional_model` arguments into an override map.
///
/// Each item maps an *actual* recorded model id to the *notional* model it
/// should be priced as. Both sides must be non-empty.
///
/// # Errors
///
/// Returns [`CostsimError::BadOverride`] for any item missing an `=` or with an
/// empty side.
///
/// # Examples
///
/// ```
/// use pkdealer_costsim::app::parse_price_as;
///
/// let map = parse_price_as(&["gemma=claude-opus-4-8".to_string()]).unwrap();
/// assert_eq!(map.get("gemma").unwrap(), "claude-opus-4-8");
/// ```
pub fn parse_price_as(items: &[String]) -> Result<HashMap<String, String>, CostsimError> {
    let mut map = HashMap::new();
    for item in items {
        let (actual, notional) = item
            .split_once('=')
            .ok_or_else(|| CostsimError::BadOverride(item.clone()))?;
        if actual.is_empty() || notional.is_empty() {
            return Err(CostsimError::BadOverride(item.clone()));
        }
        map.insert(actual.to_string(), notional.to_string());
    }
    Ok(map)
}

/// Builds an override map for a named `--scenario`, given the set of model ids
/// present in the session.
///
/// - `mixed` — no overrides (price each seat at its recorded model).
/// - `all-opus` — price every model as `claude-opus-4-8`.
/// - `all-haiku` — price every model as `claude-haiku-4-5`.
///
/// # Errors
///
/// Returns [`CostsimError::UnknownScenario`] for any other name.
///
/// # Examples
///
/// ```
/// use pkdealer_costsim::app::scenario_overrides;
///
/// let models = vec!["gemma".to_string(), "llama".to_string()];
/// let map = scenario_overrides("all-opus", &models).unwrap();
/// assert_eq!(map.get("gemma").unwrap(), "claude-opus-4-8");
/// assert_eq!(map.get("llama").unwrap(), "claude-opus-4-8");
/// ```
pub fn scenario_overrides(
    scenario: &str,
    models: &[String],
) -> Result<HashMap<String, String>, CostsimError> {
    let notional = match scenario {
        "mixed" => return Ok(HashMap::new()),
        "all-opus" => "claude-opus-4-8",
        "all-haiku" => "claude-haiku-4-5",
        other => return Err(CostsimError::UnknownScenario(other.to_string())),
    };
    Ok(models
        .iter()
        .map(|model| (model.clone(), notional.to_string()))
        .collect())
}

/// Reads a recorded [`HandCollection`] YAML file from disk.
///
/// # Errors
///
/// Returns [`CostsimError::Io`] if the file cannot be read and
/// [`CostsimError::Parse`] if its contents are not a valid `HandCollection`.
pub fn load_collection(path: &Path) -> Result<HandCollection, CostsimError> {
    let text = std::fs::read_to_string(path).map_err(CostsimError::Io)?;
    HandCollection::from_yaml(&text).map_err(|e| CostsimError::Parse(e.to_string()))
}

/// Reads a `pricing.toml` file from disk.
///
/// # Errors
///
/// Returns [`CostsimError::Io`] if the file cannot be read and
/// [`CostsimError::Parse`] if its contents are not valid pricing TOML.
pub fn load_pricing(path: &Path) -> Result<Pricing, CostsimError> {
    let text = std::fs::read_to_string(path).map_err(CostsimError::Io)?;
    Pricing::from_toml(&text).map_err(|e| CostsimError::Parse(e.to_string()))
}

/// Renders a costed leaderboard as a fixed-width text table, followed by a
/// session total line. Bots (no model) render blank model/cost cells; unpriced
/// models render their model with a `?` cost.
///
/// # Examples
///
/// ```
/// use pkdealer_costsim::report::{SeatCost, SeatUsage};
/// use pkdealer_costsim::app::render_table;
///
/// let rows = vec![SeatCost {
///     usage: SeatUsage {
///         seat: 1,
///         name: "Opus".to_string(),
///         model: Some("claude-opus-4-8".to_string()),
///         input_tokens: 3300,
///         output_tokens: 33,
///     },
///     notional_model: Some("claude-opus-4-8".to_string()),
///     cost_usd: Some(0.017325),
/// }];
/// let table = render_table(&rows);
/// assert!(table.contains("Opus"));
/// assert!(table.contains("TOTAL"));
/// ```
#[must_use]
pub fn render_table(rows: &[SeatCost]) -> String {
    use std::fmt::Write as _;

    // Column widths chosen to fit model ids and right-aligned token counts.
    let header = format!(
        "{:>4}  {:<12}  {:<20}  {:>10}  {:>10}  {:>12}",
        "SEAT", "PLAYER", "MODEL", "INPUT", "OUTPUT", "COST(USD)"
    );
    let rule = "-".repeat(header.len());

    let mut out = String::new();
    let _ = writeln!(out, "{header}");
    let _ = writeln!(out, "{rule}");

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut total_cost = 0.0_f64;

    for row in rows {
        total_in += row.usage.input_tokens;
        total_out += row.usage.output_tokens;
        // Model cell: blank for bots (no notional model).
        let model = row.notional_model.as_deref().unwrap_or("");
        // Cost cell: blank for bots; "?" for a priced-but-unknown model.
        let cost = match (&row.notional_model, row.cost_usd) {
            (_, Some(c)) => {
                total_cost += c;
                format!("{c:.6}")
            }
            (Some(_), None) => "?".to_string(),
            (None, None) => String::new(),
        };
        let _ = writeln!(
            out,
            "{:>4}  {:<12}  {:<20}  {:>10}  {:>10}  {:>12}",
            row.usage.seat,
            row.usage.name,
            model,
            row.usage.input_tokens,
            row.usage.output_tokens,
            cost,
        );
    }

    let _ = writeln!(out, "{rule}");
    let _ = writeln!(
        out,
        "{:>4}  {:<12}  {:<20}  {:>10}  {:>10}  {:>12}",
        "",
        "TOTAL",
        "",
        total_in,
        total_out,
        format!("{total_cost:.6}"),
    );
    out
}

/// Inputs for one `pkdealer_costsim` run, mirroring the CLI flags.
#[derive(Clone, Debug)]
pub struct RunConfig {
    /// Path to the recorded `HandCollection` YAML.
    pub collection: PathBuf,
    /// Optional `pricing.toml`; when absent, all costs are unknown.
    pub pricing: Option<PathBuf>,
    /// `--price-as model=notional_model` overrides.
    pub price_as: Vec<String>,
    /// Optional named `--scenario` (`mixed`, `all-opus`, `all-haiku`).
    pub scenario: Option<String>,
}

/// Executes a full cost-analysis run and returns the rendered leaderboard.
///
/// Loads the recorded session and pricing table, rolls up per-seat token usage,
/// resolves notional pricing (scenario first, then explicit `--price-as`
/// overrides, which take precedence), costs each seat, and renders the table.
///
/// # Errors
///
/// Propagates [`CostsimError`] from file loading, override parsing, or scenario
/// resolution.
pub fn run(config: &RunConfig) -> Result<String, CostsimError> {
    let collection = load_collection(&config.collection)?;
    let pricing = match &config.pricing {
        Some(path) => load_pricing(path)?,
        None => Pricing::default(),
    };

    let usages = rollup(&collection);

    // Distinct models present in the session, for scenario expansion.
    let models: Vec<String> = {
        let mut seen: Vec<String> = usages.iter().filter_map(|u| u.model.clone()).collect();
        seen.sort();
        seen.dedup();
        seen
    };

    // Scenario sets the baseline overrides; explicit --price-as wins on conflict.
    let mut overrides = match &config.scenario {
        Some(name) => scenario_overrides(name, &models)?,
        None => HashMap::new(),
    };
    for (actual, notional) in parse_price_as(&config.price_as)? {
        overrides.insert(actual, notional);
    }

    let costs = cost_seats(&usages, &pricing, &overrides);
    Ok(render_table(&costs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_price_as_builds_override_map() {
        let map = parse_price_as(&[
            "gemma=claude-opus-4-8".to_string(),
            "llama=deepseek-v3.2".to_string(),
        ])
        .expect("valid overrides");
        assert_eq!(map.get("gemma").unwrap(), "claude-opus-4-8");
        assert_eq!(map.get("llama").unwrap(), "deepseek-v3.2");
    }

    #[test]
    fn parse_price_as_rejects_item_without_equals() {
        let err = parse_price_as(&["gemma".to_string()]);
        assert!(matches!(err, Err(CostsimError::BadOverride(_))));
    }

    #[test]
    fn parse_price_as_rejects_empty_side() {
        assert!(matches!(
            parse_price_as(&["=opus".to_string()]),
            Err(CostsimError::BadOverride(_))
        ));
        assert!(matches!(
            parse_price_as(&["gemma=".to_string()]),
            Err(CostsimError::BadOverride(_))
        ));
    }

    #[test]
    fn scenario_overrides_mixed_is_empty() {
        let map = scenario_overrides("mixed", &["gemma".to_string()]).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn scenario_overrides_all_opus_maps_every_model() {
        let models = vec!["gemma".to_string(), "llama".to_string()];
        let map = scenario_overrides("all-opus", &models).unwrap();
        assert_eq!(map.len(), 2);
        assert!(map.values().all(|v| v == "claude-opus-4-8"));
    }

    #[test]
    fn scenario_overrides_rejects_unknown_name() {
        let err = scenario_overrides("all-llama", &[]);
        assert!(matches!(err, Err(CostsimError::UnknownScenario(_))));
    }

    use crate::report::{SeatCost, SeatUsage};

    fn costed_rows() -> Vec<SeatCost> {
        vec![
            SeatCost {
                usage: SeatUsage {
                    seat: 1,
                    name: "Opus".to_string(),
                    model: Some("claude-opus-4-8".to_string()),
                    input_tokens: 3300,
                    output_tokens: 33,
                },
                notional_model: Some("claude-opus-4-8".to_string()),
                cost_usd: Some(0.017_325),
            },
            SeatCost {
                usage: SeatUsage {
                    seat: 2,
                    name: "RuleBot".to_string(),
                    model: None,
                    input_tokens: 0,
                    output_tokens: 0,
                },
                notional_model: None,
                cost_usd: None,
            },
            SeatCost {
                usage: SeatUsage {
                    seat: 3,
                    name: "Haiku".to_string(),
                    model: Some("claude-haiku-4-5".to_string()),
                    input_tokens: 1700,
                    output_tokens: 13,
                },
                notional_model: Some("claude-haiku-4-5".to_string()),
                cost_usd: Some(0.001_765),
            },
        ]
    }

    #[test]
    fn render_table_shows_seats_tokens_and_costs() {
        let table = render_table(&costed_rows());
        assert!(table.contains("Opus"), "missing Opus row:\n{table}");
        assert!(table.contains("claude-opus-4-8"));
        assert!(table.contains("3300"));
        assert!(table.contains("0.017325"), "missing opus cost:\n{table}");
        assert!(table.contains("RuleBot"));
    }

    #[test]
    fn render_table_totals_token_and_cost_columns() {
        let table = render_table(&costed_rows());
        assert!(table.contains("TOTAL"));
        // 3300 + 0 + 1700 input; 0.017325 + 0.001765 cost.
        assert!(table.contains("5000"), "missing input total:\n{table}");
        assert!(table.contains("0.019090"), "missing cost total:\n{table}");
    }

    #[test]
    fn render_table_flags_unpriced_model_without_zeroing() {
        let rows = vec![SeatCost {
            usage: SeatUsage {
                seat: 1,
                name: "Gemma".to_string(),
                model: Some("gemma".to_string()),
                input_tokens: 500,
                output_tokens: 5,
            },
            notional_model: Some("gemma".to_string()),
            cost_usd: None,
        }];
        let table = render_table(&rows);
        // Inspect the seat's own row, not the TOTAL line (which may be $0).
        let gemma_row = table
            .lines()
            .find(|l| l.contains("Gemma"))
            .expect("gemma row present");
        assert!(gemma_row.contains("gemma"), "missing model id: {gemma_row}");
        // Unpriced cost must not be rendered as $0.00 on the seat row.
        assert!(
            !gemma_row.contains("0.000000"),
            "unpriced shown as zero: {gemma_row}"
        );
        assert!(
            gemma_row.contains('?'),
            "unpriced cost not flagged: {gemma_row}"
        );
    }

    // ── end-to-end `run` over real temp files ────────────────────────────────

    const SESSION_YAML: &str = r#"
hands:
  - hand: { id: "h1", game: holdem }
    table: { stakes: { small_blind: 1.0, big_blind: 2.0 } }
    players:
      - { seat: 1, name: "Opus", stack: 200.0 }
      - { seat: 2, name: "RuleBot", stack: 200.0 }
    streets:
      preflop:
        actions:
          - { seat: 1, action: raise, amount: 6.0, agent: { model: "claude-opus-4-8", input_tokens: 5000, output_tokens: 46 } }
          - { seat: 2, action: fold }
"#;

    const PRICING_TOML: &str = r#"
[models."claude-opus-4-8"]
input = 5.00
output = 25.00

[models."claude-haiku-4-5"]
input = 1.00
output = 5.00
"#;

    /// Writes `content` to a uniquely-named temp file and returns its path.
    fn temp_file(tag: &str, content: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("costsim_{tag}_{}_{nanos}", std::process::id()));
        std::fs::write(&path, content).expect("write temp file");
        path
    }

    #[test]
    fn run_costs_a_session_at_recorded_models() {
        let session = temp_file("session", SESSION_YAML);
        let pricing = temp_file("pricing", PRICING_TOML);
        let out = run(&RunConfig {
            collection: session.clone(),
            pricing: Some(pricing.clone()),
            price_as: vec![],
            scenario: None,
        })
        .expect("run succeeds");
        // 5000/1e6*5 + 46/1e6*25 = 0.025 + 0.00115 = 0.02615.
        assert!(out.contains("Opus"));
        assert!(out.contains("0.026150"), "wrong opus cost:\n{out}");
        let _ = std::fs::remove_file(session);
        let _ = std::fs::remove_file(pricing);
    }

    #[test]
    fn run_scenario_repricing_holds_5x_ratio() {
        let session = temp_file("session_ratio", SESSION_YAML);
        let pricing = temp_file("pricing_ratio", PRICING_TOML);
        let opus = run(&RunConfig {
            collection: session.clone(),
            pricing: Some(pricing.clone()),
            price_as: vec![],
            scenario: Some("all-opus".to_string()),
        })
        .unwrap();
        let haiku = run(&RunConfig {
            collection: session.clone(),
            pricing: Some(pricing.clone()),
            price_as: vec![],
            scenario: Some("all-haiku".to_string()),
        })
        .unwrap();
        // all-opus: 0.026150; all-haiku: 5000/1e6*1 + 46/1e6*5 = 0.005230 (5x).
        assert!(opus.contains("0.026150"), "opus:\n{opus}");
        assert!(haiku.contains("0.005230"), "haiku:\n{haiku}");
        let _ = std::fs::remove_file(session);
        let _ = std::fs::remove_file(pricing);
    }

    #[test]
    fn run_without_pricing_reports_tokens_with_unknown_cost() {
        let session = temp_file("session_nopricing", SESSION_YAML);
        let out = run(&RunConfig {
            collection: session.clone(),
            pricing: None,
            price_as: vec![],
            scenario: None,
        })
        .expect("run succeeds without pricing");
        assert!(out.contains("5000"), "tokens missing:\n{out}");
        assert!(out.contains('?'), "unpriced not flagged:\n{out}");
        let _ = std::fs::remove_file(session);
    }
}
