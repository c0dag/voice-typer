//! Tiny in-memory fixed-window rate limiter, keyed by an arbitrary string
//! (client IP for auth endpoints, user id for the proxy). Good enough for the
//! single-instance deployment; counters reset on restart.

use axum::http::HeaderMap;
use std::collections::HashMap;
use std::sync::Mutex;
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
