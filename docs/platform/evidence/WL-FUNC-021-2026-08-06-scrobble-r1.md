# WL-FUNC-021 — typed source-aware playback progress (2026-08-06)

Status: implementation slice complete; WL-FUNC-021 remains `Remaining` because
automatic final progress writes on every pause/stop/transfer/close, live
provider acceptance, target/DLNA proof, GUI-worker removal, and direct-DRM
proof remain open.

## Invariant

Playback progress is a bounded typed daemon mutation. The provider mutation is
made only through the retained selected source variant and its matching
admitted client; arbitrary source identities and unsupported content kinds fail
closed. Progress is never stored in the UI or sent as a raw provider command.

## Implementation

- `crates/services/mde-musicd/src/domain.rs` admits the versioned `scrobble`
  workspace action and requires both a content identity and finite `position_ms`.
- `crates/services/mde-musicd/src/airsonic.rs` adds the bounded Subsonic
  `scrobble` endpoint call with the daemon's millisecond position.
- `crates/services/mde-musicd/src/bus_responder.rs` resolves Music, Episode,
  Chapter, and Audiobook progress through the retained catalog and matching
  admitted provider client, preserving legacy primary-writer compatibility.
- The hostile regression gives the non-selected provider a failure response and
  the selected provider success, then submits an unadmitted source identity and
  proves refusal before provider I/O.

## Farm verification

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func021-scrobble-focused-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd --lib \
  bus_responder::tests::typed_scrobble_uses_the_selected_admitted_provider \
  -- --nocapture

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func021-scrobble-full-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd -- --nocapture
```

- `.90` focused regression: **1 passed, 0 failed**; 160 tests filtered out.
- BigBoy `.130` full daemon suite: **161 passed, 0 failed**; doctests: **0
  passed, 0 failed**.
- `.50` package-scoped `cargo fmt -p mde-musicd -- --check`: passed.
- Local `git diff --check` for the touched Music files passed; no unrelated
  formatter rewrite was applied.

## Source hashes

```text
af67bd5aa618fa23f1b214c04bb1822900a0f64fa2fe5c8937f45f136cd68b76  crates/services/mde-musicd/src/bus_responder.rs
fa748f5293fdd9f6a23ca8bda53c3af83aa5a117fcca47627165d928c48b7f29  crates/services/mde-musicd/src/airsonic.rs
0ab23ed9e3f1e56dc1fcf5e3f7c08892fb1ae5b7e8edaf000403d18608d63053  crates/services/mde-musicd/src/domain.rs
```

## Open acceptance

This proves an explicit typed progress write with fixture-backed provider
selection, not automatic progress finalization on every transport/close event,
cross-catalog provider outage behavior, audible playback, or live seat proof.
Those remain required before WL-FUNC-021 closes.
