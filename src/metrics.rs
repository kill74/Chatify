//! Prometheus metrics collection and exposition.
//!
//! Collects server telemetry including authentication rates, connection stats,
//! message throughput, database performance, and latency percentiles.
//! Metrics can be exported to Prometheus for alerting and dashboarding.
//!
//! # Key Metrics
//!
//! - **Authentication**: attempts, successes, failures by reason, lockouts
//! - **Connections**: total accepted, closed, currently active
//! - **Messages**: sent/received counts, bytes transferred, error rates
//! - **Database**: query latency histograms, error counts, connection pool stats
//! - **Cache**: hits and misses for in-memory buffers
//! - **Latency**: p50/p95/p99 of auth and message processing time

use prometheus::{
    CounterVec, GaugeVec, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec,
    IntGauge, Opts, Registry,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Summary of database operation performance.
#[derive(Clone, Debug, serde::Serialize)]
pub struct DbOperationSummary {
    /// Operation name (e.g., "store_event", "history").
    pub operation: String,
    /// Number of samples collected in the window.
    pub samples: u64,
    /// Number of errors encountered.
    pub errors: u64,
    /// Error rate as a fraction (0.0 to 1.0).
    pub error_rate: f64,
    /// 50th percentile latency in milliseconds.
    pub p50_ms: f64,
    /// 95th percentile latency in milliseconds.
    pub p95_ms: f64,
    /// 99th percentile latency in milliseconds.
    pub p99_ms: f64,
    /// Average latency in milliseconds.
    pub avg_ms: f64,
}

/// Alert triggered when database latency exceeds thresholds.
#[derive(Clone, Debug, serde::Serialize)]
pub struct DbLatencyAlert {
    /// Operation name.
    pub operation: String,
    /// Severity level (e.g., "warning", "critical").
    pub severity: String,
    /// Observed p95 latency.
    pub p95_ms: f64,
    /// Alert threshold that was exceeded.
    pub threshold_ms: f64,
    /// Sample count.
    pub samples: u64,
}

/// Complete Prometheus metrics registry for Chatify server.
///
/// Exposes metrics in Prometheus text format for scraping. All counters and gauges
/// are registered automatically on instantiation.
pub struct PrometheusMetrics {
    /// The underlying Prometheus registry.
    pub registry: Registry,

    /// Total authentication attempts.
    pub auth_attempts_total: IntCounter,
    /// Successful authentications.
    pub auth_success_total: IntCounter,
    /// Failed authentications by reason.
    pub auth_failures_total: IntCounterVec,
    /// Authentications rejected due to rate limiting or lockout.
    pub auth_failures_locked: IntCounter,

    /// Total connections accepted.
    pub connections_accepted: IntCounter,
    /// Total connections closed.
    pub connections_closed: IntCounter,
    /// Currently active connections (gauge).
    pub connections_active: IntGauge,

    /// Total messages sent to clients.
    pub messages_sent: IntCounter,
    /// Total messages received from clients.
    pub messages_received: IntCounter,
    /// Total bytes sent (including WebSocket overhead).
    pub bytes_sent: IntCounter,
    /// Total bytes received.
    pub bytes_received: IntCounter,
    /// Total protocol errors.
    pub errors: IntCounter,

    /// Messages sent by type (histogram).
    pub messages_sent_total: IntCounterVec,
    /// Failed message sends.
    pub messages_failed_total: IntCounter,
    /// Messages dropped due to full outbound queue.
    pub outbound_queue_drops_total: IntCounter,
    /// Disconnections caused by slow client writes.
    pub slow_client_disconnects_total: IntCounter,

    /// Currently online users (gauge).
    pub users_online: IntGauge,

    /// Database query latency distribution (histogram by operation).
    pub db_query_duration_seconds: HistogramVec,
    /// Database query errors by operation.
    pub db_query_errors_total: IntCounterVec,
    /// Currently active database connections (gauge).
    pub db_connections_active: IntGauge,
    /// Idle database connections (gauge).
    pub db_connections_idle: IntGauge,

    /// Cache hits (e.g., repeated key queries).
    pub cache_hits_total: IntCounter,
    /// Cache misses.
    pub cache_misses_total: IntCounter,

    /// HTTP requests by method and status (if applicable).
    pub http_requests_total: CounterVec,
    /// HTTP request latency distribution.
    pub http_request_duration_seconds: HistogramVec,

    /// Current member count per channel.
    pub channel_members: GaugeVec,
    /// Total number of channels.
    pub channels_total: IntGauge,

    /// Auth latency (p50, p95, p99).
    pub latency_auth_seconds: Histogram,
    /// Message processing latency (p50, p95, p99).
    pub latency_message_seconds: Histogram,
}

impl PrometheusMetrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let auth_attempts_total = IntCounter::with_opts(Opts::new(
            "chatify_auth_attempts_total",
            "Total number of authentication attempts",
        ))?;
        registry.register(Box::new(auth_attempts_total.clone()))?;

        let auth_success_total = IntCounter::with_opts(Opts::new(
            "chatify_auth_success_total",
            "Total number of successful authentications",
        ))?;
        registry.register(Box::new(auth_success_total.clone()))?;

        let auth_failures_total = IntCounterVec::new(
            Opts::new(
                "chatify_auth_failures_total",
                "Total number of authentication failures by reason",
            ),
            &["reason"],
        )?;
        registry.register(Box::new(auth_failures_total.clone()))?;

        let auth_failures_locked = IntCounter::with_opts(Opts::new(
            "chatify_auth_failures_locked_total",
            "Total number of auth attempts blocked due to account lockout",
        ))?;
        registry.register(Box::new(auth_failures_locked.clone()))?;

        let connections_accepted = IntCounter::with_opts(Opts::new(
            "chatify_connections_accepted_total",
            "Total number of WebSocket connections accepted",
        ))?;
        registry.register(Box::new(connections_accepted.clone()))?;

        let connections_closed = IntCounter::with_opts(Opts::new(
            "chatify_connections_closed_total",
            "Total number of WebSocket connections closed",
        ))?;
        registry.register(Box::new(connections_closed.clone()))?;

        let connections_active = IntGauge::with_opts(Opts::new(
            "chatify_connections_active",
            "Current number of active WebSocket connections",
        ))?;
        registry.register(Box::new(connections_active.clone()))?;

        let messages_sent_total = IntCounterVec::new(
            Opts::new(
                "chatify_messages_sent_total",
                "Total number of messages sent by channel",
            ),
            &["channel"],
        )?;
        registry.register(Box::new(messages_sent_total.clone()))?;

        let messages_sent = IntCounter::with_opts(Opts::new(
            "chatify_messages_sent",
            "Total number of messages sent",
        ))?;
        registry.register(Box::new(messages_sent.clone()))?;

        let messages_received = IntCounter::with_opts(Opts::new(
            "chatify_messages_received",
            "Total number of messages received",
        ))?;
        registry.register(Box::new(messages_received.clone()))?;

        let bytes_sent =
            IntCounter::with_opts(Opts::new("chatify_bytes_sent", "Total bytes sent"))?;
        registry.register(Box::new(bytes_sent.clone()))?;

        let bytes_received =
            IntCounter::with_opts(Opts::new("chatify_bytes_received", "Total bytes received"))?;
        registry.register(Box::new(bytes_received.clone()))?;

        let errors =
            IntCounter::with_opts(Opts::new("chatify_errors_total", "Total number of errors"))?;
        registry.register(Box::new(errors.clone()))?;

        let messages_failed_total = IntCounter::with_opts(Opts::new(
            "chatify_messages_failed_total",
            "Total number of failed message sends",
        ))?;
        registry.register(Box::new(messages_failed_total.clone()))?;

        let outbound_queue_drops_total = IntCounter::with_opts(Opts::new(
            "chatify_outbound_queue_drops_total",
            "Total number of dropped outbound messages due to full per-connection queues",
        ))?;
        registry.register(Box::new(outbound_queue_drops_total.clone()))?;

        let slow_client_disconnects_total = IntCounter::with_opts(Opts::new(
            "chatify_slow_client_disconnects_total",
            "Total number of client disconnects triggered by outbound queue backpressure",
        ))?;
        registry.register(Box::new(slow_client_disconnects_total.clone()))?;

        let users_online = IntGauge::with_opts(Opts::new(
            "chatify_users_online",
            "Current number of authenticated online users",
        ))?;
        registry.register(Box::new(users_online.clone()))?;

        let db_query_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "chatify_db_query_duration_seconds",
                "Database query duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]),
            &["operation"],
        )?;
        registry.register(Box::new(db_query_duration_seconds.clone()))?;

        let db_query_errors_total = IntCounterVec::new(
            Opts::new(
                "chatify_db_query_errors_total",
                "Total number of database query errors",
            ),
            &["operation"],
        )?;
        registry.register(Box::new(db_query_errors_total.clone()))?;

        let db_connections_active = IntGauge::with_opts(Opts::new(
            "chatify_db_connections_active",
            "Number of active database connections",
        ))?;
        registry.register(Box::new(db_connections_active.clone()))?;

        let db_connections_idle = IntGauge::with_opts(Opts::new(
            "chatify_db_connections_idle",
            "Number of idle database connections",
        ))?;
        registry.register(Box::new(db_connections_idle.clone()))?;

        let cache_hits_total = IntCounter::with_opts(Opts::new(
            "chatify_cache_hits_total",
            "Total number of cache hits",
        ))?;
        registry.register(Box::new(cache_hits_total.clone()))?;

        let cache_misses_total = IntCounter::with_opts(Opts::new(
            "chatify_cache_misses_total",
            "Total number of cache misses",
        ))?;
        registry.register(Box::new(cache_misses_total.clone()))?;

        let http_requests_total = CounterVec::new(
            Opts::new(
                "chatify_http_requests_total",
                "Total HTTP requests by endpoint and status",
            ),
            &["endpoint", "method", "status"],
        )?;
        registry.register(Box::new(http_requests_total.clone()))?;

        let http_request_duration_seconds = HistogramVec::new(
            HistogramOpts::new(
                "chatify_http_request_duration_seconds",
                "HTTP request duration in seconds",
            )
            .buckets(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5,
            ]),
            &["endpoint"],
        )?;
        registry.register(Box::new(http_request_duration_seconds.clone()))?;

        let channel_members = GaugeVec::new(
            Opts::new(
                "chatify_channel_members",
                "Number of members in each channel",
            ),
            &["channel"],
        )?;
        registry.register(Box::new(channel_members.clone()))?;

        let channels_total = IntGauge::with_opts(Opts::new(
            "chatify_channels_total",
            "Total number of active channels",
        ))?;
        registry.register(Box::new(channels_total.clone()))?;

        let latency_auth_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "chatify_auth_latency_seconds",
                "Authentication processing latency in seconds",
            )
            .buckets(vec![0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0]),
        )?;
        registry.register(Box::new(latency_auth_seconds.clone()))?;

        let latency_message_seconds = Histogram::with_opts(
            HistogramOpts::new(
                "chatify_message_latency_seconds",
                "Message processing latency in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25]),
        )?;
        registry.register(Box::new(latency_message_seconds.clone()))?;

        Ok(Self {
            registry,
            auth_attempts_total,
            auth_success_total,
            auth_failures_total,
            auth_failures_locked,
            connections_accepted,
            connections_closed,
            connections_active,
            messages_sent,
            messages_received,
            bytes_sent,
            bytes_received,
            errors,
            messages_sent_total,
            messages_failed_total,
            outbound_queue_drops_total,
            slow_client_disconnects_total,
            users_online,
            db_query_duration_seconds,
            db_query_errors_total,
            db_connections_active,
            db_connections_idle,
            cache_hits_total,
            cache_misses_total,
            http_requests_total,
            http_request_duration_seconds,
            channel_members,
            channels_total,
            latency_auth_seconds,
            latency_message_seconds,
        })
    }

    // Compatibility methods for old Metrics API
    pub fn inc_sent(&self, count: usize) {
        self.messages_sent.inc_by(count as u64);
    }

    pub fn inc_received(&self, count: usize) {
        self.messages_received.inc_by(count as u64);
    }

    pub fn inc_bytes_sent(&self, bytes: usize) {
        self.bytes_sent.inc_by(bytes as u64);
    }

    pub fn inc_bytes_received(&self, bytes: usize) {
        self.bytes_received.inc_by(bytes as u64);
    }

    pub fn inc_errors(&self) {
        self.errors.inc();
    }

    pub fn inc_accepted(&self) {
        self.record_connection_accepted();
    }

    pub fn inc_closed(&self) {
        self.record_connection_closed();
    }

    pub fn record_auth_attempt(&self) {
        self.auth_attempts_total.inc();
    }

    pub fn record_auth_success(&self) {
        self.auth_success_total.inc();
    }

    pub fn record_auth_failure(&self, reason: &str) {
        self.auth_failures_total.with_label_values(&[reason]).inc();
    }

    pub fn record_auth_locked(&self) {
        self.auth_failures_locked.inc();
    }

    pub fn record_connection_accepted(&self) {
        self.connections_accepted.inc();
        self.connections_active.inc();
    }

    pub fn record_connection_closed(&self) {
        self.connections_closed.inc();
        self.connections_active.dec();
    }

    pub fn record_message_sent(&self, channel: &str) {
        self.messages_sent_total.with_label_values(&[channel]).inc();
    }

    pub fn record_message_failed(&self) {
        self.messages_failed_total.inc();
    }

    pub fn record_outbound_queue_drop(&self) {
        self.outbound_queue_drops_total.inc();
    }

    pub fn record_slow_client_disconnect(&self) {
        self.slow_client_disconnects_total.inc();
    }

    pub fn set_users_online(&self, count: usize) {
        self.users_online.set(count as i64);
    }

    pub fn record_db_query(&self, operation: &str, duration: Duration) {
        self.db_query_duration_seconds
            .with_label_values(&[operation])
            .observe(duration.as_secs_f64());
    }

    pub fn record_db_error(&self, operation: &str) {
        self.db_query_errors_total
            .with_label_values(&[operation])
            .inc();
    }

    pub fn update_db_pool_stats(&self, active: usize, idle: usize) {
        self.db_connections_active.set(active as i64);
        self.db_connections_idle.set(idle as i64);
    }

    pub fn record_cache_hit(&self) {
        self.cache_hits_total.inc();
    }

    pub fn record_cache_miss(&self) {
        self.cache_misses_total.inc();
    }

    pub fn record_http_request(&self, endpoint: &str, method: &str, status: u16) {
        self.http_requests_total
            .with_label_values(&[endpoint, method, &status.to_string()])
            .inc();
    }

    pub fn record_http_duration(&self, endpoint: &str, duration: Duration) {
        self.http_request_duration_seconds
            .with_label_values(&[endpoint])
            .observe(duration.as_secs_f64());
    }

    pub fn set_channel_members(&self, channel: &str, count: usize) {
        self.channel_members
            .with_label_values(&[channel])
            .set(count as f64);
    }

    pub fn set_channels_total(&self, count: usize) {
        self.channels_total.set(count as i64);
    }

    pub fn record_auth_latency(&self, duration: Duration) {
        self.latency_auth_seconds.observe(duration.as_secs_f64());
    }

    pub fn record_message_latency(&self, duration: Duration) {
        self.latency_message_seconds.observe(duration.as_secs_f64());
    }

    pub fn cache_hit_ratio(&self) -> f64 {
        let hits = self.cache_hits_total.get();
        let misses = self.cache_misses_total.get();
        let total = hits + misses;
        if total == 0 {
            return 0.0;
        }
        hits as f64 / total as f64
    }

    pub fn top_db_operations_by_p95(&self, top_n: usize) -> Vec<DbOperationSummary> {
        if top_n == 0 {
            return Vec::new();
        }

        let mut duration_map: HashMap<String, (u64, f64, f64, f64, f64)> = HashMap::new();
        let mut error_map: HashMap<String, u64> = HashMap::new();

        for family in self.registry.gather() {
            match family.name() {
                "chatify_db_query_duration_seconds" => {
                    for metric in &family.metric {
                        let operation = metric
                            .label
                            .iter()
                            .find(|label| label.name() == "operation")
                            .map(|label| label.value().to_string())
                            .unwrap_or_else(|| "unknown".to_string());

                        let Some(histogram) = metric.histogram.as_ref() else {
                            continue;
                        };

                        let samples = histogram.sample_count();
                        if samples == 0 {
                            continue;
                        }

                        let sum_seconds = histogram.sample_sum();
                        let p50_ms = estimate_histogram_percentile_ms(histogram, 0.50)
                            .unwrap_or_else(|| (sum_seconds / samples as f64) * 1000.0);
                        let p95_ms = estimate_histogram_percentile_ms(histogram, 0.95)
                            .unwrap_or_else(|| (sum_seconds / samples as f64) * 1000.0);
                        let p99_ms = estimate_histogram_percentile_ms(histogram, 0.99)
                            .unwrap_or_else(|| (sum_seconds / samples as f64) * 1000.0);

                        duration_map
                            .insert(operation, (samples, sum_seconds, p50_ms, p95_ms, p99_ms));
                    }
                }
                "chatify_db_query_errors_total" => {
                    for metric in &family.metric {
                        let operation = metric
                            .label
                            .iter()
                            .find(|label| label.name() == "operation")
                            .map(|label| label.value().to_string())
                            .unwrap_or_else(|| "unknown".to_string());

                        let errors = metric
                            .counter
                            .as_ref()
                            .map(|counter| counter.value() as u64)
                            .unwrap_or_default();
                        error_map.insert(operation, errors);
                    }
                }
                _ => {}
            }
        }

        let mut summaries: Vec<DbOperationSummary> = duration_map
            .into_iter()
            .map(
                |(operation, (samples, sum_seconds, p50_ms, p95_ms, p99_ms))| {
                    let errors = *error_map.get(&operation).unwrap_or(&0);
                    let error_rate = if samples == 0 {
                        0.0
                    } else {
                        errors as f64 / samples as f64
                    };
                    DbOperationSummary {
                        operation,
                        samples,
                        errors,
                        error_rate,
                        p50_ms,
                        p95_ms,
                        p99_ms,
                        avg_ms: (sum_seconds / samples as f64) * 1000.0,
                    }
                },
            )
            .collect();

        summaries.sort_by(|a, b| {
            b.p95_ms
                .partial_cmp(&a.p95_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.samples.cmp(&a.samples))
                .then_with(|| a.operation.cmp(&b.operation))
        });

        summaries.truncate(top_n);
        summaries
    }

    pub fn db_latency_alerts(
        &self,
        top_n: usize,
        warning_p95_ms: f64,
        critical_p95_ms: f64,
        min_samples: u64,
    ) -> Vec<DbLatencyAlert> {
        if top_n == 0 {
            return Vec::new();
        }

        let mut alerts: Vec<DbLatencyAlert> = self
            .top_db_operations_by_p95(top_n.saturating_mul(4).max(top_n))
            .into_iter()
            .filter_map(|summary| {
                if summary.samples < min_samples {
                    return None;
                }

                if summary.p95_ms >= critical_p95_ms {
                    return Some(DbLatencyAlert {
                        operation: summary.operation,
                        severity: "critical".to_string(),
                        p95_ms: summary.p95_ms,
                        threshold_ms: critical_p95_ms,
                        samples: summary.samples,
                    });
                }

                if summary.p95_ms >= warning_p95_ms {
                    return Some(DbLatencyAlert {
                        operation: summary.operation,
                        severity: "warning".to_string(),
                        p95_ms: summary.p95_ms,
                        threshold_ms: warning_p95_ms,
                        samples: summary.samples,
                    });
                }

                None
            })
            .collect();

        alerts.sort_by(|a, b| {
            severity_rank(&b.severity)
                .cmp(&severity_rank(&a.severity))
                .then_with(|| {
                    b.p95_ms
                        .partial_cmp(&a.p95_ms)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .then_with(|| b.samples.cmp(&a.samples))
                .then_with(|| a.operation.cmp(&b.operation))
        });

        alerts.truncate(top_n);
        alerts
    }
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "critical" => 2,
        "warning" => 1,
        _ => 0,
    }
}

