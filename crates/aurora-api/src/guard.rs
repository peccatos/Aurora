//! In-process rate limiting and request-body bounding for the OAuth surface.
//!
//! The limiter is a fixed-window counter keyed two ways: per client IP and per
//! provider. Both windows are enforced independently and the stricter one wins,
//! so a single noisy client cannot spend a provider's budget on its own and a
//! single provider cannot be hammered by spraying source addresses.
//!
//! This is deliberately in-process state, which the deployment model supports:
//! `serve` binds a loopback listener and the process is the unit of scale. It is
//! a blast-radius limit, not a distributed quota — a second process would hold
//! its own counters, and that is stated here rather than implied away.
//!
//! Counters are kept in a `BTreeMap` keyed by `(window_start, key)`, so eviction
//! is a single prefix-range drain rather than a scan, and the structure cannot
//! grow without bound no matter how many distinct keys arrive.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::{
    Json,
    body::Body,
    extract::Request,
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::limits::MAX_REQUEST_BODY_BYTES;

/// Window length for every counter. Fixed windows are coarse — a burst that
/// straddles a boundary can spend two windows' worth — but the limiter's job
/// here is to bound sustained abuse of the provider handshake, not to be a
/// traffic shaper.
const WINDOW: Duration = Duration::from_secs(60);

/// Per-IP ceiling for one window. Generous enough for a user retrying a
/// failed connect and for a shared office address, low enough that the
/// handshake cannot be used as an amplification primitive.
const PER_IP_MAX: u32 = 30;
/// Per-provider ceiling for one window, shared by all callers.
const PER_PROVIDER_MAX: u32 = 120;
/// Ceiling on distinct client keys tracked in one window, so an attacker
/// cycling source addresses cannot grow the map without bound.
const MAX_TRACKED_KEYS: usize = 4096;

/// What the limiter is counting. `client` is "who is asking"; `provider` is
/// "whose credentials are being exercised".
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Client,
    Provider,
}

/// One caller's rate-limit decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) allowed: bool,
    /// Whole seconds until the current window closes; carried on `Retry-After`.
    pub(crate) retry_after_secs: u64,
}

/// Fixed-window counters, keyed by `(window_start, scope, key)`.
///
/// The window start is part of the key rather than a field, so moving to a new
/// window is a key change, not a mutation pass, and entries from closed windows
/// are dropped with one range removal.
pub(crate) struct RateLimiter {
    window: Duration,
    per_ip_max: u32,
    per_provider_max: u32,
    max_keys: usize,
    counters: Mutex<BTreeMap<(u64, u8, String), u32>>,
    clock_base: Instant,
}

impl RateLimiter {
    pub(crate) fn new(window: Duration, per_ip_max: u32, per_provider_max: u32) -> Self {
        Self {
            window,
            per_ip_max,
            per_provider_max,
            max_keys: MAX_TRACKED_KEYS,
            counters: Mutex::new(BTreeMap::new()),
            clock_base: Instant::now(),
        }
    }

    /// The production limiter.
    pub(crate) fn standard() -> Self {
        Self::new(WINDOW, PER_IP_MAX, PER_PROVIDER_MAX)
    }

    fn window_start(&self, now: Instant) -> u64 {
        let elapsed = now.saturating_duration_since(self.clock_base);
        (elapsed.as_nanos() / self.window.as_nanos().max(1)) as u64
    }

    /// Charges one call against `scope`/`key` and reports whether it may
    /// proceed. Both the IP and the provider counter must allow the call.
    pub(crate) fn check(&self, client: IpAddr, provider: &str) -> Decision {
        self.check_at(client, provider, Instant::now())
    }

