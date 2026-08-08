# WL-FUNC-021 evidence — Music workspace contract and shell slice

Date: 2026-08-05
Status: implementation slice; epic remains `Remaining`

## Delivered

- Added bounded version-1 Music domain records for composite content identity,
  source variants, catalog shelves, search, playback, queue, downloads,
  targets, capabilities, and authenticated action envelopes.
- Added source-collision-safe catalog helpers, cache/reachability/priority
  variant selection, evidence-backed shelf omission, and stale-search checks.
- Expanded the Subsonic capability/action contract for annotations, podcasts,
  audiobooks, bookmarks, radio, downloads, and queue synchronization.
- Added versioned queue, cache, and playback-state envelopes with legacy read
  migration, and published a retained `state/music/workspace` Bus projection.
- Added fail-closed validation for versioned action/snapshot envelopes,
  composite identities, and bounded retained collections.
- Replaced the shell Music mount with the self-contained responsive workspace
  renderer, shared Music appearance tokens, search debounce, setup/offline/
  loading/error states, library navigation, Now Playing rail, and player shell.

## Farm verification

- `cargo test -p mde-musicd --lib`: 128 passed on BigBoy, including the typed
  workspace ledger, queue-authority, token-redaction, domain, queue, Airsonic,
  engine, MPRIS, cache, reconnect, and state tests.
- `cargo test -p mde-music-egui`: 33 passed.
- `cargo test -p mackes-mesh-types subsonic`: 7 passed.
- `cargo test -p mde-media-core`: 234 passed plus its doctest.
- `cargo test -p mde-media-egui`: 104 passed.
- Jellyfin unit/integration/doctest suite: 107 passed.
- BigBoy real `cargo test -p mde-media-core --features mpv
  --test mpv_fixture_decode`: 1 passed with nonblank pixels and resolved audio.
- BigBoy shell media feature checks passed, and the optimized release shell
  build passed with `drm,live-vdi,media-mpv`.
- `lint-worklist.sh`: 17 active, 17 Remaining, 0 Blocked, 0 Needs clarification.
- `lint-doc-supersession.sh`: clean.
- `git diff --check`: clean.

## Typed daemon authority slice — 2026-08-05

- Added the `action/music/workspace` responder path. It parses the typed
  deny-unknown-fields envelope, authorizes the exact workspace scope, validates bounded
  request fields, enforces the deterministic queue revision precondition, and
  writes only typed `MusicActionResultV1` replies.
- Added an atomic, bounded 1,024-record replay ledger. A request is durably
  reserved before a queue or transport side effect; completed results are
  retained for replay refusal, with control tokens and provider error text
  excluded from the ledger/reply contract.
- Implemented the daemon-owned queue/transport seam for play, pause, resume,
  stop, seek, volume, next/previous, clear, reorder, and remove. Admitted
  playlist, curation, download, transfer, and multi-source actions return
  explicit `unsupported_action` until their provider authorities are migrated.
- Farm verification: BigBoy
  `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-typed-bus-r2
  install-helpers/xcp-build.sh cargo test -p mde-musicd --lib` — 128 passed;
  `.50` `cargo fmt --check --package mde-musicd` — passed; `.90` focused
  domain validation — 1 passed; `.170` focused queue CAS — 1 passed.
- The GUI still owns its legacy Airsonic/engine worker and has not been
  declared migrated. Live two-source aggregation, managed downloads, target
  handoff/DLNA, direct DRM, live-seat proof, and deterministic visual captures
  remain open; this slice does not claim production completion.

## Dell review staging — 2026-08-05

- Review-only bundle: `mm@172.20.146.225:~/magic-mesh-review/2026-08-05-drain-goal/`.
  The installed runtime was not overwritten or rebooted.
- Staged `WORKLIST.md`, both active-epic evidence files, the authority gate
  scripts, `musicd/{airsonic,bus_responder,cache,domain,mpris,queue}.rs`, and the governed
  Workload reply slice under `workload/{workloads,workload_compute}.rs`. The Dell package
  is `magic-mesh-12.1.6-4.x86_64`; `mackesd.service` and
  `mde-shell-egui.service` were active during the review audit.
