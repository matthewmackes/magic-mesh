# WL-CRIT-007 boot order and local identity admission S1 — 2026-08-08

Nebula now waits for network readiness and runs one root-owned, fail-closed
local identity guard before overlay startup. The guard admits the legacy flat
layout or one generation selected by the exact `identity/current` symlink, but
rejects mixed layouts, unsafe ownership or modes, symlink substitution, stale
or untrusted certificates, and malformed identity state.

etcd and Syncthing require the admitted Nebula service. The grouped mackesd
target requires Nebula and all six daemon groups, while the control group waits
for an optional local etcd member. This makes the shell's existing ordering on
`mackesd.target` reflect actual daemon readiness.

## Verification

- `.90`, slot `crit007-systemd-s1-r1`: `systemd-analyze verify` passed with no
  syntax errors or dependency cycles.
- The same slot admitted valid legacy and generation identities and refused
  duplicate flat/generation state plus stale or untrusted certificates.
- `.90`, slot `crit007-rpm-guard-r2`: the role-package identity check passed for
  base, server, and lighthouse packages. Each ships the active local guard
  helper exactly once; the post-overlay distributed producer remains disabled.
- `bash -n` and `git diff --check` passed. `shellcheck` was unavailable on the
  farm guest.
- No operational tests were removed.

## Remaining acceptance gap

The local boot identity is now guarded. Distributed live-claim collision
detection still requires an authenticated pre-Nebula authority transport that
does not create an overlay/etcd cold-boot cycle. Sleep/network rejoin, real fleet
reboots, desktop/workload restoration, and corrected-forward rollout remain, so
CRIT-007 stays `Remaining`.
