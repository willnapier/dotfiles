//! Hand-rolled per-IP leaky bucket. 60 req/min/IP per §7.
//!
//! Per §7 small enough to roll our own. Source IP is the `CF-Connecting-IP`
//! header; the caller rejects requests missing it.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CAPACITY: f64 = 60.0;
/// 60 tokens per 60s → 1 token/sec refill.
const REFILL_PER_SEC: f64 = 1.0;

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Bucket {
            tokens: CAPACITY,
            last_refill: now,
        }
    }

    /// Refill based on elapsed time and try to consume one token.
    /// Returns true if the request is allowed.
    fn try_consume(&mut self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last_refill);
        let refill = elapsed.as_secs_f64() * REFILL_PER_SEC;
        if refill > 0.0 {
            self.tokens = (self.tokens + refill).min(CAPACITY);
            self.last_refill = now;
        }
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        RateLimiter {
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Try to admit a request from the given IP.
    /// Returns true if allowed, false if rate-limited.
    pub fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("ratelimit mutex poisoned");
        // Opportunistically GC stale buckets (full + idle > 5 min) to keep memory bounded.
        // Cheap: only runs when we're touching the map anyway.
        if buckets.len() > 1024 {
            buckets.retain(|_, b| {
                let idle = now.saturating_duration_since(b.last_refill);
                !(b.tokens >= CAPACITY && idle > Duration::from_secs(300))
            });
        }
        let bucket = buckets.entry(ip).or_insert_with(|| Bucket::new(now));
        bucket.try_consume(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn allows_under_capacity() {
        let rl = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        for _ in 0..60 {
            assert!(rl.allow(ip));
        }
    }

    #[test]
    fn denies_over_capacity() {
        let rl = RateLimiter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        for _ in 0..60 {
            assert!(rl.allow(ip));
        }
        // 61st request — bucket is empty, no time has passed to refill.
        assert!(!rl.allow(ip));
    }

    #[test]
    fn separate_ips_have_separate_buckets() {
        let rl = RateLimiter::new();
        let a = IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4));
        let b = IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8));
        for _ in 0..60 {
            assert!(rl.allow(a));
        }
        // A is exhausted; B is fresh.
        assert!(!rl.allow(a));
        assert!(rl.allow(b));
    }
}