- Review hashes match the local working tree after the catalog slice:
  `WORKLIST.md`
  `70daa7ed2f7d2bec5eb07e9f4c1dfaa328e3c8c5c9d53513b354673c5992ffed`,
  `musicd/bus_responder.rs`
  `db415ee2eb1ff57b2b2fc462a43a258f700f196d0a95232f6e8ff4a5872c867d`,
  `musicd/cache.rs`
  `6ccdf9e4c7a5215034bd86db1fb8dbff36dd6d1639ba59aec23f5305e776e4a8`,
  `musicd/domain.rs`
  `7967d3178ac513e09478f3a7dbcfd1b21bf6b5cc5457f3adebc797702f2364e0`,
  `musicd/mpris.rs`
  `7581ab1f6ca81a56ff5b658c656870f167c157e13eb3bdcf4c670707ee769df0`, and
  `musicd/airsonic.rs`
  `0d9cc7c6082fb31a567d3e7d1ae6f661a58a8be288e6bffb220d38e1b6aae362`, plus
  `workload/workload_compute.rs`
  `d679863d1f33317d0a8d15158f04d6c33b3718b1480d1234186fb71c401cfd12`.

## Daemon-owned shuffle/repeat policy (2026-08-05)

The typed `action/music/workspace` lane now handles `shuffle` and `repeat`
instead of returning `unsupported_action`. It validates the boolean shuffle
field and the closed `off`/`track`/`context` repeat enum, then writes the same
durable playback-policy file used by MPRIS. Workspace snapshots read that
shared policy, so the Bus and lock-screen/media-key surfaces cannot advertise
different playback modes. Invalid or missing policy fields fail before any
state mutation.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-policy-bus-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd --lib
result: 128 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-policy-domain-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd domain::tests::
result: 5 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-policy-fmt-r2 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: passed
```

The focused hostile coverage proves missing/invalid repeat and missing shuffle
rejection, typed policy mutation, and shared-policy readback. Catalog/source
fan-out, curation, downloads, GUI-worker migration, target handoff, and live
two-source/DLNA/direct-DRM proof remain open.

## Daemon-owned provider stars (2026-08-05)

The typed workspace lane now handles `star` and `unstar` for admitted legacy
Subsonic identities whose content kind is Music, Album, or Artist. The daemon
performs the provider mutation through the shared authenticated client and
returns a stable `source_unavailable`, `curation_failed`, or
`unsupported_source` refusal without echoing provider text. Missing content is
rejected by the versioned request validator before the provider is contacted.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-curation-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd --lib
result: 130 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-curation-focused-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd \
    bus_responder::tests::typed_star_actions_use_admitted_provider_and_refuse_other_sources -- --nocapture
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-curation-fmt-r2 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: passed
```

The tests exercise real one-shot HTTP Subsonic success envelopes for both
mutations and reject an untrusted source identity before a provider call.
Playlists, cross-source catalog fan-out, downloads, GUI-worker migration,
target handoff, and live two-source/DLNA/direct-DRM proof remain open.

## Daemon-owned legacy playlists (2026-08-05)

The same typed `action/music/workspace` lane now admits bounded legacy playlist
create, update, delete, and reorder mutations. Create carries a bounded name and
optional song ids; update carries the playlist `ContentRef`, optional rename,
additions, and removal indexes; delete and reorder carry the admitted playlist
identity. The responder invokes the existing authenticated Airsonic writer
methods, keeps `source_id == legacy` and `ContentKind::Playlist` as the source
boundary, and returns stable `source_unavailable`, `playlist_mutation_failed`,
or `unsupported_source` refusals. The request validator bounds every name/id,
list length, and removal index before any provider call.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-playlist-r2 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd --lib
result: 131 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-playlist-focused-r2 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd \
    typed_playlist_actions_use_the_admitted_provider -- --nocapture
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-playlist-fmt-r2 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: passed
```

The focused test drives real one-shot HTTP success envelopes through create,
update, and delete, and verifies reorder refuses an untrusted source before the
provider is contacted. Native/cross-source playlist aggregation, managed
downloads, GUI-worker migration, target handoff, and live two-source/DLNA/
direct-DRM proof remain open.

## Daemon-owned finite offline downloads (2026-08-05)

The typed workspace lane now downloads finite admitted legacy Music, Episode,
Chapter, and Audiobook identities through the authenticated Subsonic stream
adapter. The daemon rejects radio and untrusted sources, writes bytes with the
existing cache's temp-then-rename/index path, and persists a bounded
`music-downloads-v1.json` record set. `download` records a ready item with byte
count; `cancel_download` records a cancelled item without deleting bytes; and
`remove_download` removes the durable record and cached track. Corrupt or
oversized store envelopes fail closed. The current action is a bounded
synchronous finite transfer, so queued/downloading progress, retries, pinned
eviction/storage reporting, and live source-loss playback evidence remain open.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-download-r2 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd --lib
result: 133 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-download-focused-r1 \
  install-helpers/xcp-build.sh cargo test -p mde-musicd \
    typed_download_lifecycle_writes_and_removes_durable_record -- --nocapture
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-download-fmt-r2 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: passed
```

