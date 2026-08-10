//! [`CollabEngine`] — the ergonomic tie of the whole core: it holds the local
//! actor + its HLC, the in-memory canonical event set (for validation + merge
//! dedup), the folded [`DomainState`], the SQLite [`Projection`], and the
//! tombstone [`PurgeGate`].
//!
//! The two entry points mirror the spec's data flow:
//!
//! * [`apply`](CollabEngine::apply) — the local command path: validate against
//!   the folded state, mint + sign the resulting event(s), and ingest them.
//! * [`merge`](CollabEngine::merge) — the replication path: verify signatures
//!   (drop invalid), dedup by [`EventId`], ingest the rest. Because ingest folds
//!   the projection order-independently, two engines fed the same events in any
//!   order converge to byte-identical projected state.
//!
//! A disconnected engine keeps serving reads off its cached projection and, on
//! reconnect, converges by `merge`-ing the events it missed — there is no fixed
//! centre.

use std::collections::{BTreeMap, BTreeSet};

use mde_collab_types::envelope::SCHEMA_VERSION;
use mde_collab_types::event::CollabEventKind;
use mde_collab_types::ids::{EventId, SpaceId};
use mde_collab_types::{ActorClock, ActorId, CollabCommand, CollabEventEnvelope};

use crate::domain::DomainState;
use crate::error::{CollabError, Result};
use crate::pipeline::{apply_command, ApplyCtx};
use crate::projection::Projection;
use crate::purge::PurgeGate;
use crate::signer::{EventSigner, IdSource};

/// Maximum number of envelopes one replication merge may inspect at once.
///
/// Rejecting an oversized batch preserves the all-or-nothing merge contract;
/// silently truncating it would make peers appear converged while dropping
/// accepted events.
const MAX_MERGE_BATCH_EVENTS: usize = 4096;

/// Why a replicated event was rejected before it could enter the convergent
/// event set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeRejectionReason {
    /// The envelope uses a schema version this engine does not understand.
    UnsupportedSchema,
    /// The envelope is unsigned, malformed, or its signature does not verify.
    InvalidSignature,
    /// The event id was already observed with different signed contents.
    ConflictingDuplicate,
}

/// An event rejected during [`CollabEngine::merge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeRejection {
    /// The stable id of the rejected envelope.
    pub event_id: EventId,
    /// The fail-closed reason the envelope was rejected.
    pub reason: MergeRejectionReason,
}

/// The outcome of a [`merge`](CollabEngine::merge): how many incoming events
/// were newly accepted, rejected for validation, or already held. Rejections
/// are retained as bounded per-event diagnostics so a replication worker can
/// report or schedule a resync without guessing from an aggregate count.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeOutcome {
    /// Newly accepted (valid signature, not a duplicate).
    pub accepted: usize,
    /// Dropped: signature absent, malformed, or did not verify.
    pub dropped_invalid: usize,
    /// Skipped: already present (idempotent duplicate delivery).
    pub duplicates: usize,
    /// Per-event diagnostics for the rejected envelopes, in incoming order.
    pub rejected: Vec<MergeRejection>,
}

/// The headless collaboration engine for one local actor.
pub struct CollabEngine {
    actor: ActorId,
    clock: ActorClock,
    events: BTreeMap<EventId, CollabEventEnvelope>,
    state: DomainState,
    projection: Projection,
    purge: PurgeGate,
}

impl CollabEngine {
    /// Build an engine over an existing projection for `actor`.
    #[must_use]
    pub fn new(actor: impl Into<ActorId>, projection: Projection) -> Self {
        Self {
            actor: actor.into(),
            clock: ActorClock::zero(),
            events: BTreeMap::new(),
            state: DomainState::default(),
            projection,
            purge: PurgeGate::new(),
        }
    }

    /// Build an engine backed by an in-memory projection (tests, transient).
    pub fn in_memory(actor: impl Into<ActorId>) -> Result<Self> {
        Ok(Self::new(actor, Projection::open_in_memory()?))
    }

    /// The local actor.
    #[must_use]
    pub fn actor(&self) -> &ActorId {
        &self.actor
    }

