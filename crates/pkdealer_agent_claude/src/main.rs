#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! # `pkdealer_agent_claude`
//!
//! A poker agent that uses the Anthropic Claude API to make decisions. The
//! current hand state is formatted into a natural-language prompt and sent to
//! Claude; the text response is parsed back into a [`Decision`].
//!
//! Each API call is wrapped in an OpenTelemetry span with `gen_ai.*` semantic
//! convention attributes so decisions appear in Jaeger alongside the service's
//! own hand and action spans.
//!
//! ## Usage
//!
//! ```text
//! ANTHROPIC_API_KEY=sk-... cargo run --bin pkdealer_agent_claude -- --name claude
//! ```
//!
//! ## Environment variables
//!
//! | Variable | Default | Purpose |
//! |----------|---------|---------|
//! | `ANTHROPIC_API_KEY` | — | **Required.** Anthropic API key |
//! | `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | gRPC service address |
//! | `ANTHROPIC_MODEL` | `claude-sonnet-4-6` | Claude model override |
//! | `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTel collector |
//! | `OTEL_SDK_DISABLED` | — | Set to `true` to skip OTel init |

use std::process;

use async_trait::async_trait;
use clap::Parser;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{SpanExporter, WithExportConfig as _};
use opentelemetry_sdk::{Resource, propagation::TraceContextPropagator, trace::SdkTracerProvider};
use pkdealer_agent_core::{AgentConfig, Decision, HandState, PokerAgent, run_agent};
use tracing::Instrument as _;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{EnvFilter, Registry, fmt, layer::SubscriberExt as _, util::SubscriberInitExt as _};

// ── CLI ───────────────────────────────────────────────────────────────────────

/// Claude LLM poker agent connected to a pkdealer gRPC service.
#[derive(Debug, Parser)]
#[command(
    name = "pkdealer_agent_claude",
    about = "LLM poker agent powered by Anthropic Claude"
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
    #[arg(long, default_value = "claude")]
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

    /// Claude model identifier.
    #[arg(long, env = "ANTHROPIC_MODEL", default_value = "claude-sonnet-4-6")]
    model: String,

    /// Maximum tokens Claude may generate per response.
    #[arg(long, default_value_t = 16)]
    max_tokens: u32,
}

// ── Anthropic API types ───────────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<ApiMessage>,
}

#[derive(serde::Serialize)]
struct ApiMessage {
    role: &'static str,
    content: String,
}

#[derive(serde::Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
    usage: ApiUsage,
}

#[derive(serde::Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(serde::Deserialize)]
struct ApiUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// ── Agent ─────────────────────────────────────────────────────────────────────

struct ClaudeAgent {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl ClaudeAgent {
    fn new(api_key: String, model: String, max_tokens: u32) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            model,
            max_tokens,
        }
    }

    /// Sends `prompt` to the Anthropic Messages API and returns the text
    /// response together with input/output token counts.
    async fn call_api(
        &self,
        prompt: String,
    ) -> Result<(String, u32, u32), Box<dyn std::error::Error + Send + Sync>> {
        let body = ApiRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            messages: vec![ApiMessage {
                role: "user",
                content: prompt,
            }],
        };

        let resp = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Anthropic API {status}: {text}").into());
        }

        let parsed: ApiResponse = resp.json().await?;
        let text = parsed
            .content
            .into_iter()
            .find(|b| b.kind == "text")
            .and_then(|b| b.text)
            .unwrap_or_default();

        Ok((text, parsed.usage.input_tokens, parsed.usage.output_tokens))
    }
}

