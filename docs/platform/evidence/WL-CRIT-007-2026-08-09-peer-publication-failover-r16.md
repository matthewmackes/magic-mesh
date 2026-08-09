# WL-CRIT-007 — peer-publication-aware etcd failover (r16)

Date: 2026-08-09

## Incident and live correction

Dell (`172.20.146.225`, Nebula `10.42.0.4`) was physically online with Nebula,
all six grouped `mackesd` services, the shell, and Syncthing active, but every
roster consumer reported its approximately ten-day-old peer row as offline.
The heartbeat journal repeated:

```text
peer-record: lease-backed peer/claim transaction failed; will retry next heartbeat
```

The configured voters were `10.42.0.1`, `.2`, and `.3`. Members `.2` and `.3`
committed; `.1` remained reachable at the network layer but could not commit an
etcd proposal. A bounded DigitalOcean soft reboot (action `3338686624`) and hard
power cycle (action `3338687660`) did not restore `.1`.

As a recoverable live correction, Dell and seat 15 client endpoint files were
backed up and changed to the two committing voters only, then their six grouped
workers were restarted. Backups:

- Dell: `/etc/mackesd/etcd-endpoints.pre-lh1-recovery-20260809T1159Z`
- seat 15: `/etc/mackesd/etcd-endpoints.pre-lh1-recovery-20260809T1200Z`

Dell then published a fresh healthy online row at `1786277164623`; seat 15
published online at `1786277108142` (degraded only by its pre-existing disk
alarm). Dell's shell also completed one controlled restart. The failed `.1`
member remains in etcd membership for console/data inspection; it was not
deleted. T480 and Surface rejected the available SSH identity, and Eagle's
`mackesd.target` was inactive, so this record does not claim full-fleet
convergence.

## Corrected-forward implementation

- The shared etcd connector now probes configured members one at a time with a
  bounded linearizable read, returns the first member that can actually commit,
  and remembers that member as the next preferred endpoint. A merely reachable
  non-committing first URL can no longer poison every short-lived peer
  transaction.
- The heartbeat writes `/run/mesh-health/peer-publication.ok` atomically only
  after its peer-row and overlay-identity-claim transaction succeeds.
- `mesh-health-check` requires that success stamp to remain fresh on etcd-backed
  nodes. Missing/stale publication is degraded health, requests a bounded
  observation-group restart, and exits nonzero instead of printing `ok`.
- The existing deterministic health fixture now covers the exact one-bad,
  one-good-or-better coordination case and proves that quorum reachability does
  not mask stale own-peer publication.

## Focused verification

- Machine 9 (`172.20.0.50`), slot `crit007-health-fixture-r1`:
  `install-helpers/test-syncthing-device-scope.sh` — PASS, including stale
  publication refusal and bounded recovery request.
- Machine 193 (`172.20.0.90`), slot `crit007-etcd-failover-r1`:
  `endpoint_failover_starts_at_last_committing_member_and_wraps_once` — PASS,
  1 passed, 0 failed, 4,386 filtered out.
- Machine 194 (`172.20.0.170`), slot `crit007-publication-stamp-r1`:
  `successful_peer_publication_stamp_is_atomic_and_refreshable` — PASS, 1
  passed, 0 failed, 4,386 filtered out.
- Machine 196 (`172.20.0.196`), slot `crit007-fmt-r1`: direct `rustfmt
  --edition 2021 --check` on both changed Rust files — PASS. The package-wide
  fmt check remains red on pre-existing unrelated files and is not claimed.
- Local `bash -n`, deterministic fixture, worklist lint, and `git diff --check`
  passed. No broad suite was run.

The corrected-forward bytes were not deployed to the live fleet in this lane;
the live endpoint-file correction above restored Dell and seat 15 while the
package path remains subject to CRIT-006 release gates.
