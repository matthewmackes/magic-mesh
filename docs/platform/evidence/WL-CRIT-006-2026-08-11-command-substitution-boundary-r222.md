# WL-CRIT-006 command-substitution boundary — 2026-08-11

- Scope: release-gate command validation.
- Change: corrected `COMMAND_CONTROL_RE` so `$(` is rejected as shell command substitution while `${MCNF_*}` parameter expansion remains allowed.
- Focused gate: `python3 install-helpers/verify-release-gate-matrix.py --self-test`
- Farm: `.50` (`172.20.0.50`), slot `crit006-command-substitution-r222`, after `xcp-build.sh sync`.
- Result: PASS — 1 valid fixture and 19 hostile fixtures rejected.
- Local confirmation: same self-test and `git diff --check` passed.