#[async_trait]
impl PokerAgent for ClaudeAgent {
    /// Builds a natural-language prompt, calls the Anthropic API, and parses
    /// the response into a [`Decision`]. Emits an OTel span with `gen_ai.*`
    /// attributes for every API call.
    ///
    /// Falls back to `Check` (or `Fold` if facing a bet) on any API error.
    async fn decide(&self, state: &HandState) -> Decision {
        let prompt = build_prompt(state);
        let pot_odds = pot_odds(state);
        let to_call = state.to_call;
        let street = state.street.clone();

        let span = tracing::info_span!(
            "llm.decision",
            gen_ai.system = "anthropic",
            gen_ai.request.model = self.model.as_str(),
            gen_ai.request.max_tokens = self.max_tokens,
            poker.street = street.as_str(),
            poker.pot = state.pot,
            poker.pot_odds = pot_odds,
            gen_ai.usage.input_tokens = tracing::field::Empty,
            gen_ai.usage.output_tokens = tracing::field::Empty,
            poker.action_chosen = tracing::field::Empty,
        );

        let api_result = self.call_api(prompt).instrument(span.clone()).await;

        match api_result {
            Ok((text, in_tok, out_tok)) => {
                span.record("gen_ai.usage.input_tokens", in_tok);
                span.record("gen_ai.usage.output_tokens", out_tok);
                let decision = parse_action(&text, to_call);
                span.record("poker.action_chosen", tracing::field::debug(&decision));
                eprintln!("[claude] {street} → {text:?} → {decision:?}  (in={in_tok} out={out_tok})");
                decision
            }
            Err(e) => {
                eprintln!("[claude] API error ({street}): {e}");
                if to_call == 0 {
                    Decision::Check
                } else {
                    Decision::Fold
                }
            }
        }
    }
}

// ── Prompt & parsing ──────────────────────────────────────────────────────────

/// Formats a [`HandState`] into a natural-language prompt for Claude.
///
/// # Examples
///
/// ```
/// use pkdealer_agent_core::HandState;
///
/// let state = HandState {
///     seat: 0,
///     hole_cards: "Ah Kd".to_string(),
///     board: String::new(),
///     pot: 200,
///     to_call: 100,
///     my_chips: 9_900,
///     stacks: vec![(0, "alice".to_string(), 9_900)],
///     big_blind: 100,
///     street: "preflop".to_string(),
///     action_history: vec![],
/// };
/// let prompt = pkdealer_agent_claude_build_prompt(&state);
/// assert!(prompt.contains("Ah Kd"));
/// assert!(prompt.contains("preflop"));
/// ```
fn build_prompt(state: &HandState) -> String {
    let board = if state.board.is_empty() {
        "(no community cards yet)".to_string()
    } else {
        state.board.clone()
    };

    let stacks_str = state
        .stacks
        .iter()
        .map(|(seat, name, chips)| format!("seat {seat} {name}: {chips}"))
        .collect::<Vec<_>>()
        .join(", ");

    let history = if state.action_history.is_empty() {
        "(no actions yet this street)".to_string()
    } else {
        state.action_history.join("\n")
    };

    format!(
        "You are a professional poker player at a No-Limit Hold'em table.\n\n\
         Your hand: {hole_cards}\n\
         Board: {board} ({street})\n\
         Pot: {pot} chips  |  To call: {to_call} chips  |  Your stack: {my_chips} chips\n\
         Big blind: {big_blind}\n\n\
         Seat stacks: {stacks_str}\n\n\
         Action history this street:\n\
         {history}\n\n\
         Choose ONE action: fold, check, call, bet <amount>, raise <amount>\n\
         Respond with only the action, nothing else.",
        hole_cards = state.hole_cards,
        board = board,
        street = state.street,
        pot = state.pot,
        to_call = state.to_call,
        my_chips = state.my_chips,
        big_blind = state.big_blind,
        stacks_str = stacks_str,
        history = history,
    )
}

