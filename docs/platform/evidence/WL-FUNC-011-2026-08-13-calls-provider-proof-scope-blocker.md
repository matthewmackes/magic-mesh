# WL-FUNC-011 Calls provider-proof scope blocker

Date: 2026-08-13

## Result

No safe substantive implementation slice exists in the authorized Calls
ownership for provider readiness or live-proof attribution.

The owned `mde-collab-core` projection already does the signed-state portion:

- `Projection::call_media_readiness` emits only non-ended calls where the local
  actor is signed `Connected`;
- readiness is bounded by session and participant limits;
- `candidate_adapters` is explicitly a non-selected candidate list;
- `CallMediaAdmission::AdapterReady` means only that a connected remote peer is
  present, not that transport, provider health, or advancing media exists.

The remaining executable authority is the provider/verifier registry and proof
consumer in `crates/mesh/mackesd/src/workers/collab_media.rs`. That module
maps missing transport/provider registrations to typed unavailable outcomes and
can publish `CallMediaVerificationStatus` rows after a concrete verifier
reports advancing frames/data. It is not under the authorized scope of
`crates/services/mde-collab-core` or `crates/mesh/mackesd/src/workers/collab/calls*`.

Changing the core projection would either duplicate the daemon provider
authority or incorrectly turn signed call state into a live-provider claim.
Both would violate WL-FUNC-011 S4's requirement that provider availability and
failure remain visible and auditable.

## Remaining acceptance

Provider registration, provider failure attribution, advancing-frame proof, and
the live call fixture remain open. They require the daemon Calls/media worker
scope and external provider/live-call access; no farm cargo gate was run because
no source code was changed.
