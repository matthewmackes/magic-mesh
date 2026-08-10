# WL-FUNC-019 peer-record nofollow — r180

- Scope: Remote Sessions peer registry reopens records with kernel-enforced `NOFOLLOW|CLOEXEC` after metadata validation, closing the final-symlink swap window.
- Farm gate: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=func019-peer-record-nofollow-r180 install-helpers/xcp-build.sh cargo test -p mackesd --lib workers::desktop_sources::tests::peer_record_open_refuses_final_symlink -- --nocapture`
- Result: `1 passed; 0 failed; 4712 filtered out` on seat `.90`.