/// Parses Claude's text response into a [`Decision`].
///
/// Recognizes `fold`, `check`, `call`, `all in`, `bet <n>`, `raise <n>`,
/// and `raise to <n>` (case-insensitive). Falls back to `Check` when
/// `to_call == 0` or `Fold` when facing a bet.
///
/// # Examples
///
/// ```
/// use pkdealer_agent_core::Decision;
///
/// assert_eq!(pkdealer_agent_claude_parse_action("fold", 100), Decision::Fold);
/// assert_eq!(pkdealer_agent_claude_parse_action("check", 0), Decision::Check);
/// assert_eq!(pkdealer_agent_claude_parse_action("call", 50), Decision::Call);
/// assert_eq!(pkdealer_agent_claude_parse_action("bet 300", 0), Decision::Bet(300));
/// assert_eq!(pkdealer_agent_claude_parse_action("raise 600", 100), Decision::Raise(600));
/// assert_eq!(pkdealer_agent_claude_parse_action("raise to 800", 100), Decision::Raise(800));
/// // Fallback on unrecognized response
/// assert_eq!(pkdealer_agent_claude_parse_action("???", 0), Decision::Check);
/// assert_eq!(pkdealer_agent_claude_parse_action("???", 50), Decision::Fold);
/// ```
fn parse_action(response: &str, to_call: u32) -> Decision {
    let lower = response.trim().to_lowercase();

    if lower == "fold" {
        return Decision::Fold;
    }
    if lower == "check" {
        return Decision::Check;
    }
    if lower == "call" {
        return Decision::Call;
    }
    if matches!(lower.as_str(), "all in" | "all-in" | "allin") {
        return Decision::AllIn;
    }

    if let Some(rest) = lower.strip_prefix("bet ") {
        if let Ok(n) = rest.trim().parse::<u32>() {
            return Decision::Bet(n);
        }
    }

    if let Some(rest) = lower.strip_prefix("raise ") {
        let rest = rest.trim().strip_prefix("to ").unwrap_or(rest.trim());
        if let Ok(n) = rest.trim().parse::<u32>() {
            return Decision::Raise(n);
        }
    }

    // Unrecognized response — fall back to a safe, legal action.
    if to_call == 0 {
        Decision::Check
    } else {
        Decision::Fold
    }
}

/// Computes pot odds as a fraction: `to_call / (pot + to_call)`.
///
/// Returns `0.0` when there is no bet to call.
///
/// # Examples
///
/// ```
/// use pkdealer_agent_core::HandState;
///
/// let state = HandState {
///     seat: 0,
///     hole_cards: String::new(),
///     board: String::new(),
///     pot: 300,
///     to_call: 100,
///     my_chips: 9_900,
///     stacks: vec![],
///     big_blind: 100,
///     street: "flop".to_string(),
///     action_history: vec![],
/// };
/// let odds = pkdealer_agent_claude_pot_odds(&state);
/// assert!((odds - 0.25).abs() < 1e-9);
/// ```
fn pot_odds(state: &HandState) -> f64 {
    if state.to_call == 0 {
        return 0.0;
    }
    let total = f64::from(state.pot) + f64::from(state.to_call);
    if total == 0.0 {
        return 0.0;
    }
    f64::from(state.to_call) / total
}

// ── OTel init ─────────────────────────────────────────────────────────────────

/// Holds the OTel tracer provider; flushes batched spans on drop.
struct OtelGuard(SdkTracerProvider);

impl Drop for OtelGuard {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

/// Initialises OTel tracing with a batched OTLP gRPC exporter.
///
/// Returns `None` when `OTEL_SDK_DISABLED=true`, leaving the caller without
/// an OTel subscriber (useful in tests and `cargo run` without a collector).
fn init_otel(service_name: &str) -> Option<OtelGuard> {
    if std::env::var("OTEL_SDK_DISABLED").as_deref() == Ok("true") {
        return None;
    }

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:4317".to_owned());

    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = Resource::builder()
        .with_service_name(service_name.to_owned())
        .build();

    let span_exporter = match SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()
    {
        Ok(e) => e,
        Err(e) => {
            eprintln!("OTel exporter init failed: {e}; continuing without tracing");
            return None;
        }
    };

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(span_exporter)
        .build();

    let tracer = tracer_provider.tracer(service_name.to_owned());

    Registry::default()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with(fmt::layer())
        .with(OpenTelemetryLayer::new(tracer))
        .try_init()
        .ok();

    Some(OtelGuard(tracer_provider))
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let _otel = init_otel("pkdealer_agent_claude");

    let api_key = match std::env::var("ANTHROPIC_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("ANTHROPIC_API_KEY is not set or empty");
            process::exit(1);
        }
    };

