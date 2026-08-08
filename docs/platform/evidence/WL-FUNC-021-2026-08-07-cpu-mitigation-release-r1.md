# WL-FUNC-021 — common-seat CPU mitigation release (2026-08-07)

## Diagnosis

The common-mode load was concentrated in `mackesd` and its surrounding mesh
control plane rather than `mde-musicd`/mpv: synchronized five-second runtime
status sampling, repeated NWS no-fix refreshes, and fixed-cadence peer status
writes amplified Syncthing and control-plane churn across seats.

## Implemented mitigations

- Seat snapshot startup now uses a deterministic host phase bounded to 1500 ms.
- Worker runtime status uses stable node phase/retry jitter, coalesces unchanged
  publications, and ignores unregistered rows without dropping valid rows.
- NWS no-fix retries use a one-time host phase and bounded 5/10/20/40/60-second
  backoff while preserving immediate degraded publication.
- Mesh peer status skips writes when only the heartbeat timestamp changed.
- Media registry mirrors skip atomic replacement when the credential-free body
  is unchanged.
- Node-grade and mesh-latency workers use deterministic bounded initial phases
  while preserving their freshness and shutdown semantics.
- Health reconciliation and alert relay startup sweeps also use deterministic
  bounded phases while preserving their existing cadence and shutdown bounds.
- Nebula supervisor startup sweeps use a deterministic bounded phase while
  preserving config repair, leadership, roster, and shutdown semantics.

## Verification

- Seat pump: BigBoy farm 7/7; file-scoped pinned rustfmt passed.
- Runtime status: BigBoy farm 15/15; `.50` library check passed.
- NWS overlay: `.50` farm 16/16; file-scoped rustfmt passed.
- Media registry: `.90` farm 12/12; unchanged registry bodies preserve inode.
- Node grade: BigBoy farm 13/13; mesh latency: `.90` farm 7/7.
- Health reconciler: `.50` farm 13/13; alert relay: `.90` farm 13/13.
- Nebula supervisor: `.90` farm 57/57.
- Boot readiness: BigBoy farm 11/11; service aggregator: `.50` farm 18/18.
- Desktop sources: `.90` farm 44/44; recurring Workload/mDNS scans retain
  immediate publication and heartbeat/refresh semantics.
- Running apps: BigBoy farm 8/8; unchanged replicated documents preserve their
  inode and first `/proc` scans use a bounded hostname phase.
- Firewall monitor: `.90` farm 21/21; media server: BigBoy farm 23/23 with
  unchanged manifests preserving their inode.
- Installed apps: BigBoy farm 5/5; unchanged replicated app catalogs preserve
  their inode and first scans use a bounded hostname phase.
- UPnP discovery: BigBoy farm 6/6; retained-roster pruning now starts on a
  deterministic bounded hostname phase.
- Navidrome supervisor: BigBoy farm 2/2; synchronous systemd probes now start
  on a deterministic bounded hostname phase without moving the first-pass
  deadline.
- Music reconnect: `.50` farm 9/9; zero retry budgets normalize to a one-second
  floor so malformed configuration cannot create a duplicate-request hot loop.
- Music host identity: `.50` farm 20/20 in the state target; `local_host()` now
  caches the immutable hostname instead of spawning `hostname` per caller.
- Media cast discovery: `.90` farm 1/1; live SSDP/mesh discovery refreshes are
  coalesced for the two-second probe window while retaining prior targets.
- Music daemon sweep guard: `.50` farm full mde-musicd suite 187/187; the Bus
  responder now uses a bounded deterministic startup phase and skips unchanged
  idle workspace projection rewrites. Focused Bus-responder coverage includes
  the new phase/dedupe behavior (56/56).
- MPRIS idle guard: `.50` focused MPRIS coverage passed 10/10 and the full
  mde-musicd library suite passed 188/188; the no-op MPRIS lifetime thread now
  waits on shutdown notification instead of waking every 200 ms.
- Music UI poll guard: `.90` full mde-music-egui library coverage passed 55/55;
  retained daemon workspace reads are bounded to a 500 ms cadence and only
  daemon-authoritative surfaces schedule that refresh.
- Live-proof provenance guards: `.50` shell/network helper syntax and
  self-tests passed; `.90` DRM-proof compile/self-test passed. The live-seat
  helper now binds the running MainPID to the expected RPM-owned executable,
  while DRM PNG acceptance binds to its validated metadata sidecar.
- Media retained-state guard: `.90` farm full mde-media-egui suite 109/109;
  identical retained media-source rosters no longer reapply to the controller.
- NWS forecast recovery: `.50` farm 12/12; no-fix retries use a stable host
  phase and bounded backoff while retaining immediate degraded publication.
- Mesh-status helper syntax, farm probe, and diff checks passed.
- Fedora 44 container release-5 full build on BigBoy (current-source r6 lane)
  completed with base and lighthouse payload gates passing. The farm and
  pulled local artifacts match; final SHA-256 hashes are:

```text
magic-mesh-12.1.6-5.x86_64.rpm          dbc7f945fafe58c751e96e9779e2e15c0c38db190425f5645e1302a5de806f3c
magic-mesh-lighthouse-12.1.6-5.x86_64.rpm 9e7162c85a67fe5d173b7ef8467a36a38476ec5415d22ecea86deb257a1097cb
```

The current-source r7 rebuild superseding those earlier hashes passed the same
payload gates. Its farm and pulled-local hashes match:

```text
magic-mesh-12.1.6-5.x86_64.rpm          288d303534dfd41e5ebe1df90871601278d240440a34988ac3dcc0838f0fa2ae
magic-mesh-lighthouse-12.1.6-5.x86_64.rpm fc2f112402b1f0845e6bba4e42e113419fea23d44eb66642613b058c3cf54359
```

This is farm/source/package evidence, not a live result. The current-source r7
authorized SSH retry still found `172.20.0.225` at `No route to host`, the
configured Dell alias `172.20.146.2` also at `No route to host`, while
`10.42.0.4` and `10.42.0.146` timed out; no seat was changed. Installed-seat deployment,
post-change CPU sampling, and five-seat/renderer/handoff acceptance remain
open. Focused source/farm records:
`media-registry-dedupe-r1`, `node-grade-phase-r1`, `mesh-latency-phase-r1`,
`health-reconciler-phase-r1`, `alert-relay-phase-r1`, and
`nebula-supervisor-phase-r1`, `boot-readiness-phase-r1`, and
`service-aggregator-phase-r1`, and `desktop-sources-phase-r1`.
The running-apps record is `apps-running-phase-r1`; the firewall, media, and
installed-app records are `firewall-monitor-phase-r1`,
`media-server-dedupe-r1`, and `apps-installed-phase-r1`.
The authorized read-only seat-15 CPU proof was rerun with a ten-second window
and refused before sampling because the installed package is release 4 rather
than the expected release 5; no stale-package CPU result was accepted.
