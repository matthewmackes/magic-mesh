# WL-ARCH-009 exact grouped-service policy — 2026-08-11

- Scope: the production package boundary verifier now refuses every hard
  lifecycle edge between the six grouped daemons, including `Requires`,
  `Requisite`, `BindsTo`, peer `PartOf`, `Upholds`, `Conflicts`, and explicit
  stop propagation. It also requires each group's exact CPU, memory, task, I/O,
  accounting, watchdog, and restart policy; blank, duplicate, or unlimited
  limits fail closed.
- Production path: GitHub/always-on CI → `ci-gate.sh` policy stage → shipped
  systemd units → package process-boundary verifier → acceptance refusal.
- Focused gates:
  `python3 install-helpers/verify-mackesd-process-boundary.py --self-test`:
  PASS, including hostile peer coupling and `MemoryMax=infinity` fixtures;
  `install-helpers/ci-gate.sh --self-test`: PASS, proving the clean fixture
  passes and injected peer `BindsTo=` fails through the same policy aggregator.
- The six current production units already satisfy the policy and were not
  changed.
- Remaining epic boundary: installed-fleet cgroup/restart and duplicate-owner
  census, and Workers UI cutover.
