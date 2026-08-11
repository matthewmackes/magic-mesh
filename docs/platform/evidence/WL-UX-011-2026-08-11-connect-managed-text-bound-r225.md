# WL-UX-011 CONNECT managed-text bound — 2026-08-11

- Scope: CONNECT reconcile state and managed Caddy fragment reads.
- Change: managed text files are capped at 128 KiB and invalid/oversized content fails closed before state parsing or fragment comparison.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=drain-r225-connect-bigboy install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::connect_firewall::tests::managed_text_reader_rejects_oversized_content -- --exact --nocapture`.
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
