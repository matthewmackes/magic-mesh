# WL-CRIT-007 lighthouse coordination admission — 2026-08-09

Peer-return recovery now distinguishes the admitted Workstation and lighthouse
roles before any network, lock, or service mutation. A lighthouse without a
non-empty etcd member configuration fails closed instead of entering the
Workstation-only client path and reporting false recovery without its required
coordination member.

## Verification

- Farm machine 194, build VM `172.20.0.170`, slot
  `crit007-lighthouse-etcd-r4-20260809`.
- The complete peer-recovery fixture passed: offline refusal, three hostile
  roles, missing lighthouse membership, ordered online recovery, coordination
  failure cutoff, boot races, healthy replay, single-flight locking, and
  resume/network trigger filtering.
- The new lighthouse fixture observed
  `refused-lighthouse-etcd-unconfigured` and zero service mutations.
- Farm machine 196, build VM `172.20.0.196`, slot
  `crit007-recovery-payload-r4-20260809`: exact shell syntax and all three
  role-package recovery/identity guards passed.

Source SHA-256:

```text
c57018271298c9a2c2cc7f84b3a0b4c696574f5190e7addb6a6eddc467484800  install-helpers/mesh-peer-recovery.sh
6654624fd69464960ef17cd0e991effd378d1a5369759a1b0acb64fe73067b41  install-helpers/test-mesh-peer-recovery.sh
```

Live lighthouse network-return and the remaining fleet sleep/reboot matrix
remain required; this fixture does not claim them.
