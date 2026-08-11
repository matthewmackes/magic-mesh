# WL-FUNC-021 shared artwork byte bound — 2026-08-11

- Scope: music artwork cache.
- Change: shared artwork reads and writes are bounded at 4 MiB; non-regular files and oversized payloads are refused.
- Focused gate: `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func021-artwork-bound-r222 install-helpers/xcp-build.sh cargo test -p mde-musicd --lib cache::tests::oversized_shared_artwork_is_refused_without_an_unbounded_read -- --exact --nocapture`
- Result: PASS — 1 passed, 0 failed.
- `git diff --check`: PASS.
