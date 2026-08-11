# WL-ARCH-009 renamed service owner refusal — 2026-08-11

- Scope: packaged systemd source may launch `mackesd serve` only through the six canonical group units.
- Hostile boundary: a renamed `mesh-control-recovery.service` cannot create a seventh direct daemon authority.
- Focused gate: sync through `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=2 install-helpers/xcp-build.sh sync`, then run `python3 install-helpers/verify-mackesd-process-boundary.py --self-test` in that farm workspace.
- Farm: `172.20.0.90`, slot 2, admitted with 23,029,388 KiB free.
- Result: **PASS**, focused verifier self-test including the hostile renamed-unit fixture.
- Remaining boundary: installed `/etc` overrides, transient/user units, and live PID/cgroup ownership proof remain.
