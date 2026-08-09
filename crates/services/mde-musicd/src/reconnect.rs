//! AIR-9 (v6.1) — server-lost handling + reconnect backoff.
//!
//! When the Airsonic server drops mid-track, the engine first preserves the
//! audible playhead, then retries a resumable Subsonic stream with bounded
//! exponential backoff (1s, 2s, 4s, …, 60s cap). Arbitrary direct/radio URLs
//! cannot prove an offset contract and fail closed without a from-zero replay.
//! A complete cached track remains eligible for the ordinary pre-audio cache
//! fallback; a partially-heard track never switches candidates at byte zero.
//!
//! This module ships the two **pure** decisions — the backoff schedule
//! ([`backoff_delay_secs`]) and the lost-track action ([`lost_action`]).
//! They're exercised at runtime by `mde-musicd ping --retry N` (which
//! retries a reachability check on the real backoff schedule) and by the
//! playback engine's bounded reconnect path.

/// Default backoff base (first retry waits this many seconds).
pub const DEFAULT_BASE_SECS: u64 = 1;
/// Default backoff ceiling — never wait longer than this between tries.
pub const DEFAULT_CAP_SECS: u64 = 60;
/// Smallest retry delay accepted by the backoff primitive.
///
/// A zero base or cap is an invalid retry budget: returning zero would let a
/// caller issue duplicate provider requests in a tight loop. Normalize those
/// inputs instead of turning a configuration edge case into a CPU hot loop.
pub const MIN_RETRY_DELAY_SECS: u64 = 1;
/// Maximum time allowed to establish a resumed provider connection.
pub const RECONNECT_CONNECT_TIMEOUT_SECS: u64 = 3;
/// Maximum time allowed for one resumed provider request, including its body.
/// This bounds a provider that accepts a reconnect and then stops sending.
pub const RECONNECT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Exponential backoff delay (seconds) for a 0-based `attempt`:
/// `base * 2^attempt`, capped at `cap`. With the defaults this yields
/// 1, 2, 4, 8, 16, 32, 60, 60, … — doubling until the 60 s ceiling.
///
/// Zero `base_secs` and `cap_secs` values are normalized to a one-second
/// floor, preventing a misconfigured caller from issuing duplicate retries
/// without a wait. Overflow-safe: a large `attempt` saturates to the
/// normalized cap rather than wrapping.
#[must_use]
pub fn backoff_delay_secs(attempt: u32, base_secs: u64, cap_secs: u64) -> u64 {
    let cap_secs = cap_secs.max(MIN_RETRY_DELAY_SECS);
    let base_secs = base_secs.max(MIN_RETRY_DELAY_SECS).min(cap_secs);
    base_secs
        .checked_shl(attempt)
        .unwrap_or(u64::MAX)
        .min(cap_secs)
}

/// The backoff schedule for the first `n` attempts, using the defaults.
#[must_use]
pub fn default_schedule(n: u32) -> Vec<u64> {
    (0..n)
        .map(|a| backoff_delay_secs(a, DEFAULT_BASE_SECS, DEFAULT_CAP_SECS))
        .collect()
}

/// What to do when the stream is lost mid-track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LostAction {
    /// The track was fully cached — play it to the end from the cache.
    FinishFromCache,
    /// Only partially streamed — stop now (a logged warning), then the
    /// reconnect loop takes over.
    HardStop,
}

/// Decide the lost-track action from whether the current track is fully
/// present in the local cache (AIR-7).
#[must_use]
pub fn lost_action(fully_cached: bool) -> LostAction {
    if fully_cached {
        LostAction::FinishFromCache
    } else {
        LostAction::HardStop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_then_caps() {
        let s = default_schedule(9);
        assert_eq!(s, vec![1, 2, 4, 8, 16, 32, 60, 60, 60]);
    }

    #[test]
    fn backoff_respects_custom_base_and_cap() {
        // base 2, cap 10: 2,4,8,10,10,...
        assert_eq!(backoff_delay_secs(0, 2, 10), 2);
        assert_eq!(backoff_delay_secs(1, 2, 10), 4);
        assert_eq!(backoff_delay_secs(2, 2, 10), 8);
        assert_eq!(backoff_delay_secs(3, 2, 10), 10);
        assert_eq!(backoff_delay_secs(9, 2, 10), 10);
    }

    #[test]
    fn backoff_large_attempt_saturates_to_cap_not_wrap() {
        // 1 << 100 overflows u64 → saturates to cap, never panics/wraps.
        assert_eq!(backoff_delay_secs(100, 1, 60), 60);
    }

    #[test]
    fn zero_retry_budget_is_normalized_to_avoid_a_hot_loop() {
        assert_eq!(backoff_delay_secs(0, 0, 60), MIN_RETRY_DELAY_SECS);
        assert_eq!(backoff_delay_secs(1, 0, 60), 2);
        assert_eq!(backoff_delay_secs(0, 1, 0), MIN_RETRY_DELAY_SECS);
        assert!((0..8).all(|attempt| backoff_delay_secs(attempt, 0, 0) >= MIN_RETRY_DELAY_SECS));
    }

    #[test]
    fn reconnect_request_budget_is_finite_and_longer_than_connect_budget() {
        assert!(RECONNECT_CONNECT_TIMEOUT_SECS > 0);
        assert!(RECONNECT_REQUEST_TIMEOUT_SECS >= RECONNECT_CONNECT_TIMEOUT_SECS);
    }

    #[test]
    fn lost_action_finishes_from_cache_only_when_fully_cached() {
        assert_eq!(lost_action(true), LostAction::FinishFromCache);
        assert_eq!(lost_action(false), LostAction::HardStop);
    }
}
