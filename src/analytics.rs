use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default monitored API endpoints.
pub const DEFAULT_ENDPOINTS: [&str; 2] = ["/correct-transcript", "/translate"];

/// Default monitored LLM prompts in pipeline order.
pub const DEFAULT_PROMPTS: [&str; 3] = ["correct-transcript", "translate", "classify"];

/// Preset time range selection for the analytics dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TimeRange {
    #[serde(rename = "1h")]
    OneHour,
    #[default]
    #[serde(rename = "24h")]
    TwentyFourHours,
    #[serde(rename = "7d")]
    SevenDays,
    #[serde(rename = "30d")]
    ThirtyDays,
}

impl TimeRange {
    /// Returns the duration corresponding to this time window.
    pub fn duration(&self) -> Duration {
        match self {
            TimeRange::OneHour => Duration::from_secs(3600),
            TimeRange::TwentyFourHours => Duration::from_secs(86400),
            TimeRange::SevenDays => Duration::from_secs(7 * 86400),
            TimeRange::ThirtyDays => Duration::from_secs(30 * 86400),
        }
    }

    /// Returns `(bucket_duration_secs, num_buckets)` used to build time-series curves over this range.
    pub fn bucket_config(&self) -> (u64, usize) {
        match self {
            TimeRange::OneHour => (300, 12),          // 12 x 5 minutes = 1h
            TimeRange::TwentyFourHours => (3600, 24), // 24 x 1 hour = 24h
            TimeRange::SevenDays => (86400, 7),       // 7 x 1 day = 7d
            TimeRange::ThirtyDays => (86400, 30),     // 30 x 1 day = 30d
        }
    }

    /// String representation matching query parameter values (`"1h"`, `"24h"`, `"7d"`, `"30d"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            TimeRange::OneHour => "1h",
            TimeRange::TwentyFourHours => "24h",
            TimeRange::SevenDays => "7d",
            TimeRange::ThirtyDays => "30d",
        }
    }

    /// Parses a string query parameter into a [`TimeRange`].
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "1h" => Some(TimeRange::OneHour),
            "24h" => Some(TimeRange::TwentyFourHours),
            "7d" => Some(TimeRange::SevenDays),
            "30d" => Some(TimeRange::ThirtyDays),
            _ => None,
        }
    }
}

/// High-level KPI summary cards displayed at the top of the analytics dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalyticsSummary {
    pub total_requests: u64,
    pub error_rate_percent: f64,
    pub avg_latency_ms: f64,
    pub total_tokens: u64,
}

/// HTTP request counts grouped by API endpoint and response status class (2xx, 4xx, 5xx).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndpointStatusMetrics {
    pub endpoint: String,
    pub status_2xx: u64,
    pub status_4xx: u64,
    pub status_5xx: u64,
}

/// A single time bucket of HTTP status counts for an endpoint's request curve over time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimeSeriesStatusPoint {
    /// Bucket timestamp in seconds since UNIX_EPOCH.
    pub timestamp: u64,
    pub status_2xx: u64,
    pub status_4xx: u64,
    pub status_5xx: u64,
}

/// Time-series of HTTP request counts by status class for a specific API endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndpointTimeSeriesMetrics {
    pub endpoint: String,
    pub series: Vec<TimeSeriesStatusPoint>,
}

/// Request latency in milliseconds grouped by API endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndpointLatencyMetrics {
    pub endpoint: String,
    pub avg_ms: f64,
    pub p95_ms: f64,
}

/// Token usage grouped by LLM prompt (`correct-transcript`, `translate`, `classify`) and token type (input, reasoning, response).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptTokenMetrics {
    pub prompt: String,
    pub input_tokens: u64,
    pub reasoning_tokens: u64,
    pub response_tokens: u64,
}

/// Translation request count for a specific dialog topic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopicMetric {
    pub topic: String,
    pub count: u64,
}

/// Translation request count for an alert level category (`ok`, `info`, `warning`, `danger`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CategoryMetric {
    pub category: String,
    pub count: u64,
}

/// Translation request count by classification type (`translation`, `formatting`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClassificationTypeMetric {
    pub classification_type: String,
    pub count: u64,
}

/// Distribution bucket for prior conversation history message length in `/translate` requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryLengthBucket {
    pub bucket: String,
    pub count: u64,
}

/// Aggregated analytics report containing summary statistics and chart datasets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalyticsReport {
    pub time_range: String,
    pub summary: AnalyticsSummary,
    pub requests_by_endpoint: Vec<EndpointStatusMetrics>,
    pub requests_over_time: Vec<EndpointTimeSeriesMetrics>,
    pub latencies_by_endpoint: Vec<EndpointLatencyMetrics>,
    pub token_usage_by_prompt: Vec<PromptTokenMetrics>,
    pub translation_by_topic: Vec<TopicMetric>,
    pub translation_by_category: Vec<CategoryMetric>,
    pub translation_by_type: Vec<ClassificationTypeMetric>,
    pub history_length_distribution: Vec<HistoryLengthBucket>,
}

impl AnalyticsReport {
    /// Creates an empty analytics report with zeroed counts, default endpoints, and default prompts.
    pub fn empty(time_range: TimeRange) -> Self {
        Self::empty_at(time_range, SystemTime::now())
    }

