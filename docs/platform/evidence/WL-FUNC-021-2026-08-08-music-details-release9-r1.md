# WL-FUNC-021 — Music detail request state and release 9 rollout (2026-08-08)

## Outcome

Artist, Album, and Podcast detail views no longer interpret the retained
snapshot that predates a just-published provider request as a final empty
response. The Music UI now retains an identity-bound in-flight marker for the
selected row, renders a bounded loading state while the daemon replaces the
snapshot, clears that marker only for the matching response, and falls back to
the honest unavailable/empty state after eight seconds.

The Airsonic data was not missing. Live Dell requests returned 38 Special,
AC/DC, Black Ice, and podcasts before the UI fix. The visible failures were an
asynchronous frontend-state bug: the detail route rendered once between request
publication and the next 500 ms retained-workspace poll and called that stale
snapshot definitive.

## Farm gates

- BigBoy `.130`, slot `music-detail-test2`: all 68 `mde-music-egui` library
  tests passed. New render regressions cover the in-flight Artist, Album, and
  Podcast states, the matching Artist response, and bounded Album timeout.
- BigBoy `.130`, focused `daemon_` lane: 21/21 passed.
- `.50`, slot `music-detail-fmt`: targeted `mde-music-egui` format gate passed.
- `.90`, slot `music-detail-check`: the complete `mde-shell-egui` DRM feature
  integration check passed. Existing warnings outside this slice remain; no
  new compilation failure was accepted.

## Fedora 44 package and rollout

- Native F44 BigBoy builder `.131` produced
  `magic-mesh-12.1.6-9.x86_64.rpm`, 87,607,016 bytes, SHA-256
  `1f028fe719fb405b69d8d032439d1d3e3ad70c7a471585be46f5fab2a1a64579`.
- Payload and size gates passed. Required media sonames are F44-native:
  `libavcodec.so.62`, `libswresample.so.6`, `libswscale.so.9`, and
  `libmpv.so.2`.
- T480, Eagle, seat 15, Dell, and Surface independently matched that hash and
  passed `rpm -Uvh --test --nosignature` before installation. All five now
  report `magic-mesh-12.1.6-9.x86_64`.
- RPM's transient post-install user-service requests lost their D-Bus
  connection while package-owned processes were being replaced. The live
  provenance gate caught both `mde-musicd` and `mde-shell-egui` still mapped to
  deleted release-8 executable inodes. Both services were explicitly restarted
  on all five seats. Every seat now executes package-owned
  `/usr/bin/mde-musicd` and `/usr/bin/mde-shell-egui` from release 9 with zero
  service restarts. Dell's `browser-vm` domain remains defined and shut off;
  the deployment did not delete, redefine, or start it.
- The F44 builder was halted after the cut and normal BigBoy `.130` capacity
  was restored. Temporary seat RPM copies were removed after verification.

## Live named-detail proof on Dell

After the release-9 daemon restart, bounded typed Bus requests proved:

- Artist `38 Special` (`id=1`) returns 9 albums.
- Artist `AC/DC` returns 23 albums; `Black Ice` (`id=11`) is present.
- `get-album id=11` returns 15 Black Ice tracks; the first is
  `Rock 'n' Roll Train`.
- The podcast catalog returns 31 feeds. `Wait Wait... Don't Tell Me!`
  (`id=0`) returns 3 episodes.

The reusable live-seat gate then passed on Dell and seat 15: release-9 runtime
ownership, zero `mde-musicd` restarts, provider ping, typed state and album
replies, installed payload, and `rpm -V` integrity.

## Honest remaining boundary

- The named provider records and release-9 UI state transitions are proven, but
  this record does not claim a photographed human click-through of each Dell
  detail page.
- Live provider-loss continuity, physical DLNA/Chromecast rendering, cross-seat
  owner handoff, T480/Eagle/Surface mutating playback, human speaker judgment,
  and synchronized five-seat CPU/NWS recovery remain open under WL-FUNC-021.
