use prometheus::{
    CounterVec, GaugeVec, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec,
    IntGauge, Opts, Registry,
};
use std::time::{Duration, Instant};

pub struct PrometheusMetrics {
    pub registry: Registry,

    pub auth_attempts_total: IntCounter,
    pub auth_success_total: IntCounter,
    pub auth_failures_total: IntCounterVec,
    pub auth_failures_locked: IntCounter,

    pub connections_accepted: IntCounter,
    pub connections_closed: IntCounter,
    pub connections_active: IntGauge,

    pub messages_sent: IntCounter,
    pub messages_received: IntCounter,
    pub bytes_sent: IntCounter,
    pub bytes_received: IntCounter,
    pub errors: IntCounter,

    pub messages_sent_total: IntCounterVec,
    pub messages_failed_total: IntCounter,

    pub users_online: IntGauge,

    pub db_query_duration_seconds: HistogramVec,
    pub db_query_errors_total: IntCounterVec,
    pub db_connections_active: IntGauge,
    pub db_connections_idle: IntGauge,

    pub cache_hits_total: IntCounter,
    pub cache_misses_total: IntCounter,

    pub http_requests_total: CounterVec,
    pub http_request_duration_seconds: HistogramVec,

    pub channel_members: GaugeVec,
    pub channels_total: IntGauge,

    pub latency_auth_seconds: Histogram,
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
        metrics.set_users_online(10);
        metrics.record_cache_hit();
        metrics.record_cache_miss();

        assert_eq!(metrics.auth_attempts_total.get(), 1);
        assert_eq!(metrics.auth_success_total.get(), 1);
        assert_eq!(metrics.connections_accepted.get(), 1);
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
}
