//! The actor identity and the logical clock that order a space's event log.
//!
//! This crate is **pure**: it never reads a wall clock. Every physical time is
//! injected by the caller (the worker), so the same event log replays
//! identically. Causal ordering across nodes comes from the [`ActorClock`], a
//! Hybrid Logical Clock (HLC): a physical component that keeps the log roughly
//! chronological, plus a logical counter that (a) breaks ties within one
//! millisecond and (b) preserves happens-before even when an observed clock
//! runs ahead of local wall time.

use core::fmt;

use serde::{Deserialize, Serialize};

/// The identity an event is authored *by* — the hostname, which the platform
/// treats as the username (the chat "the hostname *is* the username" rule).
///
/// The identity is a plain string; binding it to the Ed25519 public key that
/// actually signed the event is the roster/trust layer's job, not this leaf
/// crate's. The signature ([`EventSignature`](crate::EventSignature)) carries
/// the key; the actor names *who* the signer claims to be.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ActorId(pub String);

impl ActorId {
    /// Wrap a hostname/identity string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The identity as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ActorId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for ActorId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// A Hybrid Logical Clock stamp on an event — the causal ordering key.
///
/// Ordering is lexicographic on `(wall_ms, counter)` (the derived `Ord`), so
/// sorting a merged multi-node log by clock, then by
/// [`EventId`](crate::EventId) as a final tiebreak, is deterministic and
/// convergent regardless of arrival order.
///
/// The clock is advanced with [`tick`](Self::tick) for a local event and
/// [`merge`](Self::merge) when receiving a remote event; both take the caller's
/// injected physical time, keeping the type pure.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct ActorClock {
    /// The physical component: epoch milliseconds, monotonic non-decreasing as
    /// the clock advances (it never runs *backwards* even if wall time does).
    pub wall_ms: u64,
    /// The logical component: increments to break ties inside one `wall_ms` and
    /// to keep advancing when the observed physical time has not moved.
    pub counter: u32,
}

impl ActorClock {
    /// The zero clock — the start of a fresh log.
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            wall_ms: 0,
            counter: 0,
        }
    }

    /// Construct a clock from explicit components (deterministic tests, or
    /// rehydrating a persisted stamp).
    #[must_use]
    pub const fn at(wall_ms: u64, counter: u32) -> Self {
        Self { wall_ms, counter }
    }

    /// Advance for a **local** event observed at injected physical time
    /// `now_unix_ms` (HLC send/local rule): if wall time moved forward, adopt it
    /// and reset the counter; otherwise hold the (non-decreasing) wall time and
    /// bump the counter.
    #[must_use]
    pub fn tick(self, now_unix_ms: u64) -> Self {
        if now_unix_ms > self.wall_ms {
            Self {
                wall_ms: now_unix_ms,
                counter: 0,
            }
        } else {
            Self {
                wall_ms: self.wall_ms,
                counter: self.counter.saturating_add(1),
            }
        }
    }

    /// Advance on **receiving** a remote event whose clock is `observed`, at
    /// injected local physical time `now_unix_ms` (the standard HLC receive
    /// rule). The result strictly dominates both `self` and `observed`, so the
    /// merged log stays causally ordered.
    #[must_use]
    pub fn merge(self, observed: Self, now_unix_ms: u64) -> Self {
        let wall = self.wall_ms.max(observed.wall_ms).max(now_unix_ms);
        let counter = if wall == self.wall_ms && wall == observed.wall_ms {
            self.counter.max(observed.counter).saturating_add(1)
        } else if wall == self.wall_ms {
            self.counter.saturating_add(1)
        } else if wall == observed.wall_ms {
            observed.counter.saturating_add(1)
        } else {
            0
        };
        Self {
            wall_ms: wall,
            counter,
        }
    }
}

impl fmt::Display for ActorClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.wall_ms, self.counter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_adopts_forward_wall_time_and_resets_counter() {
        let c = ActorClock::at(100, 5);
        let n = c.tick(200);
        assert_eq!(n, ActorClock::at(200, 0));
    }

    #[test]
    fn tick_bumps_counter_when_wall_time_stalls() {
        let c = ActorClock::at(200, 0);
        let n = c.tick(150); // local wall went backwards / stalled
        assert_eq!(n, ActorClock::at(200, 1), "never regress wall_ms");
        assert!(n > c, "still strictly advances");
    }

    #[test]
    fn merge_dominates_both_inputs() {
        let local = ActorClock::at(200, 3);
        let remote = ActorClock::at(200, 7);
        let merged = local.merge(remote, 150);
        assert_eq!(merged, ActorClock::at(200, 8), "max counter + 1");
        assert!(merged > local && merged > remote);
    }

    #[test]
    fn merge_takes_the_furthest_ahead_component() {
        let local = ActorClock::at(100, 9);
        let remote = ActorClock::at(500, 2);
        let merged = local.merge(remote, 120);
        assert_eq!(
            merged,
            ActorClock::at(500, 3),
            "adopt remote wall + its counter"
        );
    }

    #[test]
    fn clock_orders_lexicographically() {
        assert!(ActorClock::at(1, 9) < ActorClock::at(2, 0));
        assert!(ActorClock::at(2, 0) < ActorClock::at(2, 1));
    }

    #[test]
    fn actor_id_round_trips_transparently() {
        let a = ActorId::new("eagle");
        let json = serde_json::to_string(&a).expect("serialize");
        assert_eq!(json, "\"eagle\"");
        let back: ActorId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, a);
    }
}
