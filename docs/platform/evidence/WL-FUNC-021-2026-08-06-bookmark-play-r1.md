# WL-FUNC-021 — typed bookmark audio playback admission (2026-08-06)

## Implemented slice

The daemon-owned typed workspace `play` action now admits finite Episode,
Chapter, and Audiobook content in addition to Music. Bookmark-backed identities
are resolved through the existing retained catalog/source policy: the selected
admitted provider is pinned first, alternate admitted candidates remain behind
the same engine retry boundary, and a verified finite cache is used only when
the provider is unavailable. Unknown source identities fail closed.

The retained workspace projection also preserves a queued bookmark's original
source-qualified `ContentRef` and media kind instead of downgrading it to a
legacy Music identity on the next snapshot.

No second playback worker, provider client, queue, or engine authority was
introduced. The existing typed Workload/Bus boundary and daemon engine remain
the only execution path.

Changed file:

```text
crates/services/mde-musicd/src/bus_responder.rs
```

## Farm verification

All compile/test/format work used explicit farm hosts and isolated slots:

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-bookmark-play-focused-r1 \
  install-helpers/xcp-build.sh \
  cargo test -p mde-musicd \
  typed_play_selection_accepts_an_admitted_bookmark_audio_variant
result: 1 passed, 0 failed; 165 filtered out

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-bookmark-identity-focused-r2 \
  install-helpers/xcp-build.sh \
  cargo test -p mde-musicd workspace_queue_projection_preserves_source_variant_identity
result: 1 passed, 0 failed; 165 filtered out

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-bookmark-identity-full-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 166 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-bookmark-identity-fmt-r1 \
  install-helpers/xcp-build.sh cargo fmt -p mde-musicd -- --check
result: pass
```

The required BigBoy `.130` full retry could not contact the host (`No route to
host` during farm synchronization), so no BigBoy result is claimed. The exact
reachable farm scratch workspaces were removed after completion. Local
`git diff --check` passed.

## Remaining proof

This fixture-backed daemon result does not prove live provider bookmark access,
live podcast/audiobook/chapter decode, authenticated UI action-token issuance,
network-loss playback, Dell/seat acceptance, or release promotion.

## Source hash

```text
28d4f3b0eca7e0137ad533885a65f97b1e3c3496d95604da781260de60384c4c  crates/services/mde-musicd/src/bus_responder.rs
```

Working-tree base revision: `e52322ec` (changes are intentionally
uncommitted).