    /// The local actor's current HLC.
    #[must_use]
    pub const fn clock(&self) -> ActorClock {
        self.clock
    }

    /// The folded domain aggregate (validation facts).
    #[must_use]
    pub const fn state(&self) -> &DomainState {
        &self.state
    }

    /// The read-side projection.
    #[must_use]
    pub const fn projection(&self) -> &Projection {
        &self.projection
    }

    /// The tombstone purge gate.
    #[must_use]
    pub const fn purge_gate(&self) -> &PurgeGate {
        &self.purge
    }

    /// Every event the engine holds, in canonical order.
    #[must_use]
    pub fn all_events(&self) -> Vec<CollabEventEnvelope> {
        let mut v: Vec<_> = self.events.values().cloned().collect();
        crate::domain::canonical_sort(&mut v);
        v
    }

    /// Validate `cmd`, mint + sign the resulting event(s), ingest them, and
    /// return them. A rejected command returns a typed error and mutates nothing.
    pub fn apply<S: EventSigner, I: IdSource>(
        &mut self,
        cmd: &CollabCommand,
        signer: &S,
        ids: &mut I,
        now_unix_ms: i64,
    ) -> Result<Vec<CollabEventEnvelope>> {
        let mut ctx = ApplyCtx {
            actor: self.actor.clone(),
            now_unix_ms,
            clock: self.clock,
            signer,
            ids,
        };
        let events = apply_command(&self.state, cmd, &mut ctx)?;
        self.ingest(&events)?;
        self.clock = ctx.clock;
        Ok(events)
    }

    /// Author a worker-adapted event `kind` in `space` directly, with **no**
    /// originating command: mint + HLC-stamp + sign it with the local actor's
    /// identity, ingest it, and return the signed envelope. The fold path for the
    /// event classes no command produces —
    /// [`AlertRaised`](CollabEventKind::AlertRaised) and
    /// [`ClipboardPublished`](CollabEventKind::ClipboardPublished) adapted from a
    /// truthful external Bus lane. Mirrors [`apply`](Self::apply)'s clock-advance +
    /// ingest, but skips command validation (the caller vouches for the local
    /// fact); the resulting event replicates + converges like any other.
    pub fn author<S: EventSigner, I: IdSource>(
        &mut self,
        space: SpaceId,
        kind: CollabEventKind,
        signer: &S,
        ids: &mut I,
        now_unix_ms: i64,
    ) -> Result<CollabEventEnvelope> {
        let mut ctx = ApplyCtx {
            actor: self.actor.clone(),
            now_unix_ms,
            clock: self.clock,
            signer,
            ids,
        };
        let env = ctx.author(space, kind);
        self.ingest(std::slice::from_ref(&env))?;
        self.clock = ctx.clock;
        Ok(env)
    }

    /// Merge replicated events from a peer: signature-check (drop invalid),
    /// dedup, and ingest the rest. Order-independent + idempotent.
    pub fn merge(&mut self, incoming: Vec<CollabEventEnvelope>) -> Result<MergeOutcome> {
        if incoming.len() > MAX_MERGE_BATCH_EVENTS {
            return Err(CollabError::Serde(format!(
                "merge batch exceeds {MAX_MERGE_BATCH_EVENTS} events"
            )));
        }
        let mut outcome = MergeOutcome::default();
        let mut accept: Vec<CollabEventEnvelope> = Vec::new();
        let mut pending: BTreeMap<EventId, CollabEventEnvelope> = BTreeMap::new();
        let mut merged_clock = self.clock;
        for env in incoming {
            if env.schema_version != SCHEMA_VERSION {
                outcome.dropped_invalid += 1;
                outcome.rejected.push(MergeRejection {
                    event_id: env.event_id,
                    reason: MergeRejectionReason::UnsupportedSchema,
                });
                continue;
            }
            if !env.verify() {
                outcome.dropped_invalid += 1;
                outcome.rejected.push(MergeRejection {
                    event_id: env.event_id,
                    reason: MergeRejectionReason::InvalidSignature,
                });
                continue;
            }
            if let Some(existing) = self.events.get(&env.event_id) {
                if existing == &env {
                    outcome.duplicates += 1;
                } else {
                    outcome.dropped_invalid += 1;
                    outcome.rejected.push(MergeRejection {
                        event_id: env.event_id,
                        reason: MergeRejectionReason::ConflictingDuplicate,
                    });
                }
                continue;
            }
            if let Some(existing) = pending.get(&env.event_id) {
                if existing == &env {
                    outcome.duplicates += 1;
                } else {
                    outcome.dropped_invalid += 1;
                    outcome.rejected.push(MergeRejection {
                        event_id: env.event_id,
                        reason: MergeRejectionReason::ConflictingDuplicate,
                    });
                }
                continue;
            }
            // Advance our own clock past the observed one (HLC receive rule) so a
            // subsequent local event still dominates everything we have seen.
            merged_clock = merged_clock.merge(env.clock, merged_clock.wall_ms);
            pending.insert(env.event_id, env.clone());
            accept.push(env);
        }
        outcome.accepted = accept.len();
        if !accept.is_empty() {
            self.ingest(&accept)?;
            self.clock = merged_clock;
        }
        Ok(outcome)
    }