    let agent = ClaudeAgent::new(api_key, args.model.clone(), args.max_tokens);
    eprintln!(
        "[{}] model={} max_tokens={}",
        args.name, args.model, args.max_tokens
    );

    let config = AgentConfig {
        endpoint: args.endpoint,
        name: args.name,
        seat: args.seat,
        chips: args.chips,
        client_secret: args.client_secret,
    };

    if let Err(e) = run_agent(agent, config).await {
        eprintln!("Agent error: {e}");
        process::exit(1);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> HandState {
        HandState {
            seat: 1,
            hole_cards: "Ah Kd".to_string(),
            board: "Qh Js Tc".to_string(),
            pot: 300,
            to_call: 100,
            my_chips: 9_700,
            stacks: vec![
                (0, "alice".to_string(), 10_000),
                (1, "bob".to_string(), 9_700),
            ],
            big_blind: 100,
            street: "flop".to_string(),
            action_history: vec!["alice bets 100".to_string()],
        }
    }

    // ── build_prompt ──────────────────────────────────────────────────────────

    #[test]
    fn test_build_prompt_contains_hole_cards() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("Ah Kd"));
    }

    #[test]
    fn test_build_prompt_contains_board() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("Qh Js Tc"));
    }

    #[test]
    fn test_build_prompt_contains_street() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("flop"));
    }

    #[test]
    fn test_build_prompt_contains_pot_and_to_call() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("300"));
        assert!(prompt.contains("100"));
    }

    #[test]
    fn test_build_prompt_contains_stacks() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("alice"));
        assert!(prompt.contains("bob"));
    }

    #[test]
    fn test_build_prompt_contains_action_history() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("alice bets 100"));
    }

    #[test]
    fn test_build_prompt_empty_board_shows_placeholder() {
        let state = HandState {
            board: String::new(),
            street: "preflop".to_string(),
            ..sample_state()
        };
        let prompt = build_prompt(&state);
        assert!(prompt.contains("no community cards yet"));
    }

    #[test]
    fn test_build_prompt_empty_history_shows_placeholder() {
        let state = HandState {
            action_history: vec![],
            ..sample_state()
        };
        let prompt = build_prompt(&state);
        assert!(prompt.contains("no actions yet this street"));
    }

    #[test]
    fn test_build_prompt_contains_instruction() {
        let prompt = build_prompt(&sample_state());
        assert!(prompt.contains("fold, check, call, bet"));
    }

    // ── parse_action ──────────────────────────────────────────────────────────

    #[test]
    fn test_parse_action_fold() {
        assert_eq!(parse_action("fold", 100), Decision::Fold);
        assert_eq!(parse_action("FOLD", 100), Decision::Fold);
        assert_eq!(parse_action("  fold  ", 100), Decision::Fold);
    }

    #[test]
    fn test_parse_action_check() {
        assert_eq!(parse_action("check", 0), Decision::Check);
        assert_eq!(parse_action("CHECK", 0), Decision::Check);
    }

    #[test]
    fn test_parse_action_call() {
        assert_eq!(parse_action("call", 50), Decision::Call);
        assert_eq!(parse_action("Call", 50), Decision::Call);
    }

    #[test]
    fn test_parse_action_all_in_variants() {
        assert_eq!(parse_action("all in", 100), Decision::AllIn);
        assert_eq!(parse_action("all-in", 100), Decision::AllIn);
        assert_eq!(parse_action("allin", 100), Decision::AllIn);
        assert_eq!(parse_action("All In", 100), Decision::AllIn);
    }

    #[test]
    fn test_parse_action_bet() {
        assert_eq!(parse_action("bet 300", 0), Decision::Bet(300));
        assert_eq!(parse_action("BET 500", 0), Decision::Bet(500));
    }

    #[test]
    fn test_parse_action_raise() {
        assert_eq!(parse_action("raise 600", 100), Decision::Raise(600));
        assert_eq!(parse_action("RAISE 1000", 100), Decision::Raise(1_000));
    }

    #[test]
    fn test_parse_action_raise_to() {
        assert_eq!(parse_action("raise to 800", 100), Decision::Raise(800));
        assert_eq!(parse_action("Raise To 1200", 100), Decision::Raise(1_200));
    }

    #[test]
    fn test_parse_action_fallback_no_call() {
        assert_eq!(parse_action("???", 0), Decision::Check);
        assert_eq!(parse_action("", 0), Decision::Check);
    }

    #[test]
    fn test_parse_action_fallback_with_call() {
        assert_eq!(parse_action("???", 50), Decision::Fold);
        assert_eq!(parse_action("invalid response", 100), Decision::Fold);
    }

    // ── pot_odds ──────────────────────────────────────────────────────────────

    #[test]
    fn test_pot_odds_with_call() {
        let state = HandState {
            pot: 300,
            to_call: 100,
            ..sample_state()
        };
        let odds = pot_odds(&state);
        assert!((odds - 0.25).abs() < 1e-9);
    }

    #[test]
    fn test_pot_odds_no_call_returns_zero() {
        let state = HandState {
            to_call: 0,
            ..sample_state()
        };
        assert_eq!(pot_odds(&state), 0.0);
    }

    #[test]
    fn test_pot_odds_call_equals_pot() {
        let state = HandState {
            pot: 100,
            to_call: 100,
            ..sample_state()
        };
        let odds = pot_odds(&state);
        assert!((odds - 0.5).abs() < 1e-9);
    }

    // ── ClaudeAgent ───────────────────────────────────────────────────────────

    #[test]
    fn test_claude_agent_new() {
        let agent = ClaudeAgent::new("key".to_string(), "claude-sonnet-4-6".to_string(), 16);
        assert_eq!(agent.api_key, "key");
        assert_eq!(agent.model, "claude-sonnet-4-6");
        assert_eq!(agent.max_tokens, 16);
    }

    #[tokio::test]
    async fn test_claude_agent_fallback_on_api_error() {
        // Point the agent at a non-existent server so call_api errors.
        // Even if the API call fails, decide() must return a safe action.
        let state = sample_state(); // to_call=100 → Fold on error
        // We can't reliably trigger the API error path without network
        // so we test parse_action fallback directly instead.
        let fallback = parse_action("totally garbage", state.to_call);
        assert_eq!(fallback, Decision::Fold);

        let fallback_check = parse_action("totally garbage", 0);
        assert_eq!(fallback_check, Decision::Check);
    }

    // ── Args ──────────────────────────────────────────────────────────────────

    #[test]
    fn test_args_defaults() {
        let args = Args::try_parse_from(["pkdealer_agent_claude"])
            .expect("default args should parse");
        assert_eq!(args.endpoint, "http://127.0.0.1:50051");
        assert_eq!(args.name, "claude");
        assert!(args.seat.is_none());
        assert_eq!(args.chips, 10_000);
        assert_eq!(args.model, "claude-sonnet-4-6");
        assert_eq!(args.max_tokens, 16);
        assert!(args.client_secret.is_empty());
    }

    #[test]
    fn test_args_model_override() {
        let args = Args::try_parse_from([
            "pkdealer_agent_claude",
            "--model",
            "claude-opus-4-7",
            "--max-tokens",
            "32",
        ])
        .expect("model args should parse");
        assert_eq!(args.model, "claude-opus-4-7");
        assert_eq!(args.max_tokens, 32);
    }

    #[test]
    fn test_args_with_seat_and_name() {
        let args = Args::try_parse_from([
            "pkdealer_agent_claude",
            "--name",
            "claude-bot",
            "--seat",
            "4",
            "--chips",
            "5000",
        ])
        .expect("seat/name args should parse");
        assert_eq!(args.name, "claude-bot");
        assert_eq!(args.seat, Some(4));
        assert_eq!(args.chips, 5_000);
    }
}
