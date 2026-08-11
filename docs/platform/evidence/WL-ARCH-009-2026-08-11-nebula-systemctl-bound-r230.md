# WL-ARCH-009 Nebula systemctl bound — 2026-08-11

- Scope: Nebula supervisor systemctl calls kill hung children after 2 seconds and retain no more than 8 KiB per output stream.
- Farm: BigBoy `172.20.0.130`, slot `arch009-nebula-systemctl-bound-r230`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=arch009-nebula-systemctl-bound-r230 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::nebula_supervisor::tests::systemctl_timeout_kills_a_hung_command -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