The focused test uses a real HTTP byte response, verifies cache/index contents,
round-trips the durable record through cancellation, removes both record and
bytes, and rejects a Radio identity before provider work.

Cross-package regression verification after the daemon contract expansion also
passed on the farm: `cargo test -p mde-music-egui --lib` returned 33/33, and
`cargo test -p mde-media-core --lib --tests` returned 234/234 plus the empty
fixture-test harness. The previously captured Media Player mpv-feature fixture
and optimized shell checks remain the live-engine evidence; no new hardware
claim is made by this contract-only regression.

## Daemon-owned catalog projection (2026-08-05)

Successful admitted browse replies now update an atomic, bounded
`music-catalog-v1.json` store owned by `mde-musicd`. Album, artist, song, starred,
recent, frequent, and search replies are normalized into source-identified
`CatalogItem` variants; the retained workspace snapshot projects the observed
source capabilities, shelves, collections, and stale-safe search page. Search
requests accept both the typed JSON query envelope and the legacy bare-string
shape, with control characters and query length bounded at the boundary.

The initial slice covered one configured source. The follow-on below adds a
bounded read-only source fan-out while leaving OpenSubsonic discovery,
cross-source playback selection, GUI-worker migration, target handoff/DLNA,
and direct-DRM proof open.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-catalog-r11-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd --lib
result: 134 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-catalog-r10-fmt \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: passed
```

Canonical governance gates on `.170` slot `music-doc-gates-r2` also passed:
`lint-worklist.sh --self-test`, `lint-worklist.sh` (17 active, 17 Remaining),
`lint-doc-supersession.sh`, and `lint-workload-authority.sh`.

## Open acceptance evidence

The daemon is not yet the sole catalog/action authority, managed downloads,
multi-source live aggregation, target handoff, direct-worker removal, live
two-catalog browse/playback, deterministic render captures, and DLNA proof are
still required. The full shell suite currently has 12 unrelated Car,
front-door, IaC, and surface-count failures, while the Music/Media-specific
feature checks pass. The style-leak gate still reports five unrelated raw
hover-text uses in shell surfaces. No production-complete claim is made from
this slice.

## Bounded multi-source catalog fan-out — 2026-08-05

`mde-musicd` now loads the legacy primary credential followed by an optional
`airsonic-sources.json` envelope (or `MDE_AIRSONIC_SOURCES` override), bounded
to four deduplicated URL/user pairs. The primary client remains the sole
playlist/transport mutation writer. Read-only catalog/search verbs fan out to
the configured clients, add a source identity to returned rows, merge bounded
arrays, and record each successful reply into the same atomic catalog store.
Legacy catalog files with singular `source` state migrate in memory to plural
`sources`; normalized `CatalogItem` identities retain each source variant, and
workspace snapshots expose plural capabilities without exposing credentials.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-multisource-r2-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 136 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=music-multisource-format-r1 \
  install-helpers/xcp-build.sh sync
ssh mm@172.20.0.90 'rustfmt --edition 2021 --check \
  crates/services/mde-musicd/src/creds.rs \
  crates/services/mde-musicd/src/bus_responder.rs'
result: passed
```

