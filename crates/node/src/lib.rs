//! Composition root: wires the sequencer, spawns the periodic **batch-sealing
//! loop** (the block-less "tick"), and serves the GraphQL API. No business logic.
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use async_graphql::http::GraphiQLSource;
use async_graphql_axum::GraphQL;
use axum::{
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use metrics_exporter_prometheus::PrometheusBuilder;
use tokio::task::JoinHandle;
use tracing::warn;
use weakseq_api::{build_schema, Sequencer, SequencerConfig};
use weakseq_types::ValidatorSet;

/// Node configuration.
#[derive(Clone, Debug)]
pub struct NodeConfig {
    pub listen_addr: String,
    pub validators: u64,
    pub batch_interval_ms: u64,
    pub max_orders_per_sec: u32,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:8081".into(),
            validators: 4,
            batch_interval_ms: 250,
            max_orders_per_sec: 50_000,
        }
    }
}

/// Build the sequencer from configuration (pure wiring).
#[must_use]
pub fn build_sequencer(config: &NodeConfig) -> Arc<Sequencer> {
    let validators = ValidatorSet::uniform(config.validators.max(1));
    Arc::new(Sequencer::new(
        validators,
        SequencerConfig {
            max_orders_per_sec: config.max_orders_per_sec,
            honest_validators: u64::MAX,
        },
    ))
}

/// Spawn the periodic sealing loop — WeakSeq's block-less "clock". Each tick
/// seals whatever is in the mempool into a batch and finalizes it.
pub fn spawn_sealing_loop(sequencer: Arc<Sequencer>, interval: Duration) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            match sequencer.seal_and_clear() {
                Ok(Some(batch)) => {
                    metrics::counter!("weakseq_batches_confirmed_total").increment(1);
                    metrics::histogram!("weakseq_batch_matched_qty")
                        .record(batch.result.matched_quantity.lots() as f64);
                }
                Ok(None) => {}
                Err(e) => warn!(error = %e, "sealing failed"),
            }
        }
    })
}

/// Build the axum router.
pub fn build_router(sequencer: Arc<Sequencer>) -> Router {
    let schema = build_schema(sequencer);
    Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .route("/graphql", get(graphiql).post_service(GraphQL::new(schema)))
}

async fn health() -> impl IntoResponse {
    metrics::counter!("weakseq_health_checks_total").increment(1);
    "ok"
}

async fn graphiql() -> impl IntoResponse {
    Html(GraphiQLSource::build().endpoint("/graphql").finish())
}

async fn metrics_handler() -> impl IntoResponse {
    match PROM_HANDLE.get() {
        Some(h) => h.render(),
        None => String::new(),
    }
}

use std::sync::OnceLock;
static PROM_HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
static OTEL_PROVIDER: OnceLock<opentelemetry_sdk::trace::TracerProvider> = OnceLock::new();

/// Install tracing (JSON) + Prometheus recorder, and — when
/// `OTEL_EXPORTER_OTLP_ENDPOINT` is set — an OpenTelemetry OTLP trace exporter
/// (gRPC) so spans flow to an OpenTelemetry Collector.
pub fn init_observability() -> anyhow::Result<()> {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let otel_layer = build_otel_layer("weakseq-node")?;
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().json())
        .with(otel_layer)
        .try_init();
    if PROM_HANDLE.get().is_none() {
        let handle = PrometheusBuilder::new().install_recorder()?;
        let _ = PROM_HANDLE.set(handle);
    }
    Ok(())
}

/// Build an OpenTelemetry tracing layer if an OTLP endpoint is configured.
/// Returns `Ok(None)` (no-op) when `OTEL_EXPORTER_OTLP_ENDPOINT` is unset.
#[allow(clippy::type_complexity)]
fn build_otel_layer<S>(
    service_name: &'static str,
) -> anyhow::Result<
    Option<tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>>,
>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry::KeyValue;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::{trace::TracerProvider, Resource};

    let Ok(endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") else {
        return Ok(None);
    };
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;
    let resource = Resource::new(vec![
        KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_NAME,
            service_name,
        ),
        KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_VERSION,
            env!("CARGO_PKG_VERSION"),
        ),
    ]);
    let provider = TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(resource)
        .build();
    let tracer = provider.tracer(service_name);
    let _ = OTEL_PROVIDER.set(provider);
    Ok(Some(tracing_opentelemetry::layer().with_tracer(tracer)))
}

/// Flush and shut down the OpenTelemetry exporter on graceful shutdown.
pub fn shutdown_observability() {
    if let Some(provider) = OTEL_PROVIDER.get() {
        let _ = provider.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequencer_builds() {
        let s = build_sequencer(&NodeConfig::default());
        assert_eq!(s.validator_count(), 4);
    }

    #[tokio::test]
    async fn sealing_loop_confirms_batches() {
        let s = build_sequencer(&NodeConfig::default());
        s.submit_order(weakseq_types::Side::Buy, 100, 5).unwrap();
        s.submit_order(weakseq_types::Side::Sell, 90, 5).unwrap();
        let handle = spawn_sealing_loop(s.clone(), Duration::from_millis(10));
        // Give the loop a couple of ticks.
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.abort();
        assert!(s.confirmed_count() >= 1);
    }

    #[tokio::test]
    async fn router_builds() {
        let s = build_sequencer(&NodeConfig::default());
        let _ = build_router(s);
    }
}
