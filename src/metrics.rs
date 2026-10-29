use axum::{extract::Request, middleware::Next, response::Response};
use opentelemetry::{
    global,
    metrics::{Counter, Histogram, Meter},
    KeyValue,
};
use std::sync::LazyLock;
use std::time::Instant;

/// Global metrics container holding registered OpenTelemetry instruments.
pub struct Metrics {
    pub http_request_count: Counter<u64>,
    pub http_request_duration: Histogram<f64>,
    pub gen_ai_token_usage: Histogram<u64>,
    pub translate_history_length: Histogram<u64>,
}

impl Metrics {
    /// Initializes all metrics instruments on the given [`Meter`].
    pub fn new(meter: &Meter) -> Self {
        let http_request_count = meter
            .u64_counter("http.server.request.count")
            .with_description("Total number of HTTP requests processed by endpoint and status code")
            .build();

        let http_request_duration = meter
            .f64_histogram("http.server.request.duration")
            .with_description("Duration of HTTP requests in seconds")
            .with_unit("s")
            .build();

        let gen_ai_token_usage = meter
            .u64_histogram("gen_ai.client.token.usage")
            .with_description("Number of tokens used in LLM API requests")
            .with_unit("{token}")
            .build();

        let translate_history_length = meter
            .u64_histogram("diwop.translate.history_length")
            .with_description("Number of prior conversation messages in /translate requests")
            .with_unit("{message}")
            .build();

        Self {
            http_request_count,
            http_request_duration,
            gen_ai_token_usage,
            translate_history_length,
        }
    }
}

pub static METRICS: LazyLock<Metrics> = LazyLock::new(|| {
    let meter = global::meter("diwop-begleitapp");
    Metrics::new(&meter)
});

/// Records HTTP request count and duration metrics.
pub fn record_http_request(endpoint: &'static str, status_code: u16, duration_secs: f64) {
    let attributes = [
        KeyValue::new("endpoint", endpoint),
        KeyValue::new("status_code", status_code as i64),
    ];
    METRICS.http_request_count.add(1, &attributes);
    METRICS
        .http_request_duration
        .record(duration_secs, &attributes);
}

/// Records token usage for upstream LLM requests by token type.
pub fn record_token_usage(
    endpoint: &'static str,
    input_tokens: u64,
    reasoning_tokens: u64,
    output_tokens: u64,
) {
    METRICS.gen_ai_token_usage.record(
        input_tokens,
        &[
            KeyValue::new("endpoint", endpoint),
            KeyValue::new("token_type", "input"),
        ],
    );
    METRICS.gen_ai_token_usage.record(
        reasoning_tokens,
        &[
            KeyValue::new("endpoint", endpoint),
            KeyValue::new("token_type", "reasoning"),
        ],
    );
    METRICS.gen_ai_token_usage.record(
        output_tokens,
        &[
            KeyValue::new("endpoint", endpoint),
            KeyValue::new("token_type", "output"),
        ],
    );
}

/// Records the number of messages in the conversation history passed to `/translate`.
pub fn record_translate_history_length(history_len: usize) {
    METRICS
        .translate_history_length
        .record(history_len as u64, &[]);
}

/// Axum middleware to track HTTP request count and duration for monitored endpoints (`/correct-transcript` and `/translate`).
pub async fn track_http_metrics(req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    let endpoint = match path.as_str() {
        "/correct-transcript" => Some("/correct-transcript"),
        "/translate" => Some("/translate"),
        _ => None,
    };

    let start = Instant::now();
    let response = next.run(req).await;

    if let Some(ep) = endpoint {
        let duration = start.elapsed().as_secs_f64();
        let status = response.status().as_u16();
        record_http_request(ep, status, duration);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};

    #[test]
    fn test_record_metrics_invocations() {
        record_http_request("/correct-transcript", 200, 0.042);
        record_http_request("/translate", 422, 0.010);
        record_token_usage("/correct-transcript", 50, 15, 25);
        record_token_usage("/translate", 120, 30, 45);
        record_translate_history_length(4);
    }

    #[test]
    fn test_metrics_collection_with_sdk() {
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone()).build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();
        let meter = provider.meter("test-meter");

        let metrics = Metrics::new(&meter);
        metrics.http_request_count.add(
            1,
            &[
                KeyValue::new("endpoint", "/correct-transcript"),
                KeyValue::new("status_code", 200i64),
            ],
        );
        metrics.http_request_duration.record(
            0.123,
            &[
                KeyValue::new("endpoint", "/correct-transcript"),
                KeyValue::new("status_code", 200i64),
            ],
        );
        metrics.gen_ai_token_usage.record(
            42,
            &[
                KeyValue::new("endpoint", "/correct-transcript"),
                KeyValue::new("token_type", "input"),
            ],
        );
        metrics.translate_history_length.record(3, &[]);

        provider.force_flush().unwrap();
        let finished_metrics = exporter.get_finished_metrics().unwrap();
        assert!(!finished_metrics.is_empty());
    }
}
