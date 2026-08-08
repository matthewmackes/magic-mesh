# WL-FUNC-021 — live cast discovery refresh coalescing

Date: 2026-08-07

## Finding

`MediaController::discover_cast_targets` synchronously combines the mesh roster
read with the SSDP probe. Repeating the live affordance previously launched the
same blocking discovery again even while the prior target snapshot was still
usable. The SSDP default receive window is two seconds.

## Change

`crates/desktop/mde-media-egui/src/model.rs` now coalesces live-style discovery
within a two-second cooldown. A repeated action keeps the last target snapshot
and reports the cooldown; discovery is allowed again when the interval expires.
The injected `refresh_cast_targets` seam remains immediate for deterministic
tests and callers that already own discovery.

## Verification

Farm command:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=cast-refresh-coalesce-r1 \
install-helpers/xcp-build.sh cargo test --locked -p mde-media-egui --lib \
live_cast_discovery_coalesces_duplicate_refreshes_and_expires -- --nocapture
```

Result: **1 passed, 0 failed, 107 filtered out** on build host `172.20.0.90`.

The focused test proves first discovery runs, an in-window duplicate is skipped
while the prior snapshot remains available, and an expired interval refreshes
to the replacement target.

Live Dell/seat acceptance was not attempted in this scoped change; those hosts
remain unreachable from the current environment.
