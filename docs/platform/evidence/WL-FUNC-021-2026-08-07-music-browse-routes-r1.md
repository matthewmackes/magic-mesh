# WL-FUNC-021 Music browse-route completeness (2026-08-07)

## Finding

The Subsonic/Airsonic provider was not the blocker. Direct Bus probes on Dell
and seat 15 returned 240 artists, 31 podcast channels, and 53 internet-radio
stations from the same Airsonic source. The defect was local integration:

- the Music UI changed its visible Artists/Podcasts/Radio filter without
  publishing a daemon browse request;
- the shell's embedded browse publisher allowed only `search`; and
- `mde-musicd` persisted albums/artists/songs but did not project podcast and
  radio results into the retained typed workspace. Album detail likewise did
  not request `get-album`, so an album could display while its tracks remained
  unavailable.

The canonical Subsonic API specification documents the corresponding
`getArtists`, `getArtist`, `getPodcasts`, and `getInternetRadioStations`
operations: <https://www.subsonic.org/pages/api.jsp>. Airsonic's API guide
directs implementers to that specification.

## Implementation

- `crates/services/mde-musicd/src/airsonic.rs` now deserializes podcast and
  radio provider records.
- `crates/services/mde-musicd/src/bus_responder.rs` persists and projects
  podcast/radio catalog items into typed workspace collections, with a
  regression covering both collection kinds.
- `crates/desktop/mde-music-egui/src/app.rs` publishes list requests from the
  visible library filters and requests `get-album` when opening an album.
  The narrow/embedded view now exposes the same filters as the full rail.
- `crates/desktop/mde-shell-egui/src/main.rs` admits the read-only Music browse
  verbs needed by those UI requests while retaining the typed mutation boundary.

## Farm verification

```text
BigBoy 172.20.0.130 / music-hub-routes-daemon-r1:
  mde-musicd: 195 passed, 0 failed

172.20.0.50 / music-hub-routes-ui-r1:
  mde-music-egui: 60 passed, 0 failed

172.20.0.90 / music-hub-routes-shell-r1:
  mde-shell-egui: 1453 passed, 5 unrelated existing pixel/IaC/switcher
  proof failures (the browse-route change compiled; no route-specific failure)

BigBoy 172.20.0.130 / music-hub-routes-release-r1:
  Fedora 44 RPM payload size gates: PASS
```

The broad `cargo fmt --check` remains affected by pre-existing formatting drift
outside this slice; `git diff --check` passed for the route changes.

## Live deployment and proof

The exact Fedora 44 artifact was installed after a passing `rpm -Uvh --test`
transaction and the mandatory visible seat-update warning on both authorized
targets:

```text
magic-mesh-12.1.6-5.x86_64.rpm
size: 87555378 bytes
SHA-256: 806ce7dace052619478c736bb6d91b3d22d81688b086737338db0c8a2c1b66b7

Dell 172.20.146.225: verify-music-live-seat PASS
Seat 15 172.20.0.15: verify-music-live-seat PASS
```

Both hosts report active `mde-musicd`, `mackesd`, and `mde-shell-egui` units;
Music-daemon `NRestarts` is zero. After the three browse requests, the latest
retained workspace snapshots were:

| Target | Revision | Albums | Artists | Podcasts | Radio | Reachable sources |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Dell | 246 | 100 | 239 | 31 | 53 | 1 |
| Seat 15 | 24 | 100 | 239 | 31 | 53 | 1 |

The provider returned 240 artist rows; the typed projection retained 239 after
normalization/deduplication. No provider API or documentation gap was observed.

## Boundary

This closes the reported empty/unopenable Artist, Podcast, Radio, and album
detail browse paths on Dell and seat 15. It does not claim full artist-detail,
podcast-episode-detail, or radio playback UX, nor the still-open provider-loss,
live renderer, cross-seat handoff, and five-seat CPU/NWS acceptance boundaries
of WL-FUNC-021.
