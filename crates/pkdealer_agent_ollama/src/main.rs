#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! # `pkdealer_agent_ollama` (binary)
//!
//! A poker agent that uses a locally-served Ollama model to make decisions.
//! Mirrors `pkdealer_agent_claude`: the same poker prompt is built, the same
//! response parsing is applied, only the HTTP backend differs.
//!
//! ## Usage
//!
//! ```text
//! # one-time setup:
//! #   ollama serve
//! #   ollama pull llama3.1
//!
//! cargo run --bin pkdealer_agent_ollama -- --name llama
//! ```
//!
//! ## Environment variables
//!
//! | Variable | Default | Purpose |
//! |----------|---------|---------|
//! | `OLLAMA_HOST` | `http://localhost:11434` | Ollama HTTP host |
//! | `OLLAMA_MODEL` | `llama3.1` | Ollama model identifier override |
//! | `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | gRPC service address |
//! | `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTel collector |
//! | `OTEL_SDK_DISABLED` | — | Set to `true` to skip OTel init |

use std::process;

use clap::Parser;
use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{SpanExporter, WithExportConfig as _};
use opentelemetry_sdk::{Resource, propagation::TraceContextPropagator, trace::SdkTracerProvider};
use pkdealer_agent_core::{AgentConfig, run_agent};
use pkdealer_agent_llm::LlmPokerAgent;
use pkdealer_agent_ollama::OllamaBackend;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{
    EnvFilter, Registry, fmt, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

/// Ollama LLM poker agent connected to a pkdealer gRPC service.
#[derive(Debug, Parser)]
#[command(
    name = "pkdealer_agent_ollama",
    about = "LLM poker agent powered by Ollama"
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
    #[arg(long, default_value = "ollama")]
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

    /// Ollama model identifier.
    #[arg(long, env = "OLLAMA_MODEL", default_value = "llama3.1")]
    model: String,

    /// Ollama HTTP host.
    #[arg(long, env = "OLLAMA_HOST", default_value = "http://localhost:11434")]
    host: String,
}

/// Holds the `OTel` tracer provider; flushes batched spans on drop.
struct OtelGuard(SdkTracerProvider);

impl Drop for OtelGuard {
    fn drop(&mut self) {
        let _ = self.0.shutdown();
    }
}

/// Initialises `OTel` tracing with a batched OTLP gRPC exporter.
///
/// Returns `None` when `OTEL_SDK_DISABLED=true`, leaving the caller without
/// an `OTel` subscriber (useful in tests and `cargo run` without a collector).
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
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(fmt::layer())
        .with(OpenTelemetryLayer::new(tracer))
        .try_init()
        .ok();

    Some(OtelGuard(tracer_provider))
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let _otel = init_otel("pkdealer_agent_ollama");

    let backend = OllamaBackend::new(args.host.clone(), args.model.clone());
    let agent = LlmPokerAgent::with_model(backend, "ollama", args.model.clone());

    eprintln!("[{}] host={} model={}", args.name, args.host, args.model);

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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn args_defaults() {
        let args =
            Args::try_parse_from(["pkdealer_agent_ollama"]).expect("default args should parse");
        assert_eq!(args.endpoint, "http://127.0.0.1:50051");
        assert_eq!(args.name, "ollama");
        assert!(args.seat.is_none());
        assert_eq!(args.chips, 10_000);
        assert_eq!(args.model, "llama3.1");
        assert_eq!(args.host, "http://localhost:11434");
        assert!(args.client_secret.is_empty());
    }

    #[test]
    fn args_model_and_host_override() {
        let args = Args::try_parse_from([
            "pkdealer_agent_ollama",
            "--model",
            "mistral",
            "--host",
            "http://192.168.1.10:11434",
        ])
        .expect("override args should parse");
        assert_eq!(args.model, "mistral");
        assert_eq!(args.host, "http://192.168.1.10:11434");
    }

    #[test]
    fn args_with_seat_and_name() {
        let args = Args::try_parse_from([
            "pkdealer_agent_ollama",
            "--name",
            "llama-bot",
            "--seat",
            "3",
            "--chips",
            "7500",
        ])
        .expect("seat/name args should parse");
        assert_eq!(args.name, "llama-bot");
        assert_eq!(args.seat, Some(3));
        assert_eq!(args.chips, 7_500);
    }
}
