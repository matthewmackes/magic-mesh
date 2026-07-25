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

const SHA256_HEX_LEN: usize = 64;

/// Only the canonical content-address form is safe to hand to a blob store.
///
/// Payload references are signed, but a valid signature does not make a
/// digest canonical: a peer can sign a path component, mixed-case hex, or an
/// otherwise malformed string.  Such values must remain inert in purge
/// accounting rather than becoming filesystem candidates downstream.
fn is_canonical_payload_digest(digest: &str) -> bool {
    digest.len() == SHA256_HEX_LEN
        && digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

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
        if !is_canonical_payload_digest(sha256_hex) {
            return false;
        }
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
                    let sha = env
                        .payload_ref
                        .as_ref()
                        .map(|p| p.sha256_hex.clone())
                        .filter(|sha| is_canonical_payload_digest(sha));
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
                    if is_canonical_payload_digest(&change.payload.sha256_hex) {
                        doc_live.insert(change.payload.sha256_hex.clone());
                    }
                    if let Some(p) = &env.payload_ref {
                        if !is_canonical_payload_digest(&p.sha256_hex) {
                            continue;
                        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use mde_collab_types::event::CollabEventKind;
    use mde_collab_types::ids::{EventId, SpaceId};
    use mde_collab_types::value::{MessageBody, PayloadRef};
    use uuid::Uuid;

    fn message_with_payload(
        event_id: u128,
        actor: &ActorId,
        clock: ActorClock,
        digest: &str,
    ) -> CollabEventEnvelope {
        CollabEventEnvelope::new(
            EventId::from_uuid(Uuid::from_u128(event_id)),
            SpaceId::nil(),
            actor.clone(),
            clock,
            clock.wall_ms as i64,
            CollabEventKind::MessagePosted {
                body: MessageBody::new("payload"),
                thread: None,
            },
        )
        .with_payload_ref(PayloadRef {
            sha256_hex: digest.to_owned(),
            len: 7,
            content_type: None,
        })
    }

    fn delete_message(event_id: u128, actor: &ActorId, clock: ActorClock) -> CollabEventEnvelope {
        CollabEventEnvelope::new(
            EventId::from_uuid(Uuid::from_u128(event_id)),
            SpaceId::nil(),
            actor.clone(),
            clock,
            clock.wall_ms as i64,
            CollabEventKind::MessageDeleted {
                target: EventId::from_uuid(Uuid::from_u128(event_id - 1)),
            },
        )
    }

    #[test]
    fn canonical_payload_digest_accepts_only_lower_hex_sha256() {
        assert!(is_canonical_payload_digest(&"a".repeat(SHA256_HEX_LEN)));
        assert!(!is_canonical_payload_digest(&"A".repeat(SHA256_HEX_LEN)));
        assert!(!is_canonical_payload_digest(
            &"0".repeat(SHA256_HEX_LEN - 1)
        ));
        assert!(!is_canonical_payload_digest(
            &"0".repeat(SHA256_HEX_LEN + 1)
        ));
        assert!(!is_canonical_payload_digest("../purge-outside-store"));
    }

    #[test]
    fn hostile_payload_digests_never_become_purge_candidates() {
        let actor = ActorId::new("alice");
        let tombstone_clock = ActorClock::at(20, 0);
        let mut members = BTreeSet::new();
        members.insert(actor.clone());
        let mut gate = PurgeGate::new();
        gate.note_ack(&actor, tombstone_clock);

        for (event_id, digest) in [
            (10, "../purge-outside-store".to_owned()),
            (20, "A".repeat(SHA256_HEX_LEN)),
            (30, "0".repeat(SHA256_HEX_LEN - 1)),
        ] {
            let post = message_with_payload(
                event_id,
                &actor,
                ActorClock::at(10, event_id as u32),
                &digest,
            );
            let delete = delete_message(event_id + 1, &actor, tombstone_clock);
            let events = vec![post, delete];

            assert!(gate.purgeable(&events, &members).is_empty());
            assert!(!gate.may_purge(&events, &members, &digest));
        }
    }

    #[test]
    fn valid_lowercase_payload_digest_keeps_ack_gated_purge_semantics() {
        let actor = ActorId::new("alice");
        let digest = "b".repeat(SHA256_HEX_LEN);
        let post_clock = ActorClock::at(10, 0);
        let tombstone_clock = ActorClock::at(20, 0);
        let post = message_with_payload(10, &actor, post_clock, &digest);
        let delete = delete_message(11, &actor, tombstone_clock);
        let mut members = BTreeSet::new();
        members.insert(actor.clone());
        let mut gate = PurgeGate::new();

        assert!(!gate.may_purge(&[post.clone(), delete.clone()], &members, &digest));
        gate.note_ack(&actor, tombstone_clock);
        assert_eq!(
            gate.purgeable(&[post, delete], &members),
            BTreeSet::from([digest.clone()])
        );
        assert!(gate.may_purge(
            &[
                message_with_payload(10, &actor, post_clock, &digest),
                delete_message(11, &actor, tombstone_clock),
            ],
            &members,
            &digest
        ));
    }
}
