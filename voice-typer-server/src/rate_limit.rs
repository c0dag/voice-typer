//! Tiny in-memory fixed-window rate limiter, keyed by an arbitrary string
//! (client IP for auth endpoints, user id for the proxy). Good enough for the
//! single-instance deployment; counters reset on restart.

use axum::http::HeaderMap;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct RateLimiter {
    inner: Mutex<HashMap<String, (Instant, u32)>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a hit for `key` and return true if it is within `max` hits per
    /// `window`. A poisoned lock fails open (allows the request).
    pub fn check(&self, key: &str, max: u32, window: Duration) -> bool {
        let now = Instant::now();
        let mut map = match self.inner.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        // Opportunistic cleanup so the map cannot grow without bound.
        if map.len() > 10_000 {
            map.retain(|_, (start, _)| now.duration_since(*start) < window);
        }
        let entry = map.entry(key.to_string()).or_insert((now, 0));
        if now.duration_since(entry.0) >= window {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= max
    }
}

/// Caps the number of in-flight requests per user. Bounds how far a user can
/// overshoot a quota by firing many requests at once near their cap (each would
/// otherwise read the same pre-request usage before any of them logs).
#[derive(Clone, Default)]
pub struct Concurrency {
    inner: Arc<Mutex<HashMap<i64, u32>>>,
}

/// Decrements the in-flight count for its user when dropped.
pub struct InFlight {
    inner: Arc<Mutex<HashMap<i64, u32>>>,
    user_id: i64,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut m = match self.inner.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        if let Some(c) = m.get_mut(&self.user_id) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                m.remove(&self.user_id);
            }
        }
    }
}

impl Concurrency {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve a slot for `user_id`; returns a guard if fewer than `max` are in
    /// flight, else None. The slot is released when the guard drops.
    pub fn try_acquire(&self, user_id: i64, max: u32) -> Option<InFlight> {
        let mut m = match self.inner.lock() {
            Ok(m) => m,
            Err(p) => p.into_inner(),
        };
        let c = m.entry(user_id).or_insert(0);
        if *c >= max {
            return None;
        }
        *c += 1;
        Some(InFlight {
            inner: self.inner.clone(),
            user_id,
        })
    }
}

/// Best-effort real client IP. Behind Cloudflare the socket peer is the tunnel,
/// so trust `CF-Connecting-IP` first, then the first `X-Forwarded-For` hop.
pub fn client_ip(headers: &HeaderMap) -> String {
    if let Some(ip) = headers.get("cf-connecting-ip").and_then(|v| v.to_str().ok()) {
        let ip = ip.trim();
        if !ip.is_empty() {
            return ip.to_string();
        }
    }
    if let Some(xff) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = xff.split(',').next() {
            let first = first.trim();
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    "local".to_string()
}