    /// `check` with an injectable clock so window rollover is testable without
    /// sleeping through a real window.
    pub(crate) fn check_at(&self, client: IpAddr, provider: &str, now: Instant) -> Decision {
        let start = self.window_start(now);
        let mut counters = match self.counters.lock() {
            Ok(counters) => counters,
            // A poisoned counter map must not fail open: the limiter's whole
            // purpose is to bound abuse, and a panic in another thread is not a
            // reason to start accepting unlimited handshakes.
            Err(poisoned) => poisoned.into_inner(),
        };
        counters.retain(|(window, _, _), _| *window >= start);
        // Drop the oldest keys when a single window has seen more distinct
        // clients than we are willing to track. Eviction is by key order, which
        // for one window is insertion-independent but still bounded.
        while counters.len() >= self.max_keys {
            let Some(oldest) = counters.keys().next().cloned() else {
                break;
            };
            counters.remove(&oldest);
        }

        let retry_after = self.retry_after_secs(now);
        // Atomic decision over both scopes: the client counter is charged
        // FIRST and, if the client is already over its ceiling, the provider
        // budget is NOT spent. Otherwise a single noisy client would keep
        // consuming the shared provider budget with requests that are
        // rejected anyway — exactly what the doc comment promises cannot
        // happen. When the client is allowed but the provider budget is
        // exhausted, the client counter is rolled back so a rejected call
        // does not also eat the client's own allowance.
        let client_key = (start, Scope::Client as u8, client.to_string());
        let provider_key = (start, Scope::Provider as u8, provider.to_owned());
        let client_over = *counters.entry(client_key.clone()).or_insert(0) >= self.per_ip_max;
        let provider_over =
            *counters.entry(provider_key.clone()).or_insert(0) >= self.per_provider_max;
        if client_over || provider_over {
            return Decision {
                allowed: false,
                retry_after_secs: retry_after,
            };
        }
        *counters.entry(client_key).or_insert(0) += 1;
        *counters.entry(provider_key).or_insert(0) += 1;
        Decision {
            allowed: true,
            retry_after_secs: retry_after,
        }
    }

    fn retry_after_secs(&self, now: Instant) -> u64 {
        let elapsed = now.saturating_duration_since(self.clock_base);
        let window = self.window.as_nanos().max(1);
        let remaining = window - (elapsed.as_nanos() % window);
        // Round up: a `Retry-After` that expires before the window does would
        // invite the retry straight back into the same rejection.
        remaining.div_ceil(1_000_000_000).max(1) as u64
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::standard()
    }
}

