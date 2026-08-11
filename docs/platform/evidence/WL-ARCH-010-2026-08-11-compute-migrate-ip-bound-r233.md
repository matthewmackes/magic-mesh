# WL-ARCH-010 compute migration probe bound — 2026-08-11

- Scope: migration's local `ip` probe uses the shared timeout/capture helper; the static guard prevents the bare path returning.
- Farm: BigBoy `172.20.0.130`, slot `compute-migrate-ip-timeout-r233`.
- Result: PASS, 1 passed, 0 failed.
