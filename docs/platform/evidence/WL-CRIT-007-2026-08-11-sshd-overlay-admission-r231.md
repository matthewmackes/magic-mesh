# WL-CRIT-007 SSH overlay admission — 2026-08-11

- Scope: invalid overlay-IP content fails closed before SSH drop-in publication or systemctl reload.
- Farm: BigBoy source lane; focused test: `workers::sshd_overlay_bind::tests::invalid_overlay_publish_value_defers_without_writing_dropin`.
- Farm: BigBoy `172.20.0.130`, slot `ux011-sshd-overlay-admission-r231b`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=ux011-sshd-overlay-admission-r231b install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::sshd_overlay_bind::tests::invalid_overlay_publish_value_defers_without_writing_dropin -- --exact --nocapture`.
- Result: PASS, 1 passed, 0 failed.
