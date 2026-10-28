use opentelemetry_sdk::{metrics::SdkMeterProvider, trace::SdkTracerProvider};
use std::net::SocketAddr;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Main entry point for the application:
///
/// - Initializes OpenTelemetry (via `OTEL_EXPORTER_OTLP_ENDPOINT`) and structured logging,
/// - configures the application router,
/// - starts the server with graceful `SIGTERM`/`SIGINT` shutdown, and
/// - flushes OpenTelemetry trace and metric providers on exit.
#[tokio::main]
async fn main() {
    let (tracer_provider, meter_provider) = init_tracing();

    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse()
        .unwrap_or(3000);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let app = diwop_begleitapp::create_app();

    info!(port = port, "Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

    if let Some(provider) = tracer_provider {
        let _ = provider.shutdown();
    }
    if let Some(provider) = meter_provider {
        let _ = provider.shutdown();
    }
}

/// Waits for `SIGINT` (Ctrl+C) or `SIGTERM` from the container orchestrator to initiate graceful shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received, draining in-flight requests...");
}

/// Initializes `tracing` subscriber layers (`stdout` logger and optional OTLP trace/metric exporters
/// when `OTEL_EXPORTER_OTLP_ENDPOINT` is set).
fn init_tracing() -> (Option<SdkTracerProvider>, Option<SdkMeterProvider>) {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env();

    // Setup OpenTelemetry Export pipeline using standard OTEL_EXPORTER_OTLP_ENDPOINT
    let mut otel_tracer_layer = None;
    let mut otel_tracer_provider = None;
    let mut otel_meter_provider = None;

    if let Ok(otel_endpoint) = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT") {
        use opentelemetry::global;
        use opentelemetry_otlp::WithExportConfig;

        let resource = opentelemetry_sdk::Resource::builder().build();

        // Build Spans Exporter
        let span_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_tonic()
            .with_endpoint(&otel_endpoint)
            .build()
            .expect("Failed to build OTLP trace exporter");

        // Trace Provider
        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(span_exporter)
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
        let meter_provider = SdkMeterProvider::builder()
            .with_reader(
                opentelemetry_sdk::metrics::PeriodicReader::builder(metric_exporter).build(),
            )
            .with_resource(resource)
            .build();

        global::set_meter_provider(meter_provider.clone());
        otel_tracer_layer = Some(tracing_opentelemetry::layer().with_tracer(tracer));
        otel_tracer_provider = Some(tracer_provider);
        otel_meter_provider = Some(meter_provider);
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
    } else if let Some(otel) = otel_tracer_layer {
        registry
            .with(otel)
            .with(tracing_subscriber::fmt::layer().json().flatten_event(true))
            .init();
    } else {
        registry
            .with(tracing_subscriber::fmt::layer().json().flatten_event(true))
            .init();
    }

    (otel_tracer_provider, otel_meter_provider)
}
