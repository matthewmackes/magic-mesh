//! Hybrid Logical Clock + op author (locks Q5, Q64).
//!
//! Every op carries an [`Hlc`] — a `(wall_ms, counter, node)` triple that gives
//! the whole mesh a **total order** the CRDT merge decides last-writer-wins on
//! (lock Q5). The wall component keeps the order close to real time; the counter
//! disambiguates same-millisecond events; the node id makes the order total even
//! when two nodes stamp the identical `(wall, counter)`. Because the order is
//! total and injected (this crate never reads the wall clock itself), a merge
//! that folds ops by max-HLC converges regardless of arrival order.
//!
//! Each op also carries an [`Author`] — the `(user, node)` that wrote it (lock
//! Q64) — so attribution survives the merge.

use serde::{Deserialize, Serialize};

/// A mesh node identity (the enrolled host that stamped an op / ran a clock).
pub type NodeId = String;

/// A mesh user identity (the authenticated user an op is attributed to).
pub type UserId = String;

/// A Hybrid Logical Clock timestamp: `(wall_ms, counter, node)`, ordered
/// lexicographically in that priority.
///
/// The tuple is a **total order** across the mesh, which is what makes the LWW
/// merge deterministic (lock Q5).
///
/// `Ord` is derived, so the field order below *is* the comparison priority:
/// wall time first, then the counter, then the node id as the final tiebreak.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Hlc {
    /// Physical (wall-clock) time in milliseconds since the Unix epoch,
    /// injected by the caller — this crate never reads the clock itself.
    pub wall_ms: u64,
    /// Monotonic tiebreak for events that share a `wall_ms`.
    pub counter: u32,
    /// The stamping node — the final tiebreak that makes the order total even
    /// when two nodes collide on `(wall_ms, counter)`.
    pub node: NodeId,
}

impl Hlc {
    /// Construct an `Hlc` from its parts. Prefer [`HlcClock`] for minting a
    /// monotonic sequence; this is for tests and deserialized ops.
    #[must_use]
    pub const fn new(wall_ms: u64, counter: u32, node: NodeId) -> Self {
        Self {
            wall_ms,
            counter,
            node,
        }
    }
}

/// A per-node Hybrid Logical Clock generator (lock Q5).
///
/// The clock is *driven by injected physical time* — the worker passes the
/// current wall clock; the model stays pure and I/O-free. [`HlcClock::tick`]
/// mints the next local op stamp; [`HlcClock::observe`] folds in a remote op's
/// stamp so a node that has seen a "future" timestamp never mints one that
/// sorts before it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HlcClock {
    node: NodeId,
    last_wall_ms: u64,
    counter: u32,
}

impl HlcClock {
    /// Start a fresh clock for `node` (at wall time 0, counter 0).
    #[must_use]
    pub const fn new(node: NodeId) -> Self {
        Self {
            node,
            last_wall_ms: 0,
            counter: 0,
        }
    }

    /// Mint the next monotonically-increasing stamp for a **local** op, given
    /// the current wall time in milliseconds.
    ///
    /// If the wall clock advanced past the last stamp the counter resets to 0;
    /// otherwise (clock stalled or ran backwards) the last wall time is held and
    /// the counter increments, keeping every stamp from a node strictly greater
    /// than its predecessor.
    pub fn tick(&mut self, wall_now_ms: u64) -> Hlc {
        if wall_now_ms > self.last_wall_ms {
            self.last_wall_ms = wall_now_ms;
            self.counter = 0;
        } else {
            self.counter = self.counter.saturating_add(1);
        }
        Hlc::new(self.last_wall_ms, self.counter, self.node.clone())
    }

    /// Fold in a **remote** op's stamp (received during merge), then mint a
    /// local stamp that is strictly greater than both the remote stamp and the
    /// local clock — the standard HLC receive step, so causal order is never
    /// violated after seeing a peer's timestamp.
    pub fn observe(&mut self, remote: &Hlc, wall_now_ms: u64) -> Hlc {
        let ceiling = wall_now_ms.max(self.last_wall_ms).max(remote.wall_ms);
        if ceiling == self.last_wall_ms && ceiling == remote.wall_ms {
            self.counter = self.counter.max(remote.counter).saturating_add(1);
        } else if ceiling == self.last_wall_ms {
            self.counter = self.counter.saturating_add(1);
        } else if ceiling == remote.wall_ms {
            self.counter = remote.counter.saturating_add(1);
        } else {
            self.counter = 0;
        }
        self.last_wall_ms = ceiling;
        Hlc::new(self.last_wall_ms, self.counter, self.node.clone())
    }
}

/// Who wrote an op: the authenticated `user` on a `node` (lock Q64). The worker
/// only ever stamps ops for the local authenticated user; the pair survives the
/// merge so every field can name its last writer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Author {
    /// The authenticated mesh user the op is attributed to.
    pub user: UserId,
    /// The node the op was authored on.
    pub node: NodeId,
}

impl Author {
    /// Construct an author from a `user` + `node`.
    #[must_use]
    pub const fn new(user: UserId, node: NodeId) -> Self {
        Self { user, node }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hlc_orders_wall_then_counter_then_node() {
        let a = Hlc::new(10, 0, "z-node".into());
        let b = Hlc::new(10, 1, "a-node".into());
        // counter beats node id at equal wall time.
        assert!(a < b);
        let c = Hlc::new(11, 0, "a-node".into());
        // wall time beats everything.
        assert!(b < c);
        let d = Hlc::new(10, 0, "a-node".into());
        // equal wall+counter -> node id breaks the tie.
        assert!(d < a);
    }

    #[test]
    fn tick_is_strictly_monotonic_even_when_the_clock_stalls() {
        let mut clk = HlcClock::new("n1".into());
        let t0 = clk.tick(100);
        let t1 = clk.tick(100); // clock stalled
        let t2 = clk.tick(50); // clock ran backwards
        let t3 = clk.tick(200); // clock jumped forward
        assert!(t0 < t1, "stall -> counter bump");
        assert!(t1 < t2, "backwards -> counter bump, wall held");
        assert!(t2 < t3, "forward -> new wall, counter reset");
        assert_eq!(t3.counter, 0);
    }

    #[test]
    fn observe_beats_a_future_remote_stamp() {
        let mut clk = HlcClock::new("n1".into());
        let _ = clk.tick(100);
        let remote = Hlc::new(500, 9, "n2".into());
        let next = clk.observe(&remote, 100);
        assert!(
            next > remote,
            "local stamp must dominate an observed remote"
        );
    }
}
