# WL-CRIT-007 pre-grouped overlay re-attestation — r533

Date: 2026-08-13

## Production gap closed

Peer-return recovery previously re-attested the physical route and configured
etcd/Syncthing services after Syncthing activation, but did not re-attest the
Nebula overlay immediately before starting grouped mackesd workers. If Nebula
died during the bounded Syncthing start while the default route and both local
services remained active, recovery could start workers against a partial mesh
before the later desktop-boundary check detected the loss.

`install-helpers/mesh-peer-recovery.sh` now fails closed with
`overlay-lost-before-grouped` unless the exact Nebula service and `nebula1`
address are ready at that mutation boundary. The existing recovery fixture was
extended with the exact transition: Syncthing activation drops Nebula while
physical network, etcd, and Syncthing remain active. It proves the only service
mutations are `etcd.service` and `syncthing.service`; no grouped worker, XDG, or
session mutation follows.

## Farm gates

- `172.20.0.90`, slot `crit007-overlay-pre-grouped-r533`:
  `sudo -n bash install-helpers/test-mesh-peer-recovery.sh` — passed the complete
  recovery fault suite, including `PASS pre-grouped overlay fixture: lost
  overlay blocks grouped mutation`.
- `172.20.0.90`, the same warmed isolated slot:
  `bash install-helpers/verify-rpm-payload.sh --self-test` — passed every
  packaging-contract assertion.
- `172.20.0.196`, slot `crit007-overlay-shellcheck-r533`:
  `bash -n install-helpers/mesh-peer-recovery.sh
  install-helpers/test-mesh-peer-recovery.sh` — passed.
- Local non-heavy checks: scoped `git diff --check` and `bash -n` passed.
- Clippy/build: not applicable; the owned production and fixture changes are
  shell scripts and do not alter a Rust or compiled target.

The attempted ShellCheck gate on `.196` did not execute because that farm image
does not provide `shellcheck`. A BigBoy `.130` package-contract sync was aborted
after its owned rsync remained in uninterruptible I/O; the exact unique gate was
then run successfully from the already-synced `.90` slot. Neither infrastructure
attempt is counted as positive source evidence.

## Remaining WL-CRIT-007 acceptance

- Complete any remaining boot-order and desktop/workload recovery coding gaps.
- After the first full release, run the deferred non-blocking one-node boot,
  suspend/resume, network-transition, reboot, and corrected-forward upgrade
  matrix.
- Record direct recovery evidence that one authenticated identity, one daemon
  set, and one session return, with stale identities, leases, rows, and
  processes removed or surfaced as actionable failures.
