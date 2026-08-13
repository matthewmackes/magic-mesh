# WL-FUNC-011 call-media session provenance — r495

Date: 2026-08-13
Epic: `WL-FUNC-011`
Scope: `crates/mesh/mackesd/src/workers/collab_media.rs`

## Executable gap

The retained call-media readiness board named a `local_actor`, but the media
verifier did not prove that each session contained that actor exactly once.
Malformed retained state could also repeat participants, contradict its
`AdapterReady`/`WaitingForConnectedPeer` admission, or repeat a call identity.
Such a row could reach a registered provider and publish fresh
`LiveMediaVerified` evidence with the wrong session provenance.

## Implemented boundary

Before any provider probe, the worker now rejects the whole readiness board when:

- a call identity occurs more than once;
- the board's local actor is absent or duplicated in a session;
- any connected participant is duplicated; or
- participant cardinality contradicts the declared admission state.

Rejection uses the existing retained-verification tombstone path, so an invalid
replacement revokes previously published live proof. Corrected-forward state can
subsequently be sampled without restarting the daemon.

## Farm evidence

- `.90`, workspace `func011-media-provenance-clippy-r495`:
  `cargo test -p mackesd --lib workers::collab_media::tests::invalid_session_provenance_revokes_live_proof_without_provider_probe -- --exact --nocapture`
  passed 1/1, with 4,938 filtered tests. The regression first publishes valid
  live proof, replaces readiness with a board that omits the declared local
  actor, proves the provider was not probed again, and observes the retained
  verification tombstone.
- `.90`, workspace `func011-media-provenance-clippy-r495`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `.196`, workspace `func011-media-provenance-fmt-r495`:
  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/collab_media.rs`
  passed after normalizing three existing formatter differences in the same
  owned file.

The earlier `.130` and `.170` cold attempts were interrupted during compilation
and are not acceptance evidence. No farm-only source workaround was used.

## Remaining epic acceptance

`WL-FUNC-011` still requires real provider adapters and post-release live call,
reconnect, consent/revocation, migration, office, transfer, package, and
three-seat-maximum release proof. This slice closes only the reachable media
session-provenance and stale-proof cleanup boundary.
