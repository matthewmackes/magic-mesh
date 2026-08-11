# WL-CRIT-007 — missing Workstation session return (r205)

- Scope: peer-return recovery restores an inactive `mde-shell-egui.service`
  Workstation session additively, without restarting healthy mesh services.
- Farm gate: `printf '%s\\n' 'cd magic-mesh-farm-crit007-session-return-r205 && sudo ./install-helpers/test-mesh-peer-recovery.sh' | MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=crit007-session-return-r205 install-helpers/xcp-build.sh shell`.
- Result: exit `0`; the full recovery fixture passed, including `PASS missing-session fixture`.
