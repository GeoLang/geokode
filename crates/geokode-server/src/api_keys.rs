//! API key management and rate limiting.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// API key with metadata and rate limit configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: Uuid,
    pub key_hash: String,
    pub name: String,
    pub owner: String,
    pub permissions: Vec<Permission>,
    pub rate_limit: RateLimit,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked: bool,
}

/// Permission scope for an API key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Permission {
    Read,
    Write,
    Admin,
}

/// Rate limiting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    pub requests_per_second: u32,
    pub requests_per_day: u64,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self {
            requests_per_second: 100,
            requests_per_day: 100_000,
        }
    }
}

impl RateLimit {
    pub fn free_tier() -> Self {
        Self {
            requests_per_second: 10,
            requests_per_day: 10_000,
        }
    }

    pub fn pro_tier() -> Self {
        Self {
            requests_per_second: 100,
            requests_per_day: 500_000,
        }
    }

    pub fn enterprise_tier() -> Self {
        Self {
            requests_per_second: 1000,
            requests_per_day: 0, // unlimited
        }
    }
}

/// Token bucket for rate limiting.
#[derive(Debug)]
struct TokenBucket {
    tokens: f64,
    max_tokens: f64,
    refill_rate: f64,
    last_refill: std::time::Instant,
}

impl TokenBucket {
    fn new(max_tokens: f64, refill_rate: f64) -> Self {
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate,
            last_refill: std::time::Instant::now(),
        }
    }

    fn try_consume(&mut self) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Rate limiter state.
pub struct RateLimiter {
    buckets: RwLock<HashMap<Uuid, TokenBucket>>,
    daily_counts: RwLock<HashMap<Uuid, DailyCounter>>,
}

#[derive(Debug)]
struct DailyCounter {
    requests: u64,
    reset_at: DateTime<Utc>,
}

impl DailyCounter {
    fn new() -> Self {
        Self {
            requests: 0,
            reset_at: Utc::now() + chrono::Duration::days(1),
        }
    }

    fn maybe_reset(&mut self) {
        if Utc::now() >= self.reset_at {
            self.requests = 0;
            self.reset_at = Utc::now() + chrono::Duration::days(1);
        }
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            buckets: RwLock::new(HashMap::new()),
            daily_counts: RwLock::new(HashMap::new()),
        }
    }

    /// Check if a request is allowed for the given key.
    pub async fn check_rate_limit(&self, key_id: Uuid, rate_limit: &RateLimit) -> RateLimitResult {
        let mut buckets = self.buckets.write().await;
        let bucket = buckets.entry(key_id).or_insert_with(|| {
            TokenBucket::new(
                rate_limit.requests_per_second as f64,
                rate_limit.requests_per_second as f64,
            )
        });

        if !bucket.try_consume() {
            return RateLimitResult::Denied {
                reason: "Rate limit exceeded (per-second)".into(),
                retry_after_ms: (1000.0 / rate_limit.requests_per_second as f64) as u64,
            };
        }
        drop(buckets);

        if rate_limit.requests_per_day > 0 {
            let mut counters = self.daily_counts.write().await;
            let counter = counters.entry(key_id).or_insert_with(DailyCounter::new);
            counter.maybe_reset();

            if counter.requests >= rate_limit.requests_per_day {
                return RateLimitResult::Denied {
                    reason: "Daily request limit exceeded".into(),
                    retry_after_ms: 0,
                };
            }
            counter.requests += 1;
        }

        RateLimitResult::Allowed
    }
}

/// Result of a rate limit check.
#[derive(Debug, Clone, Serialize)]
pub enum RateLimitResult {
    Allowed,
    Denied { reason: String, retry_after_ms: u64 },
}

/// API key store (in-memory).
pub struct ApiKeyStore {
    keys: Arc<RwLock<Vec<ApiKey>>>,
    pub rate_limiter: RateLimiter,
}

impl Default for ApiKeyStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiKeyStore {
    pub fn new() -> Self {
        Self {
            keys: Arc::new(RwLock::new(Vec::new())),
            rate_limiter: RateLimiter::new(),
        }
    }

    /// Validate an API key hash and return the key if valid.
    pub async fn validate(&self, key_hash: &str) -> Option<ApiKey> {
        let keys = self.keys.read().await;
        keys.iter()
            .find(|k| k.key_hash == key_hash && !k.revoked)
            .cloned()
    }

    /// Create a new API key.
    pub async fn create_key(&self, name: String, owner: String) -> (String, ApiKey) {
        let raw_key = Uuid::new_v4().to_string();
        let key_hash = format!("{:x}", sha2::Sha256::digest(raw_key.as_bytes()));
        let key = ApiKey {
            id: Uuid::new_v4(),
            key_hash: key_hash.clone(),
            name,
            owner,
            permissions: vec![Permission::Read],
            rate_limit: RateLimit::default(),
            created_at: Utc::now(),
            last_used_at: None,
            revoked: false,
        };
        self.keys.write().await.push(key.clone());
        (raw_key, key)
    }

    /// Revoke an API key.
    pub async fn revoke(&self, key_id: Uuid) -> bool {
        let mut keys = self.keys.write().await;
        if let Some(key) = keys.iter_mut().find(|k| k.id == key_id) {
            key.revoked = true;
            true
        } else {
            false
        }
    }

    /// List all keys.
    pub async fn list(&self) -> Vec<ApiKey> {
        self.keys.read().await.clone()
    }
}

use sha2::Digest;