    /// Creates an empty analytics report anchored at `now` for time-series bucket timestamps.
    pub fn empty_at(time_range: TimeRange, now: SystemTime) -> Self {
        let now_secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let (bucket_secs, num_buckets) = time_range.bucket_config();
        let total_window = bucket_secs * (num_buckets as u64);
        let start_secs = now_secs.saturating_sub(total_window);

        let requests_by_endpoint = DEFAULT_ENDPOINTS
            .iter()
            .map(|ep| EndpointStatusMetrics {
                endpoint: (*ep).to_string(),
                status_2xx: 0,
                status_4xx: 0,
                status_5xx: 0,
            })
            .collect();

        let requests_over_time = DEFAULT_ENDPOINTS
            .iter()
            .map(|ep| {
                let series = (0..num_buckets)
                    .map(|i| TimeSeriesStatusPoint {
                        timestamp: start_secs + ((i as u64 + 1) * bucket_secs),
                        status_2xx: 0,
                        status_4xx: 0,
                        status_5xx: 0,
                    })
                    .collect();
                EndpointTimeSeriesMetrics {
                    endpoint: (*ep).to_string(),
                    series,
                }
            })
            .collect();

        let latencies_by_endpoint = DEFAULT_ENDPOINTS
            .iter()
            .map(|ep| EndpointLatencyMetrics {
                endpoint: (*ep).to_string(),
                avg_ms: 0.0,
                p95_ms: 0.0,
            })
            .collect();

        let token_usage_by_prompt = DEFAULT_PROMPTS
            .iter()
            .map(|p| PromptTokenMetrics {
                prompt: (*p).to_string(),
                input_tokens: 0,
                reasoning_tokens: 0,
                response_tokens: 0,
            })
            .collect();

        let default_topics = [
            "medical",
            "shopping",
            "social",
            "education",
            "emergency",
            "authorities",
            "financial",
            "mobility",
            "juridical",
            "other",
        ];
        let translation_by_topic = default_topics
            .iter()
            .map(|t| TopicMetric {
                topic: t.to_string(),
                count: 0,
            })
            .collect();

        let default_categories = ["ok", "info", "warning", "danger"];
        let translation_by_category = default_categories
            .iter()
            .map(|c| CategoryMetric {
                category: c.to_string(),
                count: 0,
            })
            .collect();

        let default_types = ["translation", "formatting"];
        let translation_by_type = default_types
            .iter()
            .map(|t| ClassificationTypeMetric {
                classification_type: t.to_string(),
                count: 0,
            })
            .collect();

        let default_history_buckets = ["0", "1-2", "3-5", "6-10", "11+"];
        let history_length_distribution = default_history_buckets
            .iter()
            .map(|b| HistoryLengthBucket {
                bucket: b.to_string(),
                count: 0,
            })
            .collect();

        Self {
            time_range: time_range.as_str().to_string(),
            summary: AnalyticsSummary {
                total_requests: 0,
                error_rate_percent: 0.0,
                avg_latency_ms: 0.0,
                total_tokens: 0,
            },
            requests_by_endpoint,
            requests_over_time,
            latencies_by_endpoint,
            token_usage_by_prompt,
            translation_by_topic,
            translation_by_category,
            translation_by_type,
            history_length_distribution,
        }
    }
}

/// Errors that can occur when querying analytics metrics.
#[derive(Debug)]
pub enum AnalyticsError {
    CloudWatchError(String),
    QueryError(String),
}

impl fmt::Display for AnalyticsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnalyticsError::CloudWatchError(msg) => write!(f, "CloudWatch analytics error: {msg}"),
            AnalyticsError::QueryError(msg) => write!(f, "Analytics query error: {msg}"),
        }
    }
}

impl std::error::Error for AnalyticsError {}

/// Abstract strategy service for querying server analytics.
#[async_trait]
#[allow(clippy::double_must_use)]
pub trait MetricsQueryService: Send + Sync {
    /// Queries analytics and returns aggregated report metrics for the requested [`TimeRange`].
    async fn query_analytics(&self, range: TimeRange) -> Result<AnalyticsReport, AnalyticsError>;
}

// ---------------------------------------------------------------------------
// In-Memory Implementation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct HttpEvent {
    timestamp: SystemTime,
    endpoint: String,
    status_code: u16,
    duration_secs: f64,
}

#[derive(Debug, Clone)]
struct TokenEvent {
    timestamp: SystemTime,
    prompt: String,
    input_tokens: u64,
    reasoning_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Clone)]
struct HistoryEvent {
    timestamp: SystemTime,
    history_length: u64,
}

#[derive(Debug, Clone)]
struct ClassificationEvent {
    timestamp: SystemTime,
    topic: String,
    classification_type: String,
    category: String,
}

/// Thread-safe in-memory buffer storing recent telemetry events for local development and self-contained environments.
#[derive(Default)]
pub struct InMemoryMetricsStore {
    http_events: RwLock<Vec<HttpEvent>>,
    token_events: RwLock<Vec<TokenEvent>>,
    history_events: RwLock<Vec<HistoryEvent>>,
    classification_events: RwLock<Vec<ClassificationEvent>>,
}

