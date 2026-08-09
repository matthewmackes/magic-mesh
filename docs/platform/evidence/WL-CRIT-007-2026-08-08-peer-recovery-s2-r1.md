# WL-CRIT-007 peer recovery S2 first slice — 2026-08-08

Resume and positive NetworkManager events now trigger one hardened, bounded
peer-recovery service. It refuses to mutate services without an approved
network manager reporting online, coalesces concurrent triggers with a runtime
lock, restores Nebula with bounded exponential backoff, then restores configured
etcd, Syncthing, and all six grouped mackesd services in dependency order.

The helper never enrolls, leaves, founds, or changes mesh identity. Recovery
state is bounded to systemd service status and the journal, and the service has
a 90-second runtime ceiling.

## Verification

- `.90`, slot `crit007-peer-recovery-s2-r1`: offline fixture passed with zero
  service mutations.
- The online fixture passed with Nebula attempts separated by 1s/2s backoff,
  followed by etcd, Syncthing, and grouped mackesd restoration.
- Concurrent triggers coalesced; sleep/network hooks accepted only post-resume
  and positive network-return events.
- Base, server, and lighthouse package checks passed all 9/9 required
  identity/recovery assets per role.
- `systemd-analyze verify`, shell syntax, and `git diff --check` passed with no
  dependency cycles. `shellcheck` was unavailable on the farm guest.
- No operational tests were removed.

## Remaining acceptance gap

Live laptop suspend/resume, physical network transition, repeated fleet loss,
and multi-node convergence have not yet been exercised. Workload/VDI/desktop
restoration and corrected-forward rollout also remain, so CRIT-007 stays
`Remaining`.
