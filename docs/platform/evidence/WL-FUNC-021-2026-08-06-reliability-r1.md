# WL-FUNC-021 — Music/Media reliability and common-seat CPU slice (2026-08-06)

This evidence bundle records the current source-side reliability work and the
operator-authorized Dell release-5 verification. It does not claim physical
second-seat handoff or hardware renderer proof.

## CPU-spike mitigation

- The common-seat investigation identified synchronized failed MG90/NWS and
  runtime-status retry paths in `mackesd`, with Music playback idle or near
  idle. Source mitigations now cache failed audio probes and use bounded
  5/10/20/40/60-second retry ladders for failed external probes and rejected
  runtime-status samples.
- BigBoy focused worker gates passed: NWS 15/15, node-grade 10/10, airspace
  13/13, and vehicle 58/58. The full serial `mackesd` lane reached 4386 passed,
  one unrelated pre-existing cloud assertion failure, and one ignored test.
- Farm `.90`, slot `cpu-status-retry-regression-r1`, passed the new retry
  ladder regression 1/1. Farm `.50`, slot `cpu-status-bin-check-r1`, passed
  `cargo check -p mackesd --bin mackesd --features async-services --locked`.
- The Media registry now backs off a failed local Navidrome probe at
  30/60/120/240/300 seconds while retaining the honest `down` registration;
  the focused farm regression passed 1/1 on `.50`.
- Boot readiness now moves synchronous probes to `spawn_blocking`, caches the
  last honest result, and independently backs off failed fabric, ping, and
  service groups at 4/8/16/32/60 seconds; the focused BigBoy library gate
  passed 10/10 after the first small-node lane hit ENOSPC.
- `verify-music-cpu-proof.sh` now provides the bounded post-install gate: it
  binds samples to the exact RPM identity, observes `/proc` tick deltas without
  mutation, and refuses honestly on an old package. After release-5 install and
  daemon restart on Dell, the 30-second proof passed at max `437‰`, mean
  `218‰`, restarts `0→0`.
- Boot readiness now rechecks healthy blocking probe groups after 10 seconds
  while continuing to publish the cached honest snapshot every 2 seconds; the
  focused farm gate passed 10/10.
- Firewall monitoring now removes a redundant journal probe, moves its
  synchronous pass into a blocking section, backs off quiet/failed passes to 60
  seconds, and limits retention rewrites to hourly. Its focused farm gate
  passed 20/20.
- Remmina peer discovery now caches unavailable peers with a bounded
  60/120/240/480/900-second retry ladder and uses delayed missed-tick handling;
  the focused farm gate passed 12/12.

Detailed records: `WL-FUNC-021-2026-08-06-seat-cpu-spikes-r1.md`,
`WL-FUNC-021-2026-08-06-media-registry-cpu-r1.md`, and
`WL-FUNC-021-2026-08-06-boot-readiness-cpu-r1.md`,
`WL-FUNC-021-2026-08-06-firewall-cpu-audit-r1.md`,
`WL-FUNC-021-2026-08-06-remmina-cpu-audit-r1.md`, and
`WL-FUNC-021-2026-08-06-cpu-proof-tool-r1.md`.

## Music/Media recovery

- `mde-media-core` roaming now validates replicated records, releases stale or
  replaced leases, cancels obsolete pending resumes, and retains a failed
  target seek for retry while keeping the target paused. The hostile failed
  seek fixture passed 1/1 on BigBoy.
- The full BigBoy `mde-media-core --features mpv --locked` gate passed 253 unit
  tests, the mpv fixture 1/1, and the doctest 1/1.
- The cast transport audit now rejects truncated successful HTTP responses
  before DLNA progression; the BigBoy focused cast suite passed 26/26.
- The provider-loss verifier now requires a healthy catalog during a provider
  loss sample, preventing simultaneous provider/catalog failure from being
  misclassified; syntax and self-test passed.

Detailed records: `WL-FUNC-021-2026-08-06-roaming-failed-seek-r1.md`,
`WL-FUNC-021-2026-08-06-handoff-seek-r1.md`,
`WL-FUNC-021-2026-08-06-cast-audit-r1.md`, and
`WL-FUNC-021-2026-08-06-provider-loss-verifier-r1.md`.

## Live package boundary

The enhanced read-only verifier self-test, shell syntax checks, payload checks,
and integrity diagnostics pass. The native Fedora 44 release-5 RPM was built,
payload-verified, hash-matched locally and on Dell, transaction-tested, and
installed with operator authorization. Dell now reports
`magic-mesh-12.1.6-5.x86_64`; `mackesd` and `mde-musicd` are active, the live
seat verifier passes, and `rpm -V magic-mesh` is clean. A post-install restart
was required to load the new long-running daemon process.

Detailed record: `WL-FUNC-021-2026-08-06-live-seat-verifier-diagnostics-r1.md`.

## Remaining acceptance

The second canonical seat still needs release-5 installation and CPU proof.
The Dell provider-loss observation remained healthy for all 15 samples and
honestly refused because no natural outage occurred; it did not interrupt the
provider. Live provider-loss recovery, physical renderer proof, and live
two-seat owner-yield/resume remain open. The active epic stays `Remaining`.

## Post-install CPU addendum (2026-08-07)

After the HTTPS fallback idle-poll change, the rebuilt Dell release-5 RPM was
reinstalled and `mackesd.service` restarted. The provenance-bound 30-second
proof passed with max `385‰`, mean `283‰`, stable MainPID, and `NRestarts=0→0`.
The earlier `1106‰/1096‰` sample therefore belongs to the pre-mitigation
runtime and is superseded for the current Dell process. Seat 15 remains on
release 4 and was not mutated.
