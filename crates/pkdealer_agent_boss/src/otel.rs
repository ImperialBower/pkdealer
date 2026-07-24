//! OpenTelemetry bootstrap + metric instruments for the live Boss.
//!
//! Mirrors `pkdealer_service::otel` so the boss exports through the same OTLP
//! pipeline (collector → Prometheus/Jaeger/Grafana) under its own
//! `OTEL_SERVICE_NAME`, and honors `OTEL_SDK_DISABLED=true` for local runs and
//! tests. The three instruments are the EPIC-70 Phase 4 set:
//!
//! - `pkdealer.boss.pair_llr` — gauge, the running log-likelihood ratio per
//!   suspected pair (labelled `pair`).
//! - `pkdealer.boss.flag_hand` — histogram, the hand index at which a pair first
//!   crossed the SPRT flag threshold.
//! - `pkdealer.boss.false_positive` — counter, honest pairs the boss flagged.
//!   Only ever incremented when a ground-truth labels sidecar is supplied; a
//!   genuinely blind boss (no labels) cannot know truth and leaves it at zero.

use std::error::Error;

use opentelemetry::global;
use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    Resource, metrics::SdkMeterProvider, propagation::TraceContextPropagator,
    trace::SdkTracerProvider,
};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{EnvFilter, Registry, fmt, layer::SubscriberExt, util::SubscriberInitExt};

/// RAII handle flushing the `OTel` providers on drop. Held for the process
/// lifetime by `main`.
pub struct OtelGuards {
    tracer_provider: SdkTracerProvider,
    meter_provider: SdkMeterProvider,
}

impl Drop for OtelGuards {
    fn drop(&mut self) {
        let _ = self.tracer_provider.shutdown();
        let _ = self.meter_provider.shutdown();
    }
}

/// Initializes tracing + metrics, returning `Ok(None)` when
/// `OTEL_SDK_DISABLED=true`. Mirrors `pkdealer_service::otel::init_otel`.
///
/// # Errors
///
/// Returns an error only when an OTLP exporter cannot be built (e.g. an
/// unparseable `OTEL_EXPORTER_OTLP_ENDPOINT`).
pub fn init_otel() -> Result<Option<OtelGuards>, Box<dyn Error>> {
    if std::env::var("OTEL_SDK_DISABLED").as_deref() == Ok("true") {
        return Ok(None);
    }

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:4317".to_owned());
    let service_name =
        std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "pkdealer_agent_boss".to_owned());

    global::set_text_map_propagator(TraceContextPropagator::new());
    let resource = Resource::builder().with_service_name(service_name).build();

    let span_exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()?;
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter)
        .build();
    let tracer = tracer_provider.tracer("pkdealer_agent_boss");

    let metric_exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()?;
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(metric_exporter)
        .build();
    global::set_meter_provider(meter_provider.clone());

    Registry::default()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("pkdealer_agent_boss=info,info")),
        )
        .with(fmt::layer())
        .with(OpenTelemetryLayer::new(tracer))
        .try_init()
        .ok();

    Ok(Some(OtelGuards {
        tracer_provider,
        meter_provider,
    }))
}

/// The Boss's metric instruments. Constructible from the global meter whether or
/// not [`init_otel`] set up an exporter — with no provider installed, every
/// record call is a no-op, so callers never branch on `OTel` being enabled.
pub struct Metrics {
    pair_llr: Gauge<f64>,
    flag_hand: Histogram<u64>,
    false_positive: Counter<u64>,
}

impl Metrics {
    /// Builds the instruments from the global meter provider.
    #[must_use]
    pub fn new() -> Self {
        let meter: Meter = global::meter("pkdealer_agent_boss");
        Self {
            pair_llr: meter
                .f64_gauge("pkdealer.boss.pair_llr")
                .with_description("Running per-pair log-likelihood ratio")
                .build(),
            flag_hand: meter
                .u64_histogram("pkdealer.boss.flag_hand")
                .with_description("Hand index at which a pair first crossed the flag threshold")
                .with_unit("hands")
                .build(),
            false_positive: meter
                .u64_counter("pkdealer.boss.false_positive")
                .with_description("Honest pairs flagged (requires a ground-truth labels sidecar)")
                .build(),
        }
    }

    /// Records the running LLR of one pair, labelled by its `pair` id.
    pub fn record_llr(&self, pair_label: &str, llr: f64) {
        self.pair_llr.record(
            llr,
            &[opentelemetry::KeyValue::new("pair", pair_label.to_owned())],
        );
    }

    /// Records the hand index at which a pair was flagged.
    pub fn record_flag_hand(&self, pair_label: &str, hand: u32) {
        self.flag_hand.record(
            u64::from(hand),
            &[opentelemetry::KeyValue::new("pair", pair_label.to_owned())],
        );
    }

    /// Increments the false-positive counter for a wrongly-flagged honest pair.
    pub fn record_false_positive(&self, pair_label: &str) {
        self.false_positive.add(
            1,
            &[opentelemetry::KeyValue::new("pair", pair_label.to_owned())],
        );
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}