The new tests cover primary-first bounded credential loading and duplicate
rejection plus source-tagged multi-source merge behavior. A strict all-targets
clippy attempt was not clean because the pre-existing `mde-musicd/build.rs`
contains five `clippy::doc_markdown` errors; a narrowed follow-up also reaches
pre-existing mde-bus and daemon-wide lint debt. No unrelated lint debt was
silently reclassified as this slice's result. Live two-catalog reachability,
live source-loss playback acceptance, runtime source-health failover, download
progress/eviction, GUI-worker removal, target/DLNA, deterministic renders, and
direct-DRM acceptance remain open.

## Source-loss offline queue playback fallback — 2026-08-05

The daemon now has a conservative offline play path for an Airsonic outage.
Before starting playback without a live client, `mde-musicd` probes every song
in the queued tail through the durable cache index and verifies the recorded
finite file is present and non-empty. A partial tail is refused, so the daemon
does not claim playback and fail halfway through an album. For a complete tail,
it preserves each cached suffix as a `SourceCodec` hint, creates an internal
`mde-cache:///stream?id=...` source URL, and routes those entries directly into
the existing decoder; the custom scheme is never passed to reqwest. The normal
Airsonic path and its per-track fetch-failure cache fallback remain unchanged.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-offline-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd offline -- --nocapture
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-offline-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd cached -- --nocapture
result: 5 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-offline-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 138 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-offline-r1-big \
  rustfmt --edition 2021 --check \
  crates/services/mde-musicd/src/cache.rs \
  crates/services/mde-musicd/src/engine.rs \
  crates/services/mde-musicd/src/bus_responder.rs
result: passed
```

The farm suite proves the cache validity probe, opaque song-id URL round trip,
complete-versus-partial queue admission, and the existing Music regression
surface. It does not claim live gateway/network-loss playback, runtime
source-health failover, target handoff/DLNA, GUI-worker removal, deterministic
renders, or direct-DRM acceptance.

## Source-aware playback variant selection — 2026-08-05

The transport lane now receives the full bounded set of admitted Airsonic
clients instead of only the primary. For each queued song, it looks up the
retained source variants, applies the domain selector's cache/reachability/
operator-priority/latency ordering, and resolves the selected source identity
back to the matching live client before constructing the stream URL. Legacy or
not-yet-projected queue entries retain a deterministic primary-client fallback;
no credentials or provider payloads enter the workspace contract.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-source-aware-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd source_aware -- --nocapture
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-source-aware-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 139 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-source-aware-r1-big \
  rustfmt --edition 2021 --check \
  crates/services/mde-musicd/src/bus_responder.rs
result: passed
```

The focused test proves a retained higher-priority second-source variant is
resolved to that source's live client. Live source retry against another
admitted variant, two-catalog playback, target handoff/DLNA, GUI-worker
removal, deterministic renders, and direct-DRM acceptance remain open.

## Source-health projection for variant failover — 2026-08-05

Read-only browse probes now update retained source truth. A malformed or
provider-error reply marks the matching source capability and every retained
catalog/search variant for that source unreachable; a later successful probe
restores those flags. The selector can therefore exclude a source known to be
down while retaining the explicit primary fallback for legacy queues with no
projected variant.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-source-health-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd catalog_source_health -- --nocapture
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-source-health-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 141 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-source-health-r1-big \
  rustfmt --edition 2021 --check \
  crates/services/mde-musicd/src/bus_responder.rs
result: passed
```

The focused test covers both the catalog collection and retained search view.
Live source retry against a failed selected stream, two-catalog playback,
target handoff/DLNA, GUI-worker removal, deterministic renders, and direct-DRM
acceptance remain open.

## Download lifecycle truth — 2026-08-05

Managed Music downloads now publish a durable `downloading` record before the
provider request. Empty responses, provider failures, and cache-write failures
replace it with a bounded redacted `failed` record; successful finite bytes are
written atomically and then replace the record with `ready`. Existing
cancel/remove behavior remains intact, and provider credentials are never
serialized into the record or its error code.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-download-state-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd download_empty -- --nocapture
result: 1 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-download-state-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 140 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-download-state-r1-big \
  rustfmt --edition 2021 --check \
  crates/services/mde-musicd/src/bus_responder.rs
result: passed
```

The empty-response test proves the failed-state transition, durable record,
and secret redaction. Streaming byte progress, storage usage/pinned eviction,
cross-source downloads, live network-loss playback, target handoff/DLNA,
GUI-worker removal, deterministic renders, and direct-DRM acceptance remain
open.

