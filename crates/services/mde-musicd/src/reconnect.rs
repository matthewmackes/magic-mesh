//! AIR-9 (v6.1) — server-lost handling + reconnect backoff.
//!
//! When the Airsonic server drops mid-track, the daemon (AIR-2/5):
//!   * finishes the current track from the local cache **iff** it was
//!     fully cached, else hard-stops with a logged warning;
//!   * pauses the queue + surfaces a "Reconnecting…" card;
//!   * reconnects with exponential backoff (1s, 2s, 4s, …, 60s cap);
//!   * resumes the queue on reconnect.
//!
//! This module ships the two **pure** decisions — the backoff schedule
//! ([`backoff_delay_secs`]) and the lost-track action ([`lost_action`]).
//! They're exercised at runtime by `mde-musicd ping --retry N` (which
//! retries a reachability check on the real backoff schedule) so the
//! schedule is reachable + verifiable without the engine. The
//! stream-error watch + pause/resume/card side effects are AIR-9.b,
//! landing with the playback engine.

/// Default backoff base (first retry waits this many seconds).
pub const DEFAULT_BASE_SECS: u64 = 1;
/// Default backoff ceiling — never wait longer than this between tries.
pub const DEFAULT_CAP_SECS: u64 = 60;

/// Exponential backoff delay (seconds) for a 0-based `attempt`:
/// `base * 2^attempt`, capped at `cap`. With the defaults this yields
/// 1, 2, 4, 8, 16, 32, 60, 60, … — doubling until the 60 s ceiling.
///
/// Overflow-safe: a large `attempt` saturates to `cap` rather than
/// wrapping.
#[must_use]
pub fn backoff_delay_secs(attempt: u32, base_secs: u64, cap_secs: u64) -> u64 {
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
    fn lost_action_finishes_from_cache_only_when_fully_cached() {
        assert_eq!(lost_action(true), LostAction::FinishFromCache);
        assert_eq!(lost_action(false), LostAction::HardStop);
    }
}
