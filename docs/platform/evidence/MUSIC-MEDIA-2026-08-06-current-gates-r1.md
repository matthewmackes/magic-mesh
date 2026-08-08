# Music and Media Player drain evidence — 2026-08-06

This evidence records the current farm verification for the active Music
Workspace / Media Player drain goal. It is evidence for the canonical active
epics, not a second worklist.

## Media/Jellyfin persisted-store integration

`mde-media-egui` now loads the persisted Jellyfin `ServerStore` during real
media construction, refreshes it during the controller pump, selects the first
valid saved profile when the selected profile is absent, and surfaces reload
errors through the existing honest UI status path. Missing persisted state is
normal and does not fabricate a server or profile.

Focused farm verification:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=media-current-ui-jellyfin-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-media-egui
result: 104 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=media-current-jellyfin-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-jellyfin
result: 108 passed, 0 failed across unit, browse, playback, and doctest lanes
```

Live Jellyfin credentials/server playback, mpv/frame/audio proof, target
handoff/casting, and live-seat acceptance remain open. No provider or live
hardware success is inferred from these fixture-backed tests.

## Jellyfin token-store durability

Jellyfin bearer-token saves now write a same-directory temporary file with
owner-only permissions, sync its contents, atomically replace the destination,
and sync the parent directory. A failed write removes only the temporary file;
the prior complete store remains in place. The regression test confirms an
existing store is replaced and no temporary credential file is left behind.

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=media-store-atomic-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-jellyfin
result: 86 unit + 12 browse + 9 playback + 1 doctest = 108 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=media-store-atomic-fmt-r1 \
  install-helpers/xcp-build.sh sync
ssh mm@172.20.0.90 'cd /home/mm/magic-mesh-farm-media-store-atomic-fmt-r1 && \
  rustfmt --check --edition 2021 crates/desktop/mde-jellyfin/src/store.rs'
result: pass
```

## Music workspace retained-state surface

The daemon-owned Music workspace retains bounded source-aware catalog/search,
finite download lifecycle, cache usage/cap, pin state, source fallback, local
target, and finite MPRIS seek state. The UI reads the validated
`state/music/workspace` projection and renders the Downloaded view without
owning provider, queue, or playback state.

The current latest Music farm result is 146 daemon tests passed, with 35 Music
UI tests passed after the daemon persistence change. Remaining obligations are
live network-loss playback, target/DLNA handoff, download migration,
GUI-worker removal, deterministic render/live proof, and direct-DRM evidence.

## Music retained-state atomic persistence (2026-08-06)

The Music daemon's workspace-action ledger, download lifecycle store, and
source-aware catalog store now share one same-directory atomic JSON writer. It
uses a collision-resistant per-process temporary name, syncs file contents
before replacement, syncs the parent directory after rename, removes a
temporary file if the write fails, and creates new retained files as `0600` on
Unix. This keeps the daemon as the sole retained-state authority and prevents
partial action/download/catalog records across a crash or overlapping restart.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-json-atomic-r1 \
  CARGO_INCREMENTAL=0 install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 146 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-json-atomic-fmt-r2 \
  install-helpers/xcp-build.sh cargo fmt -p mde-musicd -- --check
result: pass
```

The regression replaces the retained ledger twice, confirms that only the
canonical file remains, and checks owner-only permissions on Unix. This does
not claim live two-catalog playback, provider failover, target/DLNA handoff,
or GUI-worker/direct-DRM migration.

## Music UI integration after daemon persistence (2026-08-06)

The UI crate still compiles and folds the daemon's validated workspace
projection after the persistence authority change; its retained download view,
read-only workspace reader, and existing honest setup/error states remain
intact.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-ui-atomic-r1 \
  CARGO_INCREMENTAL=0 install-helpers/xcp-build.sh cargo test -p mde-music-egui
result: 35 passed, 0 failed; doctests: 0 passed, 0 failed
```

## Music workspace revision recovery (2026-08-06)

The retained `state/music/workspace` projection now uses a durable monotonic
revision record. The daemon persists the next revision before publishing its
snapshot, so a restart cannot reset the UI's stale-result guard or reuse a
revision that was already visible before a crash. A corrupt revision record
fails closed and disables snapshot publication rather than publishing a reset
revision.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-workspace-revision-r2 \
  CARGO_INCREMENTAL=0 install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 148 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-workspace-revision-fmt-r2 \
  install-helpers/xcp-build.sh cargo fmt -p mde-musicd -- --check
result: pass
```

The focused tests cover first start, persisted revision reload, monotonic
advance after a simulated restart, and corrupt-record refusal. Live provider,
audio, target/DLNA, GUI-worker, direct-DRM, and Dell/seat-15 proof remain open.

## Media source-roster bounded refresh (2026-08-06)

`mde-media-egui` now reads the retained `state/media/sources` roster through
the Bus `read_latest` path instead of loading the full topic history on every
refresh. The reader still treats a missing Bus as empty and malformed JSON as
an honest visible error; the regression writes two records and confirms the
newest one is selected.

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=media-roster-latest-r1 \
  CARGO_INCREMENTAL=0 install-helpers/xcp-build.sh cargo test -p mde-media-egui
result: 104 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=media-roster-latest-r1 \
  cargo fmt -p mde-media-egui -- --check
result: pass
```

This bounds roster refresh cost but does not claim live Jellyfin credentials,
mpv/frame/audio, casting, or live-seat acceptance.

## Music source-aware workspace queue projection (2026-08-06)

The retained Music workspace now resolves queued and current song identities
through the catalog's admitted source variants using the same deterministic
cache/reachability ordering as transport. A queue row falls back to the
explicit `legacy` source only when no matching catalog variant is retained;
the projection no longer hides a known source behind a legacy identity.

```text
MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-workspace-source-r1 \
  CARGO_INCREMENTAL=0 install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 149 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-workspace-source-fmt-r1 \
  install-helpers/xcp-build.sh cargo fmt -p mde-musicd -- --check
result: pass
```

The focused regression covers an admitted source variant and the explicit
legacy fallback. Live two-catalog playback/failover and GUI-worker migration
remain open.