impl InMemoryMetricsStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an HTTP request execution.
    pub fn record_http_request(&self, endpoint: &str, status_code: u16, duration_secs: f64) {
        if let Ok(mut events) = self.http_events.write() {
            events.push(HttpEvent {
                timestamp: SystemTime::now(),
                endpoint: endpoint.to_string(),
                status_code,
                duration_secs,
            });
            // Keep buffer bounded (e.g. max 50,000 events)
            if events.len() > 50_000 {
                let remove_count = events.len() - 40_000;
                events.drain(0..remove_count);
            }
        }
    }

    /// Records token consumption for an upstream LLM prompt call.
    pub fn record_token_usage(
        &self,
        prompt: &str,
        input_tokens: u64,
        reasoning_tokens: u64,
        output_tokens: u64,
    ) {
        if let Ok(mut events) = self.token_events.write() {
            events.push(TokenEvent {
                timestamp: SystemTime::now(),
                prompt: prompt.to_string(),
                input_tokens,
                reasoning_tokens,
                output_tokens,
            });
            if events.len() > 50_000 {
                let remove_count = events.len() - 40_000;
                events.drain(0..remove_count);
            }
        }
    }

    /// Records conversation message history length.
    pub fn record_history_length(&self, length: u64) {
        if let Ok(mut events) = self.history_events.write() {
            events.push(HistoryEvent {
                timestamp: SystemTime::now(),
                history_length: length,
            });
            if events.len() > 50_000 {
                let remove_count = events.len() - 40_000;
                events.drain(0..remove_count);
            }
        }
    }

    /// Records dialog classification output.
    pub fn record_classification(&self, topic: &str, classification_type: &str, category: &str) {
        if let Ok(mut events) = self.classification_events.write() {
            events.push(ClassificationEvent {
                timestamp: SystemTime::now(),
                topic: topic.to_string(),
                classification_type: classification_type.to_string(),
                category: category.to_string(),
            });
            if events.len() > 50_000 {
                let remove_count = events.len() - 40_000;
                events.drain(0..remove_count);
            }
        }
    }

    /// Generates an aggregated [`AnalyticsReport`] for events within `range`.
    pub fn generate_report(&self, range: TimeRange) -> AnalyticsReport {
        let now = SystemTime::now();
        let cutoff = now.checked_sub(range.duration()).unwrap_or(now);
        let now_secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let (bucket_secs, num_buckets) = range.bucket_config();
        let total_window = bucket_secs * (num_buckets as u64);
        let start_secs = now_secs.saturating_sub(total_window);

        let mut report = AnalyticsReport::empty_at(range, now);

        // Map for endpoint -> (2xx, 4xx, 5xx, durations_ms)
        let mut endpoint_stats: HashMap<String, (u64, u64, u64, Vec<f64>)> = HashMap::new();
        for ep in DEFAULT_ENDPOINTS {
            endpoint_stats.insert(ep.to_string(), (0, 0, 0, Vec::new()));
        }

        let mut total_requests = 0u64;
        let mut total_errors = 0u64;
        let mut all_durations_ms: Vec<f64> = Vec::new();

        if let Ok(events) = self.http_events.read() {
            for ev in events.iter().filter(|e| e.timestamp >= cutoff) {
                total_requests += 1;
                let dur_ms = ev.duration_secs * 1000.0;
                all_durations_ms.push(dur_ms);

                let entry = endpoint_stats
                    .entry(ev.endpoint.clone())
                    .or_insert_with(|| (0, 0, 0, Vec::new()));
                entry.3.push(dur_ms);

                let ev_secs = ev
                    .timestamp
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let bucket_idx = ((ev_secs.saturating_sub(start_secs) / bucket_secs) as usize)
                    .min(num_buckets - 1);

                let series_entry = if let Some(existing) = report
                    .requests_over_time
                    .iter_mut()
                    .find(|s| s.endpoint == ev.endpoint)
                {
                    existing
                } else {
                    let series = (0..num_buckets)
                        .map(|i| TimeSeriesStatusPoint {
                            timestamp: start_secs + ((i as u64 + 1) * bucket_secs),
                            status_2xx: 0,
                            status_4xx: 0,
                            status_5xx: 0,
                        })
                        .collect();
                    report.requests_over_time.push(EndpointTimeSeriesMetrics {
                        endpoint: ev.endpoint.clone(),
                        series,
                    });
                    report.requests_over_time.last_mut().unwrap()
                };

                if ev.status_code >= 200 && ev.status_code < 300 {
                    entry.0 += 1;
                    if let Some(pt) = series_entry.series.get_mut(bucket_idx) {
                        pt.status_2xx += 1;
                    }
                } else if ev.status_code >= 400 && ev.status_code < 500 {
                    entry.1 += 1;
                    total_errors += 1;
                    if let Some(pt) = series_entry.series.get_mut(bucket_idx) {
                        pt.status_4xx += 1;
                    }
                } else if ev.status_code >= 500 {
                    entry.2 += 1;
                    total_errors += 1;
                    if let Some(pt) = series_entry.series.get_mut(bucket_idx) {
                        pt.status_5xx += 1;
                    }
                }
            }
        }

        // Token usage by prompt
        let mut token_stats: HashMap<String, (u64, u64, u64)> = HashMap::new();
        for p in DEFAULT_PROMPTS {
            token_stats.insert(p.to_string(), (0, 0, 0));
        }
        let mut total_tokens = 0u64;

        if let Ok(events) = self.token_events.read() {
            for ev in events.iter().filter(|e| e.timestamp >= cutoff) {
                total_tokens += ev.input_tokens + ev.reasoning_tokens + ev.output_tokens;
                let entry = token_stats.entry(ev.prompt.clone()).or_insert((0, 0, 0));
                entry.0 += ev.input_tokens;
                entry.1 += ev.reasoning_tokens;
                entry.2 += ev.output_tokens;
            }
        }

        // History lengths
        let mut history_buckets: HashMap<&'static str, u64> = HashMap::new();
        history_buckets.insert("0", 0);
        history_buckets.insert("1-2", 0);
        history_buckets.insert("3-5", 0);
        history_buckets.insert("6-10", 0);
        history_buckets.insert("11+", 0);

        if let Ok(events) = self.history_events.read() {
            for ev in events.iter().filter(|e| e.timestamp >= cutoff) {
                let bucket = match ev.history_length {
                    0 => "0",
                    1..=2 => "1-2",
                    3..=5 => "3-5",
                    6..=10 => "6-10",
                    _ => "11+",
                };
                *history_buckets.entry(bucket).or_insert(0) += 1;
            }
        }

        // Classifications
        let mut topic_counts: HashMap<String, u64> = HashMap::new();
        let mut category_counts: HashMap<String, u64> = HashMap::new();
        let mut type_counts: HashMap<String, u64> = HashMap::new();

        if let Ok(events) = self.classification_events.read() {
            for ev in events.iter().filter(|e| e.timestamp >= cutoff) {
                *topic_counts.entry(ev.topic.clone()).or_insert(0) += 1;
                *category_counts.entry(ev.category.clone()).or_insert(0) += 1;
                *type_counts
                    .entry(ev.classification_type.clone())
                    .or_insert(0) += 1;
            }
        }

        // Summary calculations
        let error_rate_percent = if total_requests > 0 {
            (total_errors as f64 / total_requests as f64) * 100.0
        } else {
            0.0
        };

        let avg_latency_ms = if !all_durations_ms.is_empty() {
            all_durations_ms.iter().sum::<f64>() / all_durations_ms.len() as f64
        } else {
            0.0
        };

        report.summary = AnalyticsSummary {
            total_requests,
            error_rate_percent: (error_rate_percent * 10.0).round() / 10.0,
            avg_latency_ms: (avg_latency_ms * 10.0).round() / 10.0,
            total_tokens,
        };

        // Populate requests by endpoint
        report.requests_by_endpoint = endpoint_stats
            .iter()
            .map(|(ep, (s2, s4, s5, _))| EndpointStatusMetrics {
                endpoint: ep.clone(),
                status_2xx: *s2,
                status_4xx: *s4,
                status_5xx: *s5,
            })
            .collect();
        report
            .requests_by_endpoint
            .sort_by(|a, b| a.endpoint.cmp(&b.endpoint));

        // Populate latency metrics
        report.latencies_by_endpoint = endpoint_stats
            .iter()
            .map(|(ep, (_, _, _, durs))| {
                let (avg, p95) = if durs.is_empty() {
                    (0.0, 0.0)
                } else {
                    let mut sorted = durs.clone();
                    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                    let avg = sorted.iter().sum::<f64>() / sorted.len() as f64;
                    let p95_idx = ((sorted.len() as f64 * 0.95).ceil() as usize).saturating_sub(1);
                    (avg, sorted[p95_idx])
                };
                EndpointLatencyMetrics {
                    endpoint: ep.clone(),
                    avg_ms: (avg * 10.0).round() / 10.0,
                    p95_ms: (p95 * 10.0).round() / 10.0,
                }
            })
            .collect();
        report
            .latencies_by_endpoint
            .sort_by(|a, b| a.endpoint.cmp(&b.endpoint));

        // Populate token usage by prompt (preserving DEFAULT_PROMPTS pipeline order first)
        let mut prompt_metrics = Vec::with_capacity(token_stats.len());
        for p in DEFAULT_PROMPTS {
            if let Some((inp, rsn, out)) = token_stats.remove(p) {
                prompt_metrics.push(PromptTokenMetrics {
                    prompt: p.to_string(),
                    input_tokens: inp,
                    reasoning_tokens: rsn,
                    response_tokens: out,
                });
            }
        }
        let mut extra_prompts: Vec<_> = token_stats
            .into_iter()
            .map(|(p, (inp, rsn, out))| PromptTokenMetrics {
                prompt: p,
                input_tokens: inp,
                reasoning_tokens: rsn,
                response_tokens: out,
            })
            .collect();
        extra_prompts.sort_by(|a, b| a.prompt.cmp(&b.prompt));
        prompt_metrics.extend(extra_prompts);
        report.token_usage_by_prompt = prompt_metrics;

        // Populate topic counts (preserve default ordering or merge)
        for item in &mut report.translation_by_topic {
            if let Some(c) = topic_counts.remove(&item.topic) {
                item.count = c;
            }
        }
        for (extra_topic, c) in topic_counts {
            report.translation_by_topic.push(TopicMetric {
                topic: extra_topic,
                count: c,
            });
        }

        // Populate category counts
        for item in &mut report.translation_by_category {
            if let Some(c) = category_counts.remove(&item.category) {
                item.count = c;
            }
        }

        // Populate classification types
        for item in &mut report.translation_by_type {
            if let Some(c) = type_counts.remove(&item.classification_type) {
                item.count = c;
            }
        }

        // Populate history length distribution
        for item in &mut report.history_length_distribution {
            if let Some(&c) = history_buckets.get(item.bucket.as_str()) {
                item.count = c;
            }
        }

        report
    }
}

