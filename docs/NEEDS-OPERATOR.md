# NEEDS-OPERATOR — parked blockers awaiting an operator action

Entries are appended by `install-helpers/park-blocker.sh` (DRAIN-5). Each is a
unit the autonomous loop **parked** (`[!]` in `docs/WORKLIST.md`) because it needs
a live fleet, an operator secret, or a gated activation the loop must not perform
itself. Clear an entry by doing its **unblock** action, then flip the worklist
marker back off `[!]`.

## DRAIN-4-ACTIVATE
- **parked:** 2026-06-28T18:46:18Z
- **reason:** FARM-AUTOSCALE apply path is implemented + gated (FA_APPLY default 0, apply-gate + --readiness preflight). Activating LIVE provisioning is operator-gated: it needs XO reachable, tofu state present, and the golden template on XCP-2 — and enabling the systemd timer. The autonomous loop must NOT flip FA_APPLY=1, enable mcnf-farm-autoscale-reconcile.timer, or apply tofu (would clone/destroy real VMs).
- **unblock:** Operator: (1) bring XO up + mint token; (2) confirm 'install-helpers/farm-reconciler.sh --readiness' (with FA_APPLY=1) reports READY; (3) 'systemctl enable --now mcnf-farm-autoscale-reconcile.timer' with FA_APPLY=1 in the unit drop-in.
