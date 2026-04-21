use std::net::SocketAddr;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Main entry point for the application:
///
/// - Initializes Open Telemetry and logging,
/// - configures the application router and
/// - starts the server.
#[tokio::main]
async fn main() {
    init_tracing();

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse()
        .unwrap_or(3000);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let app = diwop_begleitapp::create_app();

    info!(port = port, "Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env();

    // Setup OpenTelemetry Export pipeline
    let mut otel_tracer_layer = None;

    if let Ok(otel_endpoint) = std::env::var("OPEN_TELEMETRY_URL") {
        use opentelemetry::global;
        use opentelemetry_otlp::WithExportConfig;

        let resource = opentelemetry_sdk::Resource::default();

        // Build Spans Exporter
        let span_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&otel_endpoint)
            .build()
            .expect("Failed to build OTLP trace exporter");

        // Trace Provider
        let tracer_provider = opentelemetry_sdk::trace::TracerProvider::builder()
            .with_batch_exporter(span_exporter, opentelemetry_sdk::runtime::Tokio)
            .build();

        use opentelemetry::trace::TracerProvider;
        let tracer = tracer_provider.tracer("diwop-begleitapp");

        // Build Metrics Exporter
        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_tonic()
            .with_endpoint(&otel_endpoint)
            .build()
            .expect("Failed to build OTLP metric exporter");

        // Metrics Provider natively via OTLP pipeline
        let meter_provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
            .with_reader(
                opentelemetry_sdk::metrics::PeriodicReader::builder(
                    metric_exporter,
                    opentelemetry_sdk::runtime::Tokio,
                )
                .build(),
            )
            .with_resource(resource)
            .build();

        global::set_meter_provider(meter_provider);
        otel_tracer_layer = Some(tracing_opentelemetry::layer().with_tracer(tracer));
    }

    fn default_filter() -> EnvFilter {
        let default_log_level = if cfg!(debug_assertions) {
            "debug"
        } else {
            "info"
        };
        format!(
            "diwop_begleitapp={},tower_http={}",
            default_log_level, default_log_level
        )
        .into()
    }

    let filter = env_filter.unwrap_or_else(|_| default_filter());
    let registry = tracing_subscriber::registry().with(filter);

    if cfg!(debug_assertions) {
        if let Some(otel) = otel_tracer_layer {
            registry
                .with(otel)
                .with(tracing_subscriber::fmt::layer())
                .init();
        } else {
            registry.with(tracing_subscriber::fmt::layer()).init();
        }
    } else {
        if let Some(otel) = otel_tracer_layer {
            registry
                .with(otel)
                .with(tracing_subscriber::fmt::layer().json().flatten_event(true))
                .init();
        } else {
            registry
                .with(tracing_subscriber::fmt::layer().json().flatten_event(true))
                .init();
        }
    }
}