## Review refresh after offline playback slice — 2026-08-05

The review-only Dell bundle was refreshed again without replacing the installed
runtime or rebooting the seat. The non-self payload hashes in the latest bundle
match the local working tree; the evidence file's own hash is intentionally
recorded outside its contents during the handoff to avoid a self-referential
digest. Dell still reports the installed `magic-mesh-12.1.6-4.x86_64` package
with `mackesd.service` and `mde-shell-egui.service` active. No live-seat or
production-complete claim is made.

## Streamed download progress and cache reconciliation — 2026-08-05

The daemon-owned finite download path now consumes `reqwest` response chunks
through a bounded 512 MiB body collector. It reports received/expected bytes
to the durable `downloading` record at a bounded 256 KiB cadence, always emits
the final progress point, and keeps the existing temp-then-rename cache write
as the only `ready` transition. Oversized responses fail before cache
population; provider credentials and response text remain outside the record.

The same action lane now exposes typed `pin_download` and `unpin_download`
mutations. Pin state survives retries, is mirrored to the cache index's
starred bit, and post-write LRU GC reconciles evicted ready records to a
redacted `cache_evicted` failure rather than leaving a phantom offline item.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-download-storage-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 142 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-download-storage-format-r1 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: passed
```

The focused lifecycle test covers durable pin/unpin plus cache-index
convergence. Storage usage presentation, migration of older download records,
live network-loss playback, target handoff/DLNA, GUI-worker removal,
deterministic renders, and direct-DRM acceptance remain open.

## Pre-decode source fallback — 2026-08-05

The retained source policy now returns every reachable or cached variant in
deterministic cache/reachability/operator-priority/latency order. The transport
lane groups those candidates per logical queue item and the decoder tries the
next admitted URL only when the preceding source fails before producing audio;
the queue cursor and gapless boundary map therefore remain one-to-one with
the logical queue. Legacy queues still use the primary client fallback.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-source-fallback-r2-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 143 passed, 0 failed; doctests: 0 passed, 0 failed
```

This closes the pre-decode retry seam; live two-catalog outage playback,
mid-stream retry, target handoff/DLNA, GUI-worker removal, deterministic
renders, and direct-DRM acceptance remain open.

## Honest local target projection — 2026-08-05

The workspace snapshot now projects one `local_seat:<host>` target only when
the daemon has constructed its native audio engine. This makes the local
target actionable evidence rather than a configured-but-unproven renderer;
remote mesh seats and DLNA targets remain absent until their typed discovery
and ownership adapters provide reachability and handoff proof.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-local-target-r2-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 144 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-local-target-format-r2 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: passed
```

The typed handoff action, live two-catalog outage playback, mid-stream retry,
DLNA/mesh target discovery, GUI-worker removal, deterministic renders, and
direct-DRM acceptance remain open.

## Typed cache storage projection — 2026-08-05

The retained workspace snapshot now includes daemon-owned indexed cache usage
and the active byte cap used by post-write GC. The projection is derived from
the cache index rather than from provider-reported download rows, and snapshot
validation rejects a zero cap so a surface cannot present an unbounded storage
policy as usable state. The UI still needs to render this projection in its
storage/download view.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-storage-contract-r1-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 144 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-storage-contract-format-r1 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: passed
```

Storage UI presentation, migration of older download records, live network-loss
playback, typed target handoff/DLNA, GUI-worker removal, deterministic renders,
and direct-DRM acceptance remain open.

## Daemon snapshot storage view — 2026-08-05

The Music UI now reads the latest validated `state/music/workspace` snapshot
through the canonical client Bus root. The Downloaded library presents the
daemon's indexed cache usage and cap, bounded lifecycle records, byte progress,
pin state, and redacted failure codes. The reader is fail-soft and monotonic;
it never writes provider, queue, or playback state and therefore does not create
a second authority. Missing Bus state remains an honest waiting state.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-storage-ui-r3-big \
  install-helpers/xcp-build.sh cargo test -p mde-music-egui