/// Global in-memory metrics store instance.
pub static IN_MEMORY_METRICS_STORE: LazyLock<Arc<InMemoryMetricsStore>> =
    LazyLock::new(|| Arc::new(InMemoryMetricsStore::new()));

/// In-memory implementation of [`MetricsQueryService`].
pub struct InMemoryMetricsProvider {
    store: Arc<InMemoryMetricsStore>,
}

impl InMemoryMetricsProvider {
    pub fn new(store: Arc<InMemoryMetricsStore>) -> Self {
        Self { store }
    }

    pub fn global() -> Self {
        Self {
            store: Arc::clone(&IN_MEMORY_METRICS_STORE),
        }
    }
}

#[async_trait]
impl MetricsQueryService for InMemoryMetricsProvider {
    async fn query_analytics(&self, range: TimeRange) -> Result<AnalyticsReport, AnalyticsError> {
        Ok(self.store.generate_report(range))
    }
}

// ---------------------------------------------------------------------------
// STACKIT Stub Implementation
// ---------------------------------------------------------------------------

/// STACKIT metrics provider stub returning zeroed analytics.
pub struct StackitMetricsProvider;

impl StackitMetricsProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StackitMetricsProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MetricsQueryService for StackitMetricsProvider {
    async fn query_analytics(&self, range: TimeRange) -> Result<AnalyticsReport, AnalyticsError> {
        tracing::debug!(
            provider = "stackit",
            range = range.as_str(),
            "Returning empty analytics report for STACKIT stub"
        );
        Ok(AnalyticsReport::empty(range))
    }
}

// ---------------------------------------------------------------------------
// AWS CloudWatch Implementation
// ---------------------------------------------------------------------------

/// AWS CloudWatch metrics query provider using `aws-sdk-cloudwatch`.
pub struct CloudWatchMetricsProvider {
    client: tokio::sync::OnceCell<aws_sdk_cloudwatch::Client>,
    namespace: String,
}

impl CloudWatchMetricsProvider {
    /// Initializes a new CloudWatch query provider using the ambient AWS configuration on first query.
    pub fn new(namespace: String) -> Self {
        Self {
            client: tokio::sync::OnceCell::new(),
            namespace,
        }
    }

    pub fn with_client(client: aws_sdk_cloudwatch::Client, namespace: String) -> Self {
        let cell = tokio::sync::OnceCell::new();
        let _ = cell.set(client);
        Self {
            client: cell,
            namespace,
        }
    }

    async fn get_client(&self) -> &aws_sdk_cloudwatch::Client {
        self.client
            .get_or_init(|| async {
                let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
                aws_sdk_cloudwatch::Client::new(&config)
            })
            .await
    }
}

