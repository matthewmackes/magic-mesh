# WL-CRIT-007 post-etcd dependency re-attestation — 2026-08-13

## Acceptance gap closed

Peer-return recovery previously treated a successful `etcd.service` start plus
an online physical route as authority to mutate Syncthing. Nebula or the local
etcd member could disappear after the completed etcd step while the route
remained online. The helper would then start Syncthing before detecting the
partial substrate at the later pre-grouped gate.

`mesh-peer-recovery.sh` now re-attests both Nebula readiness and local etcd
activity immediately after the completed etcd step and before the Syncthing
transaction. Loss publishes `overlay-lost-after-etcd` or
`etcd-lost-after-etcd`, returns failure, and leaves Syncthing and all later
groups/session restoration untouched.

The hostile fixture models both exact transitions. In each case it proves that
`etcd.service` is the only mutation and that the expected fail-closed state is
published. Existing physical-link, post-Syncthing, grouped, desktop, and final
convergence fixtures remain intact.

## Farm evidence

- `172.20.0.170`, slot `crit007-post-etcd-test`:
  `sudo bash install-helpers/test-mesh-peer-recovery.sh` — PASS, including
  `PASS post-etcd dependency fixture: lost overlay/coordination blocks Syncthing mutation`.
- `172.20.0.50`, slot `crit007-post-etcd-shellcheck`:
  `shellcheck -e SC2015,SC2016 install-helpers/mesh-peer-recovery.sh install-helpers/test-mesh-peer-recovery.sh`
  — PASS. The exclusions are four pre-existing informational findings outside
  this slice (three existing bounds expressions and the existing lock fixture).
- `172.20.0.130`, slot `crit007-peer-unit-verify`:
  the helper was temporarily staged at its RPM payload path, then
  `systemd-analyze verify packaging/systemd/mcnf-peer-recovery.service` — PASS.
  The staged file was removed by the command trap.
- Local tiny checks: `bash -n` for both scripts and `git diff --check` — PASS.

## Remaining acceptance

WL-CRIT-007 still requires the first-release package/build verification and the
deferred, non-blocking post-release one-node boot, suspend/resume, physical
network-return, authenticated rejoin, grouped-service, and session-restoration
proof. This evidence makes no installed-seat or live-fleet claim.
