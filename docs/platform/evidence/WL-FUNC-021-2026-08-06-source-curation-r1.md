# WL-FUNC-021 — source-aware typed curation (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
live two-catalog playback, provider/network acceptance, target/DLNA proof,
GUI-worker removal, and direct-DRM proof remain open.

## Invariant

The daemon remains the sole Music curation authority. Typed `star` and
`unstar` requests may mutate only a retained catalog variant whose source
identity is admitted by the current bounded client set. A configured client
alone cannot authorize an arbitrary source identity; unknown variants fail
closed.

## Implementation

- `crates/services/mde-musicd/src/bus_responder.rs` now resolves non-legacy
  curation requests through the bounded retained catalog and selects the
  admitted client matching the requested `source_id`.
- The legacy/unprojected compatibility path remains on the primary writer.
  Non-legacy requests require exact retained variant admission and return
  stable `unsupported_source` or `source_unavailable` errors before provider
  I/O when the identity is not retained or the matching client is absent.
- The hostile regression uses two admitted providers, proves the selected
  provider receives the typed star mutation, then submits an unadmitted source
  identity and proves it is rejected. Existing legacy star/unstar coverage
  remains in the same responder suite.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func021-source-curation-focused-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd --lib \
  bus_responder::tests::typed_source_curation_uses_the_selected_admitted_provider \
  -- --nocapture

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func021-source-curation-full-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd -- --nocapture
```

- `.90` focused regression: **1 passed, 0 failed**; 158 tests filtered out.
- BigBoy `.130` full daemon suite: **159 passed, 0 failed**; doctests: **0
  passed, 0 failed**.
- `.90` package-scoped `cargo fmt -p mde-musicd -- --check`: passed.
- The local `git diff --check` remained clean for tracked changes; no unrelated
  formatter rewrite was applied.

## Source hash

```text
a89e22e4f7960f299f591f824b316afc6d78294b2fcc0149c89a133fefaee947  crates/services/mde-musicd/src/bus_responder.rs
```

## Open acceptance

This proves typed source admission and provider selection with fixture-backed
HTTP, not live audible playback. Two reachable catalogs, network-loss
playback, downloads migration, target/DLNA control, GUI-worker removal, and
direct-DRM/live-seat evidence remain required before WL-FUNC-021 closes.