fn estimate_histogram_percentile_ms(
    histogram: &prometheus::proto::Histogram,
    percentile: f64,
) -> Option<f64> {
    let samples = histogram.sample_count();
    if samples == 0 {
        return None;
    }

    let target = ((samples as f64) * percentile).ceil() as u64;
    let average_ms = (histogram.sample_sum() / samples as f64) * 1000.0;

    for bucket in &histogram.bucket {
        if bucket.cumulative_count() >= target {
            let upper = bucket.upper_bound();
            if upper.is_finite() {
                return Some(upper * 1000.0);
            }
            return Some(average_ms);
        }
    }

    Some(average_ms)
}

pub struct Timer {
    start: Instant,
    callback: Box<dyn Fn(Duration) + Send + Sync>,
}

impl Timer {
    pub fn new<F>(callback: F) -> Self
    where
        F: Fn(Duration) + Send + Sync + 'static,
    {
        Self {
            start: Instant::now(),
            callback: Box::new(callback),
        }
    }

    pub fn finish(self) {
        let duration = self.start.elapsed();
        (self.callback)(duration);
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        let duration = self.start.elapsed();
        (self.callback)(duration);
    }
}

impl Default for PrometheusMetrics {
    fn default() -> Self {
        Self::new().expect("failed to initialize metrics")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = PrometheusMetrics::new();
        assert!(metrics.is_ok());
    }

