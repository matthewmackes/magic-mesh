# WL-FUNC-020 guest readiness publication — 2026-08-11

- Scope: Android guest-tool readiness pins every output-directory component, writes through an exclusive staging descriptor, and publishes with descriptor-relative rename.
- Hostile boundary: a symlinked output parent or staging substitution cannot redirect the readiness receipt.
- Focused gate: `packaging/android/record-guest-tool-readiness.sh --self-test`.
- Farm: fixed coordinator snapshot on `172.20.0.196`, slot 1.
- Result: **PASS**, including the hostile symlinked-parent fixture.
- Remaining boundary: capture a live Cuttlefish guest-tool receipt and bind it to current Workloads generation and image evidence.