#[async_trait]
impl MetricsQueryService for CloudWatchMetricsProvider {
    async fn query_analytics(&self, range: TimeRange) -> Result<AnalyticsReport, AnalyticsError> {
        use aws_sdk_cloudwatch::types::{Dimension, Metric, MetricDataQuery, MetricStat};

        let now = SystemTime::now();
        let start_time = now.checked_sub(range.duration()).unwrap_or(now);

        let now_secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        let (bucket_secs, num_buckets) = range.bucket_config();
        let total_window = bucket_secs * (num_buckets as u64);
        let start_secs = now_secs.saturating_sub(total_window);

        let start_dt = aws_sdk_cloudwatch::primitives::DateTime::from(start_time);
        let end_dt = aws_sdk_cloudwatch::primitives::DateTime::from(now);
        let period_secs = bucket_secs as i32;

        let mut queries = Vec::new();

        // 1. Total request count
        queries.push(
            MetricDataQuery::builder()
                .id("total_req")
                .metric_stat(
                    MetricStat::builder()
                        .metric(
                            Metric::builder()
                                .namespace(&self.namespace)
                                .metric_name("http.server.request.count")
                                .build(),
                        )
                        .period(period_secs)
                        .stat("Sum")
                        .build(),
                )
                .return_data(true)
                .build(),
        );

        // 2. Endpoint status codes (time-series & totals) & latencies
        let status_classes = ["2xx", "4xx", "5xx"];

        for (ep_idx, ep) in DEFAULT_ENDPOINTS.iter().enumerate() {
            for sc_name in status_classes {
                let id = format!("req_{ep_idx}_{sc_name}");
                queries.push(
                    MetricDataQuery::builder()
                        .id(id)
                        .metric_stat(
                            MetricStat::builder()
                                .metric(
                                    Metric::builder()
                                        .namespace(&self.namespace)
                                        .metric_name("http.server.request.count")
                                        .dimensions(
                                            Dimension::builder()
                                                .name("endpoint")
                                                .value(*ep)
                                                .build(),
                                        )
                                        .dimensions(
                                            Dimension::builder()
                                                .name("status_class")
                                                .value(sc_name)
                                                .build(),
                                        )
                                        .build(),
                                )
                                .period(period_secs)
                                .stat("Sum")
                                .build(),
                        )
                        .return_data(true)
                        .build(),
                );
            }

            // Latency Average & p95
            queries.push(
                MetricDataQuery::builder()
                    .id(format!("lat_avg_{ep_idx}"))
                    .metric_stat(
                        MetricStat::builder()
                            .metric(
                                Metric::builder()
                                    .namespace(&self.namespace)
                                    .metric_name("http.server.request.duration")
                                    .dimensions(
                                        Dimension::builder().name("endpoint").value(*ep).build(),
                                    )
                                    .build(),
                            )
                            .period(period_secs)
                            .stat("Average")
                            .build(),
                    )
                    .return_data(true)
                    .build(),
            );
            queries.push(
                MetricDataQuery::builder()
                    .id(format!("lat_p95_{ep_idx}"))
                    .metric_stat(
                        MetricStat::builder()
                            .metric(
                                Metric::builder()
                                    .namespace(&self.namespace)
                                    .metric_name("http.server.request.duration")
                                    .dimensions(
                                        Dimension::builder().name("endpoint").value(*ep).build(),
                                    )
                                    .build(),
                            )
                            .period(period_secs)
                            .stat("p95")
                            .build(),
                    )
                    .return_data(true)
                    .build(),
            );
        }

        // 3. Token usage by prompt
        for (p_idx, prompt_name) in DEFAULT_PROMPTS.iter().enumerate() {
            for token_type in ["input", "reasoning", "output"] {
                let id = format!("tok_{p_idx}_{token_type}");
                queries.push(
                    MetricDataQuery::builder()
                        .id(id)
                        .metric_stat(
                            MetricStat::builder()
                                .metric(
                                    Metric::builder()
                                        .namespace(&self.namespace)
                                        .metric_name("gen_ai.client.token.usage")
                                        .dimensions(
                                            Dimension::builder()
                                                .name("prompt")
                                                .value(*prompt_name)
                                                .build(),
                                        )
                                        .dimensions(
                                            Dimension::builder()
                                                .name("token_type")
                                                .value(token_type)
                                                .build(),
                                        )
                                        .build(),
                                )
                                .period(period_secs)
                                .stat("Sum")
                                .build(),
                        )
                        .return_data(true)
                        .build(),
                );
            }
        }

        // 4. Topics
        let topics = [
            "medical",
            "shopping",
            "social",
            "education",
            "emergency",
            "authorities",
            "financial",
            "mobility",
            "juridical",
            "other",
        ];
        for (idx, topic) in topics.iter().enumerate() {
            queries.push(
                MetricDataQuery::builder()
                    .id(format!("topic_{idx}"))
                    .metric_stat(
                        MetricStat::builder()
                            .metric(
                                Metric::builder()
                                    .namespace(&self.namespace)
                                    .metric_name("diwop.translate.classification.count")
                                    .dimensions(
                                        Dimension::builder().name("topic").value(*topic).build(),
                                    )
                                    .build(),
                            )
                            .period(period_secs)
                            .stat("Sum")
                            .build(),
                    )
                    .return_data(true)
                    .build(),
            );
        }

        // 5. Alert categories
        let categories = ["ok", "info", "warning", "danger"];
        for (idx, cat) in categories.iter().enumerate() {
            queries.push(
                MetricDataQuery::builder()
                    .id(format!("cat_{idx}"))
                    .metric_stat(
                        MetricStat::builder()
                            .metric(
                                Metric::builder()
                                    .namespace(&self.namespace)
                                    .metric_name("diwop.translate.classification.count")
                                    .dimensions(
                                        Dimension::builder()
                                            .name("classification_category")
                                            .value(*cat)
                                            .build(),
                                    )
                                    .build(),
                            )
                            .period(period_secs)
                            .stat("Sum")
                            .build(),
                    )
                    .return_data(true)
                    .build(),
            );
        }

        // 6. Classification types
        let class_types = ["translation", "formatting"];
        for (idx, ct) in class_types.iter().enumerate() {
            queries.push(
                MetricDataQuery::builder()
                    .id(format!("type_{idx}"))
                    .metric_stat(
                        MetricStat::builder()
                            .metric(
                                Metric::builder()
                                    .namespace(&self.namespace)
                                    .metric_name("diwop.translate.classification.count")
                                    .dimensions(
                                        Dimension::builder()
                                            .name("classification_type")
                                            .value(*ct)
                                            .build(),
                                    )
                                    .build(),
                            )
                            .period(period_secs)
                            .stat("Sum")
                            .build(),
                    )
                    .return_data(true)
                    .build(),
            );
        }

        // 7. History length distribution
        let history_buckets = ["0", "1-2", "3-5", "6-10", "11+"];
        for (idx, bucket) in history_buckets.iter().enumerate() {
            queries.push(
                MetricDataQuery::builder()
                    .id(format!("hist_{idx}"))
                    .metric_stat(
                        MetricStat::builder()
                            .metric(
                                Metric::builder()
                                    .namespace(&self.namespace)
                                    .metric_name("diwop.translate.history_length")
                                    .dimensions(
                                        Dimension::builder()
                                            .name("history_bucket")
                                            .value(*bucket)
                                            .build(),
                                    )
                                    .build(),
                            )
                            .period(period_secs)
                            .stat("SampleCount")
                            .build(),
                    )
                    .return_data(true)
                    .build(),
            );
        }

        // Execute GetMetricData request
        let client = self.get_client().await;
        let mut req = client
            .get_metric_data()
            .start_time(start_dt)
            .end_time(end_dt);

        for q in queries {
            req = req.metric_data_queries(q);
        }

        let resp = req.send().await.map_err(|e| {
            AnalyticsError::CloudWatchError(
                aws_sdk_cloudwatch::error::DisplayErrorContext(&e).to_string(),
            )
        })?;

        let mut report = AnalyticsReport::empty_at(range, now);

        for result in resp.metric_data_results() {
            let sum_val: f64 = result.values().iter().sum();
            let id = result.id().unwrap_or_default();

            if id == "total_req" {
                report.summary.total_requests = sum_val as u64;
            } else if let Some(stripped) = id.strip_prefix("req_") {
                let parts: Vec<&str> = stripped.split('_').collect();
                if parts.len() == 2 {
                    if let Ok(ep_idx) = parts[0].parse::<usize>() {
                        if let Some(ep) = report.requests_by_endpoint.get_mut(ep_idx) {
                            match parts[1] {
                                "2xx" => ep.status_2xx = sum_val as u64,
                                "4xx" => ep.status_4xx = sum_val as u64,
                                "5xx" => ep.status_5xx = sum_val as u64,
                                _ => {}
                            }
                        }
                        if let Some(ep_series) = report.requests_over_time.get_mut(ep_idx) {
                            for (ts, val) in result.timestamps().iter().zip(result.values().iter())
                            {
                                let ts_secs = ts.secs().max(0) as u64;
                                let bucket_idx =
                                    ((ts_secs.saturating_sub(start_secs) / bucket_secs) as usize)
                                        .min(num_buckets - 1);
                                if let Some(pt) = ep_series.series.get_mut(bucket_idx) {
                                    match parts[1] {
                                        "2xx" => pt.status_2xx += *val as u64,
                                        "4xx" => pt.status_4xx += *val as u64,
                                        "5xx" => pt.status_5xx += *val as u64,
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            } else if let Some(stripped) = id.strip_prefix("lat_avg_") {
                if let Ok(ep_idx) = stripped.parse::<usize>() {
                    if let Some(ep) = report.latencies_by_endpoint.get_mut(ep_idx) {
                        let count = result.values().len().max(1) as f64;
                        ep.avg_ms = ((sum_val / count) * 1000.0 * 10.0).round() / 10.0;
                    }
                }
            } else if let Some(stripped) = id.strip_prefix("lat_p95_") {
                if let Ok(ep_idx) = stripped.parse::<usize>() {
                    if let Some(ep) = report.latencies_by_endpoint.get_mut(ep_idx) {
                        let max_p95 = result.values().iter().copied().fold(0.0f64, f64::max);
                        ep.p95_ms = (max_p95 * 1000.0 * 10.0).round() / 10.0;
                    }
                }
            } else if let Some(stripped) = id.strip_prefix("tok_") {
                let parts: Vec<&str> = stripped.split('_').collect();
                if parts.len() == 2 {
                    if let Ok(p_idx) = parts[0].parse::<usize>() {
                        if let Some(pm) = report.token_usage_by_prompt.get_mut(p_idx) {
                            match parts[1] {
                                "input" => pm.input_tokens = sum_val as u64,
                                "reasoning" => pm.reasoning_tokens = sum_val as u64,
                                "output" => pm.response_tokens = sum_val as u64,
                                _ => {}
                            }
                        }
                    }
                }
            } else if let Some(stripped) = id.strip_prefix("topic_") {
                if let Ok(t_idx) = stripped.parse::<usize>() {
                    if let Some(t) = report.translation_by_topic.get_mut(t_idx) {
                        t.count = sum_val as u64;
                    }
                }
            } else if let Some(stripped) = id.strip_prefix("cat_") {
                if let Ok(c_idx) = stripped.parse::<usize>() {
                    if let Some(c) = report.translation_by_category.get_mut(c_idx) {
                        c.count = sum_val as u64;
                    }
                }
            } else if let Some(stripped) = id.strip_prefix("type_") {
                if let Ok(ct_idx) = stripped.parse::<usize>() {
                    if let Some(ct) = report.translation_by_type.get_mut(ct_idx) {
                        ct.count = sum_val as u64;
                    }
                }
            } else if let Some(stripped) = id.strip_prefix("hist_") {
                if let Ok(h_idx) = stripped.parse::<usize>() {
                    if let Some(h) = report.history_length_distribution.get_mut(h_idx) {
                        h.count = sum_val as u64;
                    }
                }
            }
        }

        // Compute summary totals from gathered endpoint metrics
        let total_4xx: u64 = report
            .requests_by_endpoint
            .iter()
            .map(|e| e.status_4xx)
            .sum();
        let total_5xx: u64 = report
            .requests_by_endpoint
            .iter()
            .map(|e| e.status_5xx)
            .sum();
        let total_err = total_4xx + total_5xx;
        let total_req = if report.summary.total_requests > 0 {
            report.summary.total_requests
        } else {
            report
                .requests_by_endpoint
                .iter()
                .map(|e| e.status_2xx + e.status_4xx + e.status_5xx)
                .sum()
        };
        report.summary.total_requests = total_req;
        report.summary.error_rate_percent = if total_req > 0 {
            ((total_err as f64 / total_req as f64) * 1000.0).round() / 10.0
        } else {
            0.0
        };

        let total_tokens: u64 = report
            .token_usage_by_prompt
            .iter()
            .map(|t| t.input_tokens + t.reasoning_tokens + t.response_tokens)
            .sum();
        report.summary.total_tokens = total_tokens;

        let avg_lat: f64 = report
            .latencies_by_endpoint
            .iter()
            .filter(|l| l.avg_ms > 0.0)
            .map(|l| l.avg_ms)
            .sum::<f64>()
            / report
                .latencies_by_endpoint
                .iter()
                .filter(|l| l.avg_ms > 0.0)
                .count()
                .max(1) as f64;
        report.summary.avg_latency_ms = (avg_lat * 10.0).round() / 10.0;

        Ok(report)
    }
}

// ---------------------------------------------------------------------------
// Provider Factory
// ---------------------------------------------------------------------------

/// Creates a [`MetricsQueryService`] instance based on environment variables.
///
/// Supported values for `METRICS_PROVIDER`:
/// - `"cloudwatch"`: Queries AWS CloudWatch via [`CloudWatchMetricsProvider`] using `CLOUDWATCH_METRICS_NAMESPACE` (default: `"diwop"`).
/// - `"stackit"`: Returns [`StackitMetricsProvider`] stub returning 0 / empty metrics.
/// - `"memory"` / unset: Returns [`InMemoryMetricsProvider`] querying the live in-memory store.
pub fn create_metrics_query_service() -> Arc<dyn MetricsQueryService> {
    let provider = std::env::var("METRICS_PROVIDER")
        .unwrap_or_else(|_| "memory".to_string())
        .to_ascii_lowercase();

    match provider.as_str() {
        "cloudwatch" => {
            let namespace = std::env::var("CLOUDWATCH_METRICS_NAMESPACE")
                .unwrap_or_else(|_| "diwop".to_string());
            tracing::info!(
                provider = "cloudwatch",
                namespace = %namespace,
                "Initialized CloudWatch metrics query service"
            );
            Arc::new(CloudWatchMetricsProvider::new(namespace))
        }
        "stackit" => {
            tracing::info!(
                provider = "stackit",
                "Initialized STACKIT stub metrics query service"
            );
            Arc::new(StackitMetricsProvider::new())
        }
        _ => {
            tracing::info!(
                provider = "memory",
                "Initialized in-memory metrics query service"
            );
            Arc::new(InMemoryMetricsProvider::global())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_range_parsing_and_duration() {
        assert_eq!(TimeRange::from_str_opt("1h"), Some(TimeRange::OneHour));
        assert_eq!(
            TimeRange::from_str_opt("24h"),
            Some(TimeRange::TwentyFourHours)
        );
        assert_eq!(TimeRange::from_str_opt("7d"), Some(TimeRange::SevenDays));
        assert_eq!(TimeRange::from_str_opt("30d"), Some(TimeRange::ThirtyDays));
        assert_eq!(TimeRange::from_str_opt("invalid"), None);

        assert_eq!(TimeRange::OneHour.duration(), Duration::from_secs(3600));
        assert_eq!(
            TimeRange::TwentyFourHours.duration(),
            Duration::from_secs(86400)
        );
        assert_eq!(
            TimeRange::SevenDays.duration(),
            Duration::from_secs(7 * 86400)
        );
        assert_eq!(
            TimeRange::ThirtyDays.duration(),
            Duration::from_secs(30 * 86400)
        );

        assert_eq!(TimeRange::OneHour.bucket_config(), (300, 12));
        assert_eq!(TimeRange::TwentyFourHours.bucket_config(), (3600, 24));
        assert_eq!(TimeRange::SevenDays.bucket_config(), (86400, 7));
        assert_eq!(TimeRange::ThirtyDays.bucket_config(), (86400, 30));
    }

    #[test]
    fn test_in_memory_metrics_store_empty() {
        let store = InMemoryMetricsStore::new();
        let report = store.generate_report(TimeRange::TwentyFourHours);

        assert_eq!(report.summary.total_requests, 0);
        assert_eq!(report.summary.error_rate_percent, 0.0);
        assert_eq!(report.summary.avg_latency_ms, 0.0);
        assert_eq!(report.summary.total_tokens, 0);
        assert_eq!(report.requests_by_endpoint.len(), 2);
        assert_eq!(report.requests_over_time.len(), 2);
        assert_eq!(report.requests_over_time[0].series.len(), 24);
        assert_eq!(report.requests_over_time[1].series.len(), 24);
        assert_eq!(report.latencies_by_endpoint.len(), 2);
        assert_eq!(report.token_usage_by_prompt.len(), 3);
        assert_eq!(report.token_usage_by_prompt[0].prompt, "correct-transcript");
        assert_eq!(report.token_usage_by_prompt[1].prompt, "translate");
        assert_eq!(report.token_usage_by_prompt[2].prompt, "classify");
        assert_eq!(report.translation_by_topic.len(), 10);
        assert_eq!(report.translation_by_category.len(), 4);
        assert_eq!(report.translation_by_type.len(), 2);
        assert_eq!(report.history_length_distribution.len(), 5);
    }

    #[test]
    fn test_in_memory_metrics_store_recording_and_aggregation() {
        let store = InMemoryMetricsStore::new();

        // Record HTTP requests
        store.record_http_request("/translate", 200, 0.250);
        store.record_http_request("/translate", 500, 0.750);
        store.record_http_request("/correct-transcript", 200, 0.100);
        store.record_http_request("/correct-transcript", 400, 0.050);

        // Record tokens by prompt
        store.record_token_usage("translate", 100, 50, 40);
        store.record_token_usage("classify", 60, 10, 15);
        store.record_token_usage("correct-transcript", 30, 0, 20);

        // Record history
        store.record_history_length(0);
        store.record_history_length(2);
        store.record_history_length(4);
        store.record_history_length(8);
        store.record_history_length(15);

        // Record classifications
        store.record_classification("medical", "translation", "danger");
        store.record_classification("authorities", "formatting", "info");
        store.record_classification("medical", "translation", "ok");

        let report = store.generate_report(TimeRange::OneHour);

        assert_eq!(report.summary.total_requests, 4);
        // Errors: 1 (500) + 1 (400) = 2 out of 4 = 50.0%
        assert_eq!(report.summary.error_rate_percent, 50.0);
        // Total tokens: (100 + 50 + 40) + (60 + 10 + 15) + (30 + 0 + 20) = 325
        assert_eq!(report.summary.total_tokens, 325);
        // Avg latency: (250 + 750 + 100 + 50) / 4 = 287.5 ms
        assert_eq!(report.summary.avg_latency_ms, 287.5);

        // Endpoints
        let trans_ep = report
            .requests_by_endpoint
            .iter()
            .find(|e| e.endpoint == "/translate")
            .unwrap();
        assert_eq!(trans_ep.status_2xx, 1);
        assert_eq!(trans_ep.status_4xx, 0);
        assert_eq!(trans_ep.status_5xx, 1);

        let correct_ep = report
            .requests_by_endpoint
            .iter()
            .find(|e| e.endpoint == "/correct-transcript")
            .unwrap();
        assert_eq!(correct_ep.status_2xx, 1);
        assert_eq!(correct_ep.status_4xx, 1);
        assert_eq!(correct_ep.status_5xx, 0);

        // Time-series curves (1h -> 12 buckets; recent events land in last bucket)
        let trans_series = report
            .requests_over_time
            .iter()
            .find(|s| s.endpoint == "/translate")
            .unwrap();
        assert_eq!(trans_series.series.len(), 12);
        let trans_2xx_sum: u64 = trans_series.series.iter().map(|p| p.status_2xx).sum();
        let trans_5xx_sum: u64 = trans_series.series.iter().map(|p| p.status_5xx).sum();
        assert_eq!(trans_2xx_sum, 1);
        assert_eq!(trans_5xx_sum, 1);

        // Tokens by prompt
        let correct_tok = report
            .token_usage_by_prompt
            .iter()
            .find(|t| t.prompt == "correct-transcript")
            .unwrap();
        assert_eq!(correct_tok.input_tokens, 30);
        assert_eq!(correct_tok.reasoning_tokens, 0);
        assert_eq!(correct_tok.response_tokens, 20);

        let trans_tok = report
            .token_usage_by_prompt
            .iter()
            .find(|t| t.prompt == "translate")
            .unwrap();
        assert_eq!(trans_tok.input_tokens, 100);
        assert_eq!(trans_tok.reasoning_tokens, 50);
        assert_eq!(trans_tok.response_tokens, 40);

        let classify_tok = report
            .token_usage_by_prompt
            .iter()
            .find(|t| t.prompt == "classify")
            .unwrap();
        assert_eq!(classify_tok.input_tokens, 60);
        assert_eq!(classify_tok.reasoning_tokens, 10);
        assert_eq!(classify_tok.response_tokens, 15);

        // Topics
        let med_topic = report
            .translation_by_topic
            .iter()
            .find(|t| t.topic == "medical")
            .unwrap();
        assert_eq!(med_topic.count, 2);

        let auth_topic = report
            .translation_by_topic
            .iter()
            .find(|t| t.topic == "authorities")
            .unwrap();
        assert_eq!(auth_topic.count, 1);

        // Categories
        let danger_cat = report
            .translation_by_category
            .iter()
            .find(|c| c.category == "danger")
            .unwrap();
        assert_eq!(danger_cat.count, 1);

        let info_cat = report
            .translation_by_category
            .iter()
            .find(|c| c.category == "info")
            .unwrap();
        assert_eq!(info_cat.count, 1);

        let ok_cat = report
            .translation_by_category
            .iter()
            .find(|c| c.category == "ok")
            .unwrap();
        assert_eq!(ok_cat.count, 1);

        // Types
        let trans_type = report
            .translation_by_type
            .iter()
            .find(|t| t.classification_type == "translation")
            .unwrap();
        assert_eq!(trans_type.count, 2);

        let format_type = report
            .translation_by_type
            .iter()
            .find(|t| t.classification_type == "formatting")
            .unwrap();
        assert_eq!(format_type.count, 1);

        // History buckets: 0, 1-2, 3-5, 6-10, 11+ (each recorded once)
        for bucket in &report.history_length_distribution {
            assert_eq!(
                bucket.count, 1,
                "Bucket {} should have 1 count",
                bucket.bucket
            );
        }
    }

    #[tokio::test]
    async fn test_stackit_provider_stub() {
        let provider = StackitMetricsProvider::new();
        let report = provider
            .query_analytics(TimeRange::TwentyFourHours)
            .await
            .unwrap();
        assert_eq!(report.summary.total_requests, 0);
        assert_eq!(report.summary.total_tokens, 0);
        assert_eq!(report.token_usage_by_prompt.len(), 3);
        assert_eq!(report.requests_over_time.len(), 2);
    }

    #[tokio::test]
    async fn test_in_memory_provider_query() {
        let store = Arc::new(InMemoryMetricsStore::new());
        store.record_http_request("/translate", 200, 0.123);
        let provider = InMemoryMetricsProvider::new(store);
        let report = provider.query_analytics(TimeRange::OneHour).await.unwrap();
        assert_eq!(report.summary.total_requests, 1);
        assert_eq!(
            report
                .requests_by_endpoint
                .iter()
                .find(|e| e.endpoint == "/translate")
                .unwrap()
                .status_2xx,
            1
        );
    }

    #[test]
    fn test_create_metrics_query_service_provider_selection() {
        // Default / unset -> memory
        std::env::remove_var("METRICS_PROVIDER");
        let service = create_metrics_query_service();
        assert!(!format!("{service:p}").is_empty());

        // stackit -> stackit stub
        std::env::set_var("METRICS_PROVIDER", "stackit");
        let service = create_metrics_query_service();
        assert!(!format!("{service:p}").is_empty());

        // cloudwatch -> cloudwatch provider
        std::env::set_var("METRICS_PROVIDER", "cloudwatch");
        std::env::set_var("CLOUDWATCH_METRICS_NAMESPACE", "custom-ns");
        let service = create_metrics_query_service();
        assert!(!format!("{service:p}").is_empty());

        std::env::remove_var("METRICS_PROVIDER");
        std::env::remove_var("CLOUDWATCH_METRICS_NAMESPACE");
    }
}
