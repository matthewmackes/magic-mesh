# WL-CRIT-007 — recovery readiness ordering (r149)

Date: 2026-08-10

`mesh-peer-recovery.sh` now restores role-specific XDG desktop state only
after the grouped `mackesd` target has reached readiness. A failed daemon
recovery therefore cannot partially mutate or report a healthy desktop bind.

The dedicated recovery fixture passed on farm `.50`, including the expected
mutation order, while shell syntax and `git diff --check` passed. Live
suspend/resume and fleet-convergence proof remain outstanding.

