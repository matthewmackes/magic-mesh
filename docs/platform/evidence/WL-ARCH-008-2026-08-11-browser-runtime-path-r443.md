# WL-ARCH-008 Browser runtime executable path — 2026-08-11

- Scope: the Browser VM runtime pins executable lookup to immutable guest system directories and selects Chromium only from fixed `/usr/bin` entrypoints.
- Hostile boundary: an xrdp-provided executable path cannot substitute a host-provisioned Browser or helper after restart.
- Focused gate: `packaging/browser-vm/verify-session-input-contract.sh --self-test`.
- Farm: fixed coordinator snapshot on `172.20.0.196`, slot 1.
- Result: **PASS**, including the hostile executable-search-path fixture.
- Remaining boundary: demonstrate the same refusal in a live Browser VM session while preserving guest Chromium input and audio.