    /// Add already-validated events to the in-memory set, refold the domain
    /// aggregate, project them, and advance each author's purge-ack high-water.
    fn ingest(&mut self, events: &[CollabEventEnvelope]) -> Result<()> {
        // The durable projection is the commit point for both local authoring
        // and replicated offline replay. Project first: if SQLite rejects the
        // batch, the live engine must not expose events/state/purge acks that a
        // restarted engine cannot recover. Everything below is infallible.
        self.projection.project(events)?;
        for env in events {
            self.events.insert(env.event_id, env.clone());
            self.purge.note_ack(&env.actor, env.clock);
        }
        // WL-FUNC-011 Phase 1 follow-up: refold the whole aggregate for
        // simplicity + obvious correctness; a worker at fleet scale would fold
        // incrementally per touched space.
        let all: Vec<_> = self.events.values().cloned().collect();
        self.state = DomainState::from_events(&all);
        Ok(())
    }

    /// The set of present members of `space` (the members that must ack a
    /// tombstone before its payload may be purged).
    #[must_use]
    pub fn space_members(&self, space: SpaceId) -> BTreeSet<ActorId> {
        self.state
            .space(space)
            .map(|s| {
                s.members
                    .iter()
                    .filter(|(_, m)| m.present)
                    .map(|(a, _)| a.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Record that `actor` has replicated up to `clock` (their tombstone ack).
    pub fn note_purge_ack(&mut self, actor: &ActorId, clock: ActorClock) {
        self.purge.note_ack(actor, clock);
    }

    /// Digests safe to purge from the blob store for `space`'s membership.
    #[must_use]
    pub fn purgeable_payloads(&self, space: SpaceId) -> BTreeSet<String> {
        let members = self.space_members(space);
        self.purge.purgeable(&self.all_events(), &members)
    }

    /// Whether `sha256_hex` may be purged now, for `space`'s membership.
    #[must_use]
    pub fn may_purge(&self, space: SpaceId, sha256_hex: &str) -> bool {
        let members = self.space_members(space);
        self.purge
            .may_purge(&self.all_events(), &members, sha256_hex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signer::{Ed25519Signer, EventSigner};
    use mde_collab_types::space::SpaceKind;
    use uuid::Uuid;

    fn signed_event() -> CollabEventEnvelope {
        let mut event = CollabEventEnvelope::new(
            EventId::from_uuid(Uuid::from_u128(1)),
            SpaceId::from_uuid(Uuid::from_u128(2)),
            ActorId::new("alice"),
            ActorClock::at(1, 0),
            1,
            CollabEventKind::SpaceCreated {
                kind: SpaceKind::Team,
                name: "ops".into(),
            },
        );
        Ed25519Signer::from_seed([7; 32]).sign(&mut event);
        event
    }

    #[test]
    fn merge_rejects_oversized_batch_before_iteration_or_retention() {
        let mut engine = CollabEngine::in_memory("viewer").expect("engine");
        let error = engine
            .merge(vec![signed_event(); MAX_MERGE_BATCH_EVENTS + 1])
            .expect_err("oversized merge must fail closed");

        assert!(matches!(
            error,
            CollabError::Serde(message)
                if message == format!("merge batch exceeds {MAX_MERGE_BATCH_EVENTS} events")
        ));
        assert!(engine.all_events().is_empty());
    }

    #[test]
    fn merge_accepts_the_normal_batch_boundary() {
        let mut engine = CollabEngine::in_memory("viewer").expect("engine");
        let event = signed_event();
        let invalid = CollabEventEnvelope {
            signature: None,
            ..event.clone()
        };
        let mut batch = vec![event.clone()];
        batch.extend(vec![invalid; MAX_MERGE_BATCH_EVENTS - 1]);

        let outcome = engine.merge(batch).expect("boundary-sized merge");

        assert_eq!(outcome.accepted, 0);
        assert_eq!(outcome.duplicates, 0);
        assert_eq!(outcome.dropped_invalid, MAX_MERGE_BATCH_EVENTS - 1);
        assert_eq!(engine.all_events(), vec![event]);
    }

    #[test]
    fn merge_reports_rejection_reason_without_retaining_or_advancing() {
        let mut engine = CollabEngine::in_memory("viewer").expect("engine");
        let mut unsupported = signed_event();
        unsupported.event_id = EventId::from_uuid(Uuid::from_u128(3));
        unsupported.schema_version = SCHEMA_VERSION + 1;
        let mut unsigned = signed_event();
        unsigned.event_id = EventId::from_uuid(Uuid::from_u128(4));
        unsigned.signature = None;

        let outcome = engine
            .merge(vec![unsupported, unsigned])
            .expect("invalid events are reported, not fatal to the batch");

        assert_eq!(outcome.accepted, 1);
        assert_eq!(outcome.duplicates, 0);
        assert_eq!(outcome.dropped_invalid, 2);
        assert_eq!(
            outcome.rejected,
            vec![
                MergeRejection {
                    event_id: EventId::from_uuid(Uuid::from_u128(3)),
                    reason: MergeRejectionReason::UnsupportedSchema,
                },
                MergeRejection {
                    event_id: EventId::from_uuid(Uuid::from_u128(4)),
                    reason: MergeRejectionReason::InvalidSignature,
                },
            ]
        );
        assert!(engine.all_events().is_empty());
        assert_eq!(engine.clock(), ActorClock::zero());
    }

    #[test]
    fn merge_rejects_conflicting_event_id_reuse_in_log_and_batch() {
        let mut engine = CollabEngine::in_memory("viewer").expect("engine");
        let original = signed_event();
        engine
            .merge(vec![original.clone()])
            .expect("original event");

        let mut conflict = original.clone();
        conflict.kind = CollabEventKind::SpaceCreated {
            kind: SpaceKind::Team,
            name: "different".into(),
        };
        Ed25519Signer::from_seed([7; 32]).sign(&mut conflict);

        let mut batch_original = signed_event();
        batch_original.event_id = EventId::from_uuid(Uuid::from_u128(5));
        Ed25519Signer::from_seed([7; 32]).sign(&mut batch_original);
        let mut batch_conflict = batch_original.clone();
        batch_conflict.kind = CollabEventKind::SpaceCreated {
            kind: SpaceKind::Team,
            name: "batch-different".into(),
        };
        Ed25519Signer::from_seed([7; 32]).sign(&mut batch_conflict);

        let outcome = engine
            .merge(vec![
                conflict,
                original.clone(),
                batch_original.clone(),
                batch_conflict,
            ])
            .expect("conflicting duplicates are reported, not fatal to the batch");

        assert_eq!(outcome.accepted, 1);
        assert_eq!(outcome.duplicates, 1);
        assert_eq!(outcome.dropped_invalid, 2);
        assert_eq!(
            outcome.rejected,
            vec![
                MergeRejection {
                    event_id: original.event_id,
                    reason: MergeRejectionReason::ConflictingDuplicate,
                },
                MergeRejection {
                    event_id: batch_original.event_id,
                    reason: MergeRejectionReason::ConflictingDuplicate,
                },
            ]
        );
        assert_eq!(engine.all_events(), vec![original, batch_original]);
    }
}
