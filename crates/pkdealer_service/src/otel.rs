//! OpenTelemetry initialization and gRPC propagation helpers for
//! `pkdealer_service`.

use std::error::Error;
use opentelemetry::propagation::Extractor;
use tonic::metadata::MetadataMap;

/// Newtype adapter implementing [`Extractor`] over an incoming
/// [`MetadataMap`], used by the W3C TraceContext propagator to read
/// `traceparent` / `tracestate` headers from gRPC metadata.
///
/// # Examples
///
/// ```
/// use opentelemetry::propagation::Extractor;
/// use pkdealer_service::otel::MetadataExtractor;
/// use tonic::metadata::{MetadataMap, MetadataValue};
///
/// let mut md = MetadataMap::new();
/// md.insert("traceparent",
///     MetadataValue::try_from("00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01").unwrap());
/// let extractor = MetadataExtractor(&md);
/// assert!(extractor.get("traceparent").unwrap().starts_with("00-"));
/// ```
pub struct MetadataExtractor<'a>(pub &'a MetadataMap);

impl<'a> Extractor for MetadataExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0
            .iter()
            .filter_map(|kv| match kv {
                tonic::metadata::KeyAndValueRef::Ascii(k, _) => Some(k.as_str()),
                tonic::metadata::KeyAndValueRef::Binary(_, _) => None,
            })
            .collect()
    }
}

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    metrics::SdkMeterProvider,
    propagation::TraceContextPropagator,
    trace::SdkTracerProvider,
    Resource,
};
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry, fmt};

/// Holds the lifetime of the OTel tracer + meter providers. Dropping it
/// flushes batched exports and shuts down the SDK. [`init_otel`] returns
/// `None` when `OTEL_SDK_DISABLED=true` so callers can skip the rest of
/// the OTel wiring (useful in tests and CI).
pub struct OtelGuards {
    tracer_provider: SdkTracerProvider,
    meter_provider:  SdkMeterProvider,
}

impl Drop for OtelGuards {
    fn drop(&mut self) {
        let _ = self.tracer_provider.shutdown();
        let _ = self.meter_provider.shutdown();
    }
}

/// Initialises OpenTelemetry tracing + metrics.
///
/// Returns `Ok(None)` when `OTEL_SDK_DISABLED=true` so callers can skip
/// the rest of the OTel wiring.
///
/// # Errors
///
/// Returns `Err` when an OTLP exporter cannot be constructed (e.g. an
/// unparseable endpoint URL). Network failures at startup are *not*
/// errors — the exporter buffers and retries.
///
/// # Examples
///
/// ```
/// # // SAFETY: single-threaded doctest.
/// unsafe { std::env::set_var("OTEL_SDK_DISABLED", "true"); }
/// let guards = pkdealer_service::otel::init_otel().expect("disabled path");
/// assert!(guards.is_none());
/// ```
pub fn init_otel() -> Result<Option<OtelGuards>, Box<dyn Error>> {
    if std::env::var("OTEL_SDK_DISABLED").as_deref() == Ok("true") {
        // No global subscriber installed — the disabled path is purely a
        // skip. Callers that want stdout logging can wire `tracing-subscriber`
        // themselves. (Installing a fmt subscriber here would clobber the
        // thread-local subscribers integration tests rely on.)
        return Ok(None);
    }

    let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:4317".to_owned());
    let service_name = std::env::var("OTEL_SERVICE_NAME")
        .unwrap_or_else(|_| "pkdealer_service".to_owned());

    // 1. Global W3C TraceContext propagator.
    global::set_text_map_propagator(TraceContextPropagator::new());

    let resource = Resource::builder().with_service_name(service_name).build();

    // 2. Tracer provider with batched OTLP gRPC exporter.
    let span_exporter = SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()?;
    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(span_exporter)
        .build();
    let tracer = tracer_provider.tracer("pkdealer_service");

    // 3. Meter provider with periodic OTLP gRPC exporter.
    let metric_exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(&endpoint)
        .build()?;
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(metric_exporter)
        .build();
    global::set_meter_provider(meter_provider.clone());

    // 4. tracing-subscriber: env filter + fmt + OTel layer.
    Registry::default()
        .with(EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("pkdealer_service=info,info")))
        .with(fmt::layer())
        .with(OpenTelemetryLayer::new(tracer))
        .try_init()
        .ok();

    Ok(Some(OtelGuards { tracer_provider, meter_provider }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_otel_with_disabled_flag_is_noop() {
        // SAFETY: tests in this module are run with `--test-threads=1`;
        // env mutation is synchronous and confined to this test.
        unsafe { std::env::set_var("OTEL_SDK_DISABLED", "true"); }
        let guards = init_otel().expect("disabled path is infallible");
        assert!(guards.is_none(), "disabled flag must short-circuit init");
        unsafe { std::env::remove_var("OTEL_SDK_DISABLED"); }
    }

    #[test]
    fn metadata_extractor_round_trips_traceparent() {
        use opentelemetry::propagation::{Extractor, TextMapPropagator};
        use opentelemetry::trace::TraceContextExt;
        use opentelemetry_sdk::propagation::TraceContextPropagator;
        use tonic::metadata::{MetadataMap, MetadataValue};

        // Inject a known traceparent into a MetadataMap.
        let mut md = MetadataMap::new();
        let trace_id = "0af7651916cd43dd8448eb211c80319c";
        let span_id  = "b7ad6b7169203331";
        let header = format!("00-{trace_id}-{span_id}-01");
        md.insert("traceparent", MetadataValue::try_from(header).unwrap());

        // Sanity-check the Extractor::get impl returns the header value.
        let extractor = MetadataExtractor(&md);
        assert!(extractor.get("traceparent").unwrap().contains(trace_id));

        // Run the W3C propagator over our extractor.
        let propagator = TraceContextPropagator::new();
        let ctx = propagator.extract(&extractor);
        let span = ctx.span();
        let sc = span.span_context();
        assert!(sc.is_valid(), "propagator must produce a valid SpanContext");
        assert_eq!(sc.trace_id().to_string(), trace_id);
        assert_eq!(sc.span_id().to_string(),  span_id);
    }
}
