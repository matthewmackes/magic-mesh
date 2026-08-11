# WL-CRIT-007 — stale-session duplicate guard (r213)

- Scope: peer recovery refuses to start a duplicate Workstation shell when an
  inactive unit still has an orphaned `mde-shell-egui` process, before XDG mutation.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=crit007-stale-session-guard-r213-final2 install-helpers/xcp-build.sh sync`; farm shell ran `sudo ./install-helpers/test-mesh-peer-recovery.sh`.
- Result: all recovery fixtures passed, including `PASS stale-session fixture: orphaned shell refuses duplicate recovery`.
