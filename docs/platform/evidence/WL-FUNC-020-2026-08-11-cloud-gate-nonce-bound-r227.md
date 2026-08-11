# WL-FUNC-020 cloud gate nonce bound — 2026-08-11

- Scope: expired replay rows are opened without following symlinks, require regular files, and are bounded to 128 bytes before parsing.
- Farm: BigBoy `172.20.0.130`, slot `cloud-gate-bound-r227`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=cloud-gate-bound-r227 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::cloud::gate::tests::claim_nonce_rejects_oversized_or_symlinked_expired_nonce_rows -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
