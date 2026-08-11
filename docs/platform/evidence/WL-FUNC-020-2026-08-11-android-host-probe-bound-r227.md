# WL-FUNC-020 Android host-probe bound — 2026-08-11

- Scope: `/proc/meminfo` and nested-KVM sysfs probes now require bounded regular files and reject probe text over 64 KiB before parsing.
- Farm: BigBoy `172.20.0.130`, slot `func020-android-probe-bound-r227b`.
- Command: `MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=func020-android-probe-bound-r227b install-helpers/xcp-build.sh cargo test -p mackesd --features async-services --lib workers::cloud::android_provider::tests::oversized_host_probe_text_is_rejected_before_parse -- --exact --nocapture`
- Result: PASS, 1 passed, 0 failed.
