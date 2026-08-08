# WL-FUNC-021 — source-aware playlist mutations (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
live two-catalog playback, provider/network acceptance, target/DLNA proof,
GUI-worker removal, and direct-DRM proof remain open.

## Invariant

The daemon remains the sole Music playlist mutation authority. Existing
playlist update, delete, and reorder requests may use a non-legacy provider
only when the exact playlist variant is retained in the bounded catalog and a
currently admitted client has the same source identity. Legacy/unprojected
playlist rows continue to use the primary writer; arbitrary source identities
fail closed.

## Implementation

- `crates/services/mde-musicd/src/bus_responder.rs` replaces the legacy-only
  playlist guard for update/delete/reorder with source-aware client admission.
- The helper validates playlist kind, checks exact retained catalog identity for
  non-legacy playlists, and resolves the matching admitted client before any
  provider mutation. Create remains on the primary writer because its request
  has no selected playlist source identity.
- The hostile regression gives the selected provider a successful response and
  the non-selected provider a failure response. It proves the selected client
  is used, then submits an unadmitted source identity and proves the mutation is
  refused before provider I/O.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func021-playlist-source-focused-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd --lib \
  bus_responder::tests::typed_playlist_mutation_uses_the_selected_admitted_provider \
  -- --nocapture

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func021-playlist-source-full-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd -- --nocapture
```

- `.90` focused regression: **1 passed, 0 failed**; 159 tests filtered out.
- BigBoy `.130` full daemon suite: **160 passed, 0 failed**; doctests: **0
  passed, 0 failed**.
- `.50` package-scoped `cargo fmt -p mde-musicd -- --check`: passed.
- Local `git diff --check` for the touched responder passed; no unrelated
  formatter rewrite was applied.

## Source hash

```text
9c4904ff899365a8f07230dad8801bd64c47f18e4a1e0908df9dd24950d4ed92  crates/services/mde-musicd/src/bus_responder.rs
```

## Open acceptance

This proves typed source admission and provider selection with fixture-backed
HTTP, not live cross-catalog playlist synchronization or audible playback.
Cross-source playlist creation, live provider outage behavior, downloads
migration, target/DLNA control, GUI-worker removal, and direct-DRM/live-seat
evidence remain required before WL-FUNC-021 closes.
