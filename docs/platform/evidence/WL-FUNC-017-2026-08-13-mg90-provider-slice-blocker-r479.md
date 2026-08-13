# WL-FUNC-017 MG90 provider slice blocker — 2026-08-13

- Scope: bounded review of `crates/mesh/mackesd/src/workers/vehicle.rs` for one
  safe offline/provider implementation slice, with no changes to unrelated
  worktree files.
- Result: **no safe code gap in the owned provider module**.

## Existing bounded behavior

The provider already has explicit offline and fault behavior:

- an unconfigured local probe returns `ProbeUnavailable` and does not create an
  offline telemetry snapshot;
- a missing or mismatched MG90 identity is rejected before roster admission;
- manager loss clears that manager's retained row and removes source
  publication when no other manager remains;
- expired declared heartbeats remove retained manager rows;
- a failed WAN refresh retains diagnostic metrics only as stale data and
  revokes every active-path claim; it never republishes the retained link as
  live;
- no accepted source snapshot is represented as `NoSource`, not as fabricated
  offline telemetry.

## Exact blocker

The remaining S7 acceptance is live approved-manager/provider integration:
provider credentials/configuration, real MG90 hardware or a recorded hardware
fixture, and the manager registration/reconnect trace. `vehicle.rs` exposes
typed probe and roster seams but has no additional verified MG90 protocol,
credential authority, or provider fixture from which to implement that slice.
Adding a guessed endpoint, credential flow, or synthetic radio response would
violate the epic's requirement that no provider claims success without a live
response.

## Existing evidence

- `docs/platform/evidence/WL-FUNC-017-2026-08-11-mg90-source-generation-r470.md`
  — source-generation identity binding passed on `.90`.
- `docs/platform/evidence/WL-FUNC-017-2026-08-11-mg90-radio-stale-r304.md`
  — failed radio refresh passed on `.50`.
- `docs/platform/evidence/WL-FUNC-017-2026-08-09-mg90-roster-runtime-r5.md`
  — approved roster selection and loss handling passed on `.90`.

No cargo gate was run for this evidence-only blocker record because no source
code changed. Existing unrelated dirty files were preserved.