    #[test]
    fn test_metrics_recording() {
        let metrics = PrometheusMetrics::new().unwrap();

        metrics.record_auth_attempt();
        metrics.record_auth_success();
        metrics.record_auth_failure("invalid_password");
        metrics.record_connection_accepted();
        metrics.record_connection_closed();
        metrics.record_message_sent("general");
        metrics.record_outbound_queue_drop();
        metrics.record_slow_client_disconnect();
        metrics.set_users_online(10);
        metrics.record_cache_hit();
        metrics.record_cache_miss();

        assert_eq!(metrics.auth_attempts_total.get(), 1);
        assert_eq!(metrics.auth_success_total.get(), 1);
        assert_eq!(metrics.connections_accepted.get(), 1);
        assert_eq!(metrics.outbound_queue_drops_total.get(), 1);
        assert_eq!(metrics.slow_client_disconnects_total.get(), 1);
        assert_eq!(metrics.cache_hits_total.get(), 1);
    }

    #[test]
    fn test_cache_hit_ratio() {
        let metrics = PrometheusMetrics::new().unwrap();

        assert_eq!(metrics.cache_hit_ratio(), 0.0);

        metrics.record_cache_hit();
        metrics.record_cache_hit();
        metrics.record_cache_miss();

        assert_eq!(metrics.cache_hit_ratio(), 2.0 / 3.0);
    }

