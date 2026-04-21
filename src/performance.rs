//! Performance optimization utilities for high-throughput server.
//!
//! Includes caching layers for frequently-used objects (errors, JSON responses)
//! and metrics collection to avoid allocation overhead in the hot path.
//!
//! # Design Notes
//!
//! - **Static JSON cache**: Pre-allocated Arc-wrapped strings for common error responses
//!   reduce per-connection allocation and serialization time
//! - **LRU cache**: Bounded-size caches for derived keys (channel encryption keys, etc.)
//!   prevent unbounded memory growth under adversarial loads
//! - **VecCache**: Ring buffer for message history allows O(1) append/evict without heap thrashing
//!
//! These optimizations are critical for the server to handle thousands of concurrent
//! connections without latency spikes.

// Performance optimization utilities
// Designed for high-throughput, low-latency server applications

use lru::LruCache;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Pre-allocated static JSON strings for frequently sent messages
/// to avoid repeated allocation/serialization overhead
pub struct StaticJsonCache {
    cache: RwLock<HashMap<&'static str, Arc<String>>>,
}

impl StaticJsonCache {
    pub fn new() -> Self {
        let mut cache = HashMap::new();

        // Auth errors
        cache.insert(
            "err_invalid_auth",
            Arc::new(r#"{"t":"err","m":"invalid auth JSON"}"#.into()),
        );
        cache.insert(
            "err_auth_too_large",
            Arc::new(r#"{"t":"err","m":"auth frame too large"}"#.into()),
        );
        cache.insert(
            "err_first_text",
            Arc::new(r#"{"t":"err","m":"first frame must be text auth"}"#.into()),
        );
        cache.insert(
            "err_bad_creds",
            Arc::new(r#"{"t":"err","m":"invalid credentials"}"#.into()),
        );
        cache.insert(
            "err_user_taken",
            Arc::new(r#"{"t":"err","m":"username already in use"}"#.into()),
        );
        cache.insert(
            "err_payload_size",
            Arc::new(r#"{"t":"err","m":"payload exceeds max size"}"#.into()),
        );
        cache.insert(
            "err_bad_json",
            Arc::new(r#"{"t":"err","m":"invalid JSON payload"}"#.into()),
        );
        cache.insert(
            "err_payload_obj",
            Arc::new(r#"{"t":"err","m":"payload must be a JSON object"}"#.into()),
        );
        cache.insert(
            "err_rate_limit",
            Arc::new(r#"{"t":"err","m":"too many connections"}"#.into()),
        );

        // System messages
        cache.insert("sys_left", Arc::new(r#"{"t":"sys"}"#.into()));

        Self {
            cache: RwLock::new(cache),
        }
    }

    pub fn get(&self, key: &'static str) -> Option<Arc<String>> {
        self.cache.read().get(key).cloned()
    }

    pub fn insert(&self, key: &'static str, value: String) {
        self.cache.write().insert(key, Arc::new(value));
    }
}

impl Default for StaticJsonCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Global static map of common error messages for fast path
pub static STATIC_ERRORS: std::sync::LazyLock<std::collections::HashMap<&'static str, String>> =
    std::sync::LazyLock::new(|| {
        let mut m = std::collections::HashMap::new();
        m.insert(
            "err_invalid_auth",
            r#"{"t":"err","m":"invalid auth JSON"}"#.to_string(),
        );
        m.insert(
            "err_auth_too_large",
            r#"{"t":"err","m":"auth frame too large"}"#.to_string(),
        );
        m.insert(
            "err_first_text",
            r#"{"t":"err","m":"first frame must be text auth"}"#.to_string(),
        );
        m.insert(
            "err_bad_creds",
            r#"{"t":"err","m":"invalid credentials"}"#.to_string(),
        );
        m.insert(
            "err_user_taken",
            r#"{"t":"err","m":"username already in use"}"#.to_string(),
        );
        m.insert(
            "err_payload_size",
            r#"{"t":"err","m":"payload exceeds max size"}"#.to_string(),
        );
        m.insert(
            "err_bad_json",
            r#"{"t":"err","m":"invalid JSON payload"}"#.to_string(),
        );
        m.insert(
            "err_payload_obj",
            r#"{"t":"err","m":"payload must be a JSON object"}"#.to_string(),
        );
        m
    });

/// Connection pool statistics for monitoring
#[derive(Debug, Default)]
pub struct PoolStats {
    pub active_connections: usize,
    pub idle_connections: usize,
    pub total_connections: usize,
    pub wait_count: usize,
    pub acquisition_count: u64,
    pub release_count: u64,
}

/// Memory arena for reducing allocations in hot paths
/// Uses bump allocation for predictable memory patterns
pub struct MemoryArena {
    buffer: Vec<u8>,
    ptr: usize,
}

impl MemoryArena {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: vec![0u8; capacity],
            ptr: 0,
        }
    }

    pub fn alloc(&mut self, size: usize) -> Option<&mut [u8]> {
        let size = (size + 7) & !7; // Align to 8 bytes
        if self.ptr + size > self.buffer.len() {
            self.ptr = 0; // Reset on overflow (simple strategy)
        }
        if self.ptr + size > self.buffer.len() {
            return None; // Too large for arena
        }
        let ptr = self.ptr;
        self.ptr += size;
        Some(&mut self.buffer[ptr..ptr + size])
    }

    pub fn reset(&mut self) {
        self.ptr = 0;
    }

    pub fn capacity(&self) -> usize {
        self.buffer.len()
    }

    pub fn used(&self) -> usize {
        self.ptr
    }
}

/// Simple circuit breaker for external dependencies
pub struct CircuitBreaker {
    failure_count: usize,
    last_failure_time: std::time::Instant,
    state: CircuitState,
    threshold: usize,
    timeout: std::time::Duration,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

impl CircuitBreaker {
    pub fn new(threshold: usize, timeout_secs: u64) -> Self {
        Self {
            failure_count: 0,
            last_failure_time: std::time::Instant::now(),
            state: CircuitState::Closed,
            threshold,
            timeout: std::time::Duration::from_secs(timeout_secs),
        }
    }

    pub fn is_available(&self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if self.last_failure_time.elapsed() > self.timeout {
                    true // Allow trial
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    pub fn record_success(&mut self) {
        self.failure_count = 0;
        self.state = CircuitState::Closed;
    }

    pub fn record_failure(&mut self) {
        self.failure_count += 1;
        self.last_failure_time = std::time::Instant::now();
        if self.failure_count >= self.threshold {
            self.state = CircuitState::Open;
        }
    }

    pub fn state(&self) -> CircuitState {
        self.state
    }

    pub fn attempt_reset(&mut self) {
        if self.state == CircuitState::Open && self.last_failure_time.elapsed() > self.timeout {
            self.state = CircuitState::HalfOpen;
        }
    }
}

/// Rate limiter using token bucket algorithm
pub struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,
    last_refill: std::time::Instant,
}

impl TokenBucket {
    pub fn new(max_tokens: usize, refill_per_sec: f64) -> Self {
        Self {
            tokens: max_tokens as f64,
            max_tokens: max_tokens as f64,
            refill_rate: refill_per_sec,
            last_refill: std::time::Instant::now(),
        }
    }

    pub fn try_consume(&mut self, tokens: usize) -> bool {
        self.refill();
        if self.tokens >= tokens as f64 {
            self.tokens -= tokens as f64;
            true
        } else {
            false
        }
    }

    fn refill(&mut self) {
        let elapsed = self.last_refill.elapsed().as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = std::time::Instant::now();
    }
}

/// Message compressor for large payloads
pub struct MessageCompressor;

impl MessageCompressor {
    /// Compress using flate2 (gzip)
    pub fn compress(data: &[u8]) -> Vec<u8> {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let _ = encoder.write_all(data);
        encoder.finish().unwrap_or_else(|_| data.to_vec())
    }

    /// Decompress gzip data
    pub fn decompress(data: &[u8]) -> Vec<u8> {
        use std::io::Read;
        let mut decoder = flate2::read::GzDecoder::new(data);
        let mut result = Vec::new();
        let _ = decoder.read_to_end(&mut result);
        result
    }
}

/// Global performance metrics
pub struct Metrics {
    messages_sent: std::sync::atomic::AtomicU64,
    messages_received: std::sync::atomic::AtomicU64,
    bytes_sent: std::sync::atomic::AtomicU64,
    bytes_received: std::sync::atomic::AtomicU64,
    errors: std::sync::atomic::AtomicU64,
    connections_accepted: std::sync::atomic::AtomicU64,
    connections_closed: std::sync::atomic::AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            messages_sent: std::sync::atomic::AtomicU64::new(0),
            messages_received: std::sync::atomic::AtomicU64::new(0),
            bytes_sent: std::sync::atomic::AtomicU64::new(0),
            bytes_received: std::sync::atomic::AtomicU64::new(0),
            errors: std::sync::atomic::AtomicU64::new(0),
            connections_accepted: std::sync::atomic::AtomicU64::new(0),
            connections_closed: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn inc_sent(&self, count: usize) {
        self.messages_sent
            .fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn inc_received(&self, count: usize) {
        self.messages_received
            .fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn inc_bytes_sent(&self, bytes: usize) {
        self.bytes_sent
            .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn inc_bytes_received(&self, bytes: usize) {
        self.bytes_received
            .fetch_add(bytes as u64, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn inc_errors(&self) {
        self.errors
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn inc_accepted(&self) {
        self.connections_accepted
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn inc_closed(&self) {
        self.connections_closed
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            messages_sent: self
                .messages_sent
                .load(std::sync::atomic::Ordering::Relaxed),
            messages_received: self
                .messages_received
                .load(std::sync::atomic::Ordering::Relaxed),
            bytes_sent: self.bytes_sent.load(std::sync::atomic::Ordering::Relaxed),
            bytes_received: self
                .bytes_received
                .load(std::sync::atomic::Ordering::Relaxed),
            errors: self.errors.load(std::sync::atomic::Ordering::Relaxed),
            connections_accepted: self
                .connections_accepted
                .load(std::sync::atomic::Ordering::Relaxed),
            connections_closed: self
                .connections_closed
                .load(std::sync::atomic::Ordering::Relaxed),
            cache_hits: 0,
            cache_misses: 0,
            cache_hit_rate: 0.0,
            pool_active: 0,
            pool_idle: 0,
            pool_total: 0,
        }
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// LRU Cache for message caching - reduces DB lookups for recent messages
/// Generic over the cached value type
pub struct MessageCache<V> {
    cache: RwLock<LruCache<String, Arc<V>>>,
    _max_entries: usize,
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
}

impl<V> MessageCache<V> {
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: RwLock::new(LruCache::new(
                std::num::NonZeroUsize::new(max_entries).unwrap(),
            )),
            _max_entries: max_entries,
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn get(&self, key: &str) -> Option<Arc<V>> {
        let result = self.cache.write().get(key).cloned();
        if result.is_some() {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        result
    }

    pub fn insert(&self, key: String, value: V) {
        let arc_value = Arc::new(value);
        self.cache.write().push(key, arc_value);
    }

    pub fn invalidate(&self, key: &str) {
        self.cache.write().pop(key);
    }

    pub fn clear(&self) {
        self.cache.write().clear();
    }

    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(std::sync::atomic::Ordering::Relaxed);
        let misses = self.misses.load(std::sync::atomic::Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    pub fn stats(&self) -> (u64, u64, f64) {
        let hits = self.hits.load(std::sync::atomic::Ordering::Relaxed);
        let misses = self.misses.load(std::sync::atomic::Ordering::Relaxed);
        (hits, misses, self.hit_rate())
    }
}

/// Specialized LRU cache for vectors that avoids JSON serialization round-trips.
/// Stores Vec<T> directly instead of wrapping in serde_json::Value.
pub struct VecCache<T> {
    cache: RwLock<LruCache<String, Arc<Vec<T>>>>,
    _max_entries: usize,
    hits: std::sync::atomic::AtomicU64,
    misses: std::sync::atomic::AtomicU64,
}

impl<T: Clone> VecCache<T> {
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: RwLock::new(LruCache::new(
                std::num::NonZeroUsize::new(max_entries).unwrap(),
            )),
            _max_entries: max_entries,
            hits: std::sync::atomic::AtomicU64::new(0),
            misses: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn get(&self, key: &str) -> Option<Arc<Vec<T>>> {
        let result = self.cache.write().get(key).cloned();
        if result.is_some() {
            self.hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        result
    }

    pub fn insert(&self, key: String, value: Vec<T>) {
        let arc_value = Arc::new(value);
        self.cache.write().push(key, arc_value);
    }

    pub fn invalidate(&self, key: &str) {
        self.cache.write().pop(key);
    }

    pub fn clear(&self) {
        self.cache.write().clear();
    }

    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(std::sync::atomic::Ordering::Relaxed);
        let misses = self.misses.load(std::sync::atomic::Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            hits as f64 / total as f64
        }
    }

    pub fn stats(&self) -> (u64, u64, f64) {
        let hits = self.hits.load(std::sync::atomic::Ordering::Relaxed);
        let misses = self.misses.load(std::sync::atomic::Ordering::Relaxed);
        (hits, misses, self.hit_rate())
    }

    pub fn push_and_trim(&self, key: &str, item: T, max_len: usize) {
        let mut cache = self.cache.write();
        if let Some(existing) = cache.get_mut(key) {
            let vec = Arc::make_mut(existing);
            vec.push(item);
            if vec.len() > max_len {
                let drain_start = vec.len() - max_len;
                vec.drain(0..drain_start);
            }
        } else {
            cache.push(key.to_string(), Arc::new(vec![item]));
        }
    }
}

#[derive(Debug, Default)]
pub struct MetricsSnapshot {
    pub messages_sent: u64,
    pub messages_received: u64,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub errors: u64,
    pub connections_accepted: u64,
    pub connections_closed: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_hit_rate: f64,
    pub pool_active: usize,
    pub pool_idle: usize,
    pub pool_total: usize,
}
