//! Tombstone-gated **payload purge**.
//!
//! Deletion is convergent: a [`MessageDeleted`](mde_collab_types::event::CollabEventKind::MessageDeleted)
//! tombstone (folded by the projection) is sticky — a stale peer re-delivering
//! the original event can never resurrect the content, because the fold marks
//! the message deleted whenever *any* valid delete is in the set, regardless of
//! order.
//!
//! Reclaiming the deleted *bytes* is a stronger step and is gated: a
//! content-addressed payload may be purged from the blob store only when
//!
//! 1. it is referenced by a tombstoned (deleted) message, and
//! 2. no *live* (non-deleted) event still references the same bytes, and
//! 3. **every known member has acked** the tombstone — modelled here as each
//!    member's replicated high-water clock having reached (>=) the tombstone's
//!    clock, i.e. they have all seen the deletion.
//!
//! Canonical file bytes (a [`FileRef`](mde_collab_types::value::FileRef)'s
//! sha256) are deliberately **out of scope**: unlinking a file or deleting a
//! space never purges the canonical file, which may be referenced elsewhere.

use std::collections::{BTreeMap, BTreeSet};

use mde_collab_types::event::CollabEventKind;
use mde_collab_types::{ActorClock, ActorId, CollabEventEnvelope};

/// Tracks each member's replicated high-water clock (their "ack" of everything
/// up to that point) and decides which deleted payloads are safe to purge.
#[derive(Debug, Default, Clone)]
pub struct PurgeGate {
    /// actor → the highest clock that actor is known to have replicated.
    acks: BTreeMap<ActorId, ActorClock>,
}

impl PurgeGate {
    /// A fresh gate with no acks recorded.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `actor` has replicated up to `clock` (monotonic — a lower
    /// clock never regresses the high-water).
    pub fn note_ack(&mut self, actor: &ActorId, clock: ActorClock) {
        let entry = self.acks.entry(actor.clone()).or_insert(ActorClock::zero());
        if clock > *entry {
            *entry = clock;
        }
    }

    /// This actor's recorded high-water clock (the zero clock if none).
    #[must_use]
    pub fn high_water(&self, actor: &ActorId) -> ActorClock {
        self.acks
            .get(actor)
            .copied()
            .unwrap_or_else(ActorClock::zero)
    }

    /// Whether every member in `known_members` has acked at least `clock`.
    #[must_use]
    pub fn all_acked(&self, known_members: &BTreeSet<ActorId>, clock: ActorClock) -> bool {
        known_members.iter().all(|m| self.high_water(m) >= clock)
    }

    /// The set of payload digests that are safe to purge from the blob store,
    /// given the full event set and the members that must ack.
    #[must_use]
    pub fn purgeable(
        &self,
        events: &[CollabEventEnvelope],
        known_members: &BTreeSet<ActorId>,
    ) -> BTreeSet<String> {
        let refs = PayloadRefs::scan(events);
        refs.tombstoned
            .iter()
            .filter(|(sha, _)| !refs.live.contains(*sha))
            .filter(|(_, clk)| self.all_acked(known_members, **clk))
            .map(|(sha, _)| sha.clone())
            .collect()
    }

    /// Whether a specific payload digest may be purged now.
    #[must_use]
    pub fn may_purge(
        &self,
        events: &[CollabEventEnvelope],
        known_members: &BTreeSet<ActorId>,
        sha256_hex: &str,
    ) -> bool {
        self.purgeable(events, known_members).contains(sha256_hex)
    }
}

/// The live vs. tombstoned payload references discovered in an event set.
struct PayloadRefs {
    /// Digests still referenced by a non-deleted event (never purge these).
    live: BTreeSet<String>,
    /// Digests referenced by a deleted message → the tombstone clock.
    tombstoned: BTreeMap<String, ActorClock>,
}

impl PayloadRefs {
    fn scan(events: &[CollabEventEnvelope]) -> Self {
        // Map each message event → (its payload digest, author).
        let mut msg_payload: BTreeMap<mde_collab_types::ids::EventId, (Option<String>, ActorId)> =
            BTreeMap::new();
        // Deleted message targets → the delete clock (max, canonical order).
        let mut deletes: BTreeMap<mde_collab_types::ids::EventId, (ActorClock, ActorId)> =
            BTreeMap::new();
        // Digests referenced by a still-live document update (kept alive).
        let mut doc_live: BTreeSet<String> = BTreeSet::new();

        for env in events {
            match &env.kind {
                CollabEventKind::MessagePosted { .. } => {
                    let sha = env.payload_ref.as_ref().map(|p| p.sha256_hex.clone());
                    msg_payload.insert(env.event_id, (sha, env.actor.clone()));
                }
                CollabEventKind::MessageDeleted { target } => {
                    let e = deletes
                        .entry(*target)
                        .or_insert((ActorClock::zero(), env.actor.clone()));
                    if env.clock > e.0 {
                        *e = (env.clock, env.actor.clone());
                    }
                }
                CollabEventKind::DocumentUpdated { change, .. } => {
                    doc_live.insert(change.payload.sha256_hex.clone());
                    if let Some(p) = &env.payload_ref {
                        doc_live.insert(p.sha256_hex.clone());
                    }
                }
                _ => {}
            }
        }

        let mut live: BTreeSet<String> = doc_live;
        let mut tombstoned: BTreeMap<String, ActorClock> = BTreeMap::new();
        for (id, (sha, author)) in &msg_payload {
            let Some(sha) = sha else { continue };
            // A delete only tombstones when authored by the message's author.
            let deleted = match deletes.get(id) {
                Some((clk, deleter)) if deleter == author => Some(*clk),
                _ => None,
            };
            match deleted {
                Some(clk) => {
                    let e = tombstoned.entry(sha.clone()).or_insert(clk);
                    if clk > *e {
                        *e = clk;
                    }
                }
                None => {
                    live.insert(sha.clone());
                }
            }
        }
        // A digest that is both live and tombstoned stays live (some other
        // message or document still references those exact bytes).
        tombstoned.retain(|sha, _| !live.contains(sha));
        Self { live, tombstoned }
    }
}
