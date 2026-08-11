# WL-CRIT-007 SSH overlay bounds — 2026-08-11

- Scope: overlay-IP reads require a regular file under 256 bytes; SSH reset/reload commands use the shared timeout boundary.
- Farm: BigBoy `172.20.0.130`, slot `crit007-sshd-bounded-r233`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=crit007-sshd-bounded-r233 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::sshd_overlay_bind -- --nocapture`.
- Result: PASS, 9 passed, 0 failed.