    #[test]
    fn test_compatibility_methods() {
        let metrics = PrometheusMetrics::new().unwrap();

        metrics.inc_sent(5);
        metrics.inc_received(3);
        metrics.inc_bytes_sent(1024);
        metrics.inc_bytes_received(2048);
        metrics.inc_errors();
        metrics.inc_accepted();
        metrics.inc_closed();

        assert_eq!(metrics.messages_sent.get(), 5);
        assert_eq!(metrics.messages_received.get(), 3);
        assert_eq!(metrics.bytes_sent.get(), 1024);
        assert_eq!(metrics.bytes_received.get(), 2048);
        assert_eq!(metrics.errors.get(), 1);
    }

    #[test]
    fn test_top_db_operations_profile_includes_percentiles() {
        let metrics = PrometheusMetrics::new().unwrap();

        metrics.record_db_query("history", Duration::from_millis(8));
        metrics.record_db_query("history", Duration::from_millis(12));
        metrics.record_db_query("history", Duration::from_millis(20));
        metrics.record_db_query("search_encrypted_dm", Duration::from_millis(120));
        metrics.record_db_query("search_encrypted_dm", Duration::from_millis(180));
        metrics.record_db_error("search_encrypted_dm");

        let top = metrics.top_db_operations_by_p95(10);
        assert!(!top.is_empty());

        let history = top
            .iter()
            .find(|summary| summary.operation == "history")
            .expect("history operation should be present");
        assert!(history.samples >= 3);
        assert!(history.p50_ms > 0.0);
        assert!(history.p95_ms > 0.0);
        assert!(history.p99_ms > 0.0);
        assert!(history.avg_ms > 0.0);
    }

    #[test]
    fn test_db_latency_alerts_respect_thresholds() {
        let metrics = PrometheusMetrics::new().unwrap();

        for _ in 0..8 {
            metrics.record_db_query("persist_insert", Duration::from_millis(220));
        }
        for _ in 0..8 {
            metrics.record_db_query("history", Duration::from_millis(60));
        }

        let alerts = metrics.db_latency_alerts(10, 50.0, 200.0, 5);
        assert!(!alerts.is_empty());

        let has_critical = alerts
            .iter()
            .any(|alert| alert.operation == "persist_insert" && alert.severity == "critical");
        let has_warning = alerts
            .iter()
            .any(|alert| alert.operation == "history" && alert.severity == "warning");

        assert!(has_critical);
        assert!(has_warning);
    }
}