result: 35 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-storage-ui-format-r3 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-music-egui
result: passed
```

Versioned download-record migration, typed download mutation controls in this
surface, live network-loss playback, target handoff/DLNA, GUI-worker removal,
deterministic renders, and direct-DRM acceptance remain open.

## MPRIS finite seek integration — 2026-08-05

MPRIS `Seek` now converts its signed microsecond offset into a saturating
millisecond target, while `SetPosition` accepts only a non-negative absolute
microsecond position for the currently loaded queue track. Both methods use the
shared engine's finite-source/seekable gate; rejected live streams, mismatched
track paths, and negative positions have no effect. An MPRIS `Seeked` signal is
emitted only after the engine accepts the seek, keeping external controls
aligned with the daemon-owned playback authority.

Farm verification:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=music-mpris-seek-r5-big \
  install-helpers/xcp-build.sh cargo test -p mde-musicd
result: 145 passed, 0 failed; doctests: 0 passed, 0 failed

MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=music-mpris-seek-format-r5 \
  install-helpers/xcp-build.sh cargo fmt --check -p mde-musicd
result: passed
```

The MPRIS conversion tests cover signed offset saturation, microsecond-to-
millisecond conversion, and rejection of negative absolute positions. Live
network-loss playback, target handoff/DLNA, GUI-worker removal, download
migration, deterministic renders, and direct-DRM acceptance remain open.

## Review hash manifest — 2026-08-05

The review-only Dell bundle was refreshed after this slice without replacing
the installed runtime or rebooting the seat. Current non-self payload hashes
for the Music/worklist review are:

```text
WORKLIST.md                                      5b52af8b914299a2f8b766d917013fb881209697ead90beaee27f5f313896e81
musicd/Cargo.toml                                68d3abff0270f8c3a92b7d3cad3199dd5bdc74db46af93d0c25a8307a3edee94
musicd/airsonic.rs                               acabf124745c327c19c30390f8316d3805170f7104c3588f5bc1b2411dc18eac
musicd/bus_responder.rs                          eb7a0a1d53ff8ca1421fd08005aff3c1254c542b5b767cf24a02c1a2f19f0837
musicd/cache.rs                                   ef8655d1abe56f2f4d0ae155e1ae10a8fb5e366c256dc20275e35e536fa29e2a
musicd/creds.rs                                  58d9e216be9a8d64eedbdc6201befd3838d90cd61f5c2a3c41cfeb8d432953bd
musicd/domain.rs                                 b301f5768e7c2019cfefd789b139d6f52981b85f0f90138976ca20919c561b6a
musicd/engine.rs                                 e57faef6f86f8e48d82939cdc7874e70b7da7d2e516bd64b8e08d47e759aa95d
musicd/mpris.rs                                  e80cb8369efd9b8fc2f5f9d98dba6c8d23e16180aad2080b5ea2345bfdb9bed3
musicd/queue.rs                                  f42fa4feb0d6f7a1fd3e6d4a4e724a5eed4fc2dd58480cb9691096e42f2afc4e
music-egui/Cargo.toml                            885cf7e5ee244cc13f432d495a9a27b1e5c1ccae5dc3779f3d8180cf609f7fe9
music-egui/app.rs                                d88f89f33f71c7b3adf471baef03de43dfdc243fba3ad0a2ff00129be55b282f
music-egui/lib.rs                                24b7f90c9a170b01efd16776fb9444d88a7ba35d6a4126212038aeeccc402c7c
music-egui/model.rs                              1417d6c82c310e65cfcf9755607e737dae2de1c31714ca7603d671159ba40d03
music-egui/workspace_reader.rs                   615b6091994025b8bc38645b9d67337b2de94fe58ba457fbd95dcce753f6000a
mesh-types/workloads.rs                          b7644047a1ba03c0bcb5028167bc38375097e28996d29f1cf4642b2b2a6e1b9c
shell/workload_api.rs                            904e7389f226d847ff3c65974a7282e82dc9892bf8f24d3ba4924f8dbab3eedc
```

The canonical Worklist self-test and lint passed, with 17 active Remaining
epics; document supersession and Workload-authority lint also passed. The
exact touched Music files passed farm rustfmt. BigBoy's current daemon test
result is 145/145. The strict all-targets clippy attempt remains an evidence
gap because pre-existing build-script and dependency lint errors fail before a
clean package-wide result; no production or live-seat completion is claimed.
