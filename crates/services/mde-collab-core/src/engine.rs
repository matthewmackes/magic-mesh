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
use mde_collab_types::ids::{EventId, SpaceId};
use mde_collab_types::{ActorClock, ActorId, CollabCommand, CollabEventEnvelope};

use crate::domain::DomainState;
use crate::error::Result;
use crate::pipeline::{apply_command, ApplyCtx};
use crate::projection::Projection;
use crate::purge::PurgeGate;
use crate::signer::{EventSigner, IdSource};

/// The outcome of a [`merge`](CollabEngine::merge): how many incoming events
/// were newly accepted, dropped for a bad/absent signature, or already held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MergeOutcome {
    /// Newly accepted (valid signature, not a duplicate).
    pub accepted: usize,
    /// Dropped: signature absent, malformed, or did not verify.
    pub dropped_invalid: usize,
    /// Skipped: already present (idempotent duplicate delivery).
    pub duplicates: usize,
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
        self.clock = ctx.clock;
        self.ingest(&events)?;
        Ok(events)
    }

    /// Merge replicated events from a peer: signature-check (drop invalid),
    /// dedup, and ingest the rest. Order-independent + idempotent.
    pub fn merge(&mut self, incoming: Vec<CollabEventEnvelope>) -> Result<MergeOutcome> {
        let mut outcome = MergeOutcome::default();
        let mut accept: Vec<CollabEventEnvelope> = Vec::new();
        for env in incoming {
            if env.schema_version != SCHEMA_VERSION || !env.verify() {
                outcome.dropped_invalid += 1;
                continue;
            }
            if self.events.contains_key(&env.event_id) {
                outcome.duplicates += 1;
                continue;
            }
            // Advance our own clock past the observed one (HLC receive rule) so a
            // subsequent local event still dominates everything we have seen.
            self.clock = self.clock.merge(env.clock, self.clock.wall_ms);
            accept.push(env);
        }
        outcome.accepted = accept.len();
        if !accept.is_empty() {
            self.ingest(&accept)?;
        }
        Ok(outcome)
    }

    /// Add already-validated events to the in-memory set, refold the domain
    /// aggregate, project them, and advance each author's purge-ack high-water.
    fn ingest(&mut self, events: &[CollabEventEnvelope]) -> Result<()> {
        for env in events {
            self.events.insert(env.event_id, env.clone());
            self.purge.note_ack(&env.actor, env.clock);
        }
        // WL-FUNC-011 Phase 1 follow-up: refold the whole aggregate for
        // simplicity + obvious correctness; a worker at fleet scale would fold
        // incrementally per touched space.
        let all: Vec<_> = self.events.values().cloned().collect();
        self.state = DomainState::from_events(&all);
        self.projection.project(events)?;
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