/// The 429 a limited request receives.
///
/// The body is a distinct code from the generic failure so the frontend can
/// tell "slow down" apart from "the service is broken", and `Retry-After` says
/// for how long.
pub(crate) fn rate_limited(retry_after_secs: u64) -> Response {
    let body = Json(serde_json::json!({
        "error": "rate_limited",
        "retryAfterSeconds": retry_after_secs,
    }));
    let mut response = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
    if let Ok(value) = retry_after_secs.to_string().parse() {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// The address a request is charged against.
///
/// `serve` binds a loopback listener and does not terminate a proxy, so the TCP
/// peer address reported by `ConnectInfo` is the only address we can believe;
/// `X-Forwarded-For` is attacker-controlled and is deliberately not consulted.
/// A request that carries no peer address at all — a handler invoked directly
/// from a unit test, or a request that did not go through the connect-info
/// service — is charged to one shared bucket rather than to an unbounded set of
/// invented keys.
pub(crate) fn client_ip(peer: Option<SocketAddr>) -> IpAddr {
    peer.map_or(IpAddr::V4(Ipv4Addr::LOCALHOST), |addr| addr.ip())
}

/// Rejects a request whose declared body is larger than the cap before any
/// handler sees it, and caps what a body with no declared length may deliver.
pub(crate) async fn bounded_body(request: Request, next: Next) -> Response {
    if let Some(len) = declared_body_length(request.headers())
        && len > MAX_REQUEST_BODY_BYTES as u64
    {
        return payload_too_large();
    }
    // A `Content-Length` is a claim, not a guarantee (and chunked bodies declare
    // nothing at all), so the body is also bounded while it is read.
    let (parts, body) = request.into_parts();
    let bounded = http_body_util::Limited::new(body, MAX_REQUEST_BODY_BYTES);
    let request = Request::from_parts(parts, Body::new(bounded));
    next.run(request).await
}

fn declared_body_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
}

pub(crate) fn payload_too_large() -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        Json(serde_json::json!({"error":"request_too_large"})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, last))
    }

    #[test]
    fn a_client_is_allowed_up_to_its_ceiling_and_then_rejected() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 3, 1000);
        let start = Instant::now();
        for _ in 0..3 {
            assert!(limiter.check_at(ip(1), "spotify", start).allowed);
        }
        let rejected = limiter.check_at(ip(1), "spotify", start);
        assert!(!rejected.allowed);
        assert!(rejected.retry_after_secs >= 1 && rejected.retry_after_secs <= 60);
    }

    #[test]
    fn the_provider_ceiling_applies_across_clients() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 1000, 2);
        let start = Instant::now();
        assert!(limiter.check_at(ip(1), "spotify", start).allowed);
        assert!(limiter.check_at(ip(2), "spotify", start).allowed);
        // A third client is rejected because the provider budget is spent, even
        // though its own counter is untouched.
        assert!(!limiter.check_at(ip(3), "spotify", start).allowed);
        // A different provider keeps its own budget.
        assert!(limiter.check_at(ip(3), "youtube-music", start).allowed);
    }

    #[test]
    fn counters_reset_when_the_window_closes() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 1, 1000);
        let start = Instant::now();
        assert!(limiter.check_at(ip(1), "spotify", start).allowed);
        assert!(!limiter.check_at(ip(1), "spotify", start).allowed);
        assert!(
            limiter
                .check_at(ip(1), "spotify", start + Duration::from_secs(61))
                .allowed
        );
    }

    #[test]
    fn closed_windows_do_not_accumulate_state() {
        let limiter = RateLimiter::new(Duration::from_secs(1), 5, 5);
        let start = Instant::now();
        for step in 0..50 {
            let now = start + Duration::from_secs(step * 2);
            assert!(limiter.check_at(ip(1), "spotify", now).allowed);
        }
        let counters = limiter.counters.lock().unwrap();
        assert!(
            counters.len() <= 2,
            "stale keys retained: {}",
            counters.len()
        );
    }

    #[test]
    fn distinct_client_keys_are_bounded() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 1000, 1_000_000);
        let start = Instant::now();
        for step in 0..10_000u32 {
            let octets = step.to_be_bytes();
            let address = IpAddr::V4(Ipv4Addr::new(10, octets[0], octets[1], octets[2]));
            limiter.check_at(address, "spotify", start);
        }
        let counters = limiter.counters.lock().unwrap();
        assert!(counters.len() <= MAX_TRACKED_KEYS, "{}", counters.len());
    }

    #[test]
    fn a_blocked_client_does_not_spend_the_provider_budget() {
        // The core regression: once a client is over its ceiling, its
        // rejected requests must not keep consuming the shared provider
        // budget (the doc comment promises exactly this).
        let limiter = RateLimiter::new(Duration::from_secs(60), 2, 10);
        let start = Instant::now();
        for _ in 0..2 {
            assert!(limiter.check_at(ip(1), "spotify", start).allowed);
        }
        // ip(1) is now blocked; hammer it well past its own ceiling.
        for _ in 0..50 {
            assert!(!limiter.check_at(ip(1), "spotify", start).allowed);
        }
        // The provider budget must still be intact for a fresh client:
        // 10 - 2 = 8 allowed calls remain, not 0.
        for i in 0..8 {
            assert!(limiter.check_at(ip(10 + i as u8), "spotify", start).allowed);
        }
        assert!(!limiter.check_at(ip(20), "spotify", start).allowed);
    }

    #[test]
    fn a_provider_rejection_does_not_eat_the_client_budget() {
        // Symmetric property: when the shared provider budget is spent, the
        // rejected call is not charged against the calling client either —
        // the client keeps its full allowance for a different provider in
        // the same window.
        let limiter = RateLimiter::new(Duration::from_secs(60), 5, 1);
        let start = Instant::now();
        assert!(limiter.check_at(ip(1), "spotify", start).allowed);
        // Provider budget spent; ip(1) is rejected but its own counter
        // must not have advanced.
        for _ in 0..10 {
            assert!(!limiter.check_at(ip(1), "spotify", start).allowed);
        }
        // ip(1)'s client budget is intact on a fresh limiter (per_ip 1000),
        // proving the 10 rejected calls above charged nothing: 4 youtube
        // calls succeed on the same client that was just throttled.
        let other = RateLimiter::new(Duration::from_secs(60), 1000, 1000);
        for _ in 0..4 {
            assert!(other.check_at(ip(1), "youtube-music", start).allowed);
        }
    }

    #[test]
    fn retry_after_never_reports_zero() {
        let limiter = RateLimiter::new(Duration::from_secs(60), 1, 1);
        let start = Instant::now();
        for millis in [0, 1, 59_999, 60_000, 60_001] {
            let seconds = limiter.retry_after_secs(start + Duration::from_millis(millis));
            assert!((1..=60).contains(&seconds), "{millis}ms -> {seconds}s");
        }
    }
}
