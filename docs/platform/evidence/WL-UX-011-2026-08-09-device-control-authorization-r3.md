# WL-UX-011 device-control authorization r3 — 2026-08-09

The live device-control path now reuses the platform root-shell signer and
daemon `ActionAuthorizer`. Schema-v1 requests are bound to the exact operation,
device identity, provider generation, target host, requester, and a short-lived
single-use capability before the node may execute its fixed sysfs/binary plan.
Missing, altered, replayed, wrong-schema, raw-command, arbitrary-path, secret,
and unknown nested fields fail closed before effects. Refusals continue through
the existing device-control audit and failure-notification path; tokens are not
copied into audit records.

## Focused farm verification

Machine 9 (`172.20.0.50`), slot `ux011-controls-r3`:

- Shared contract hostile-field regression: 1 passed, 0 failed.
- Executor missing/altered/replay hardware-boundary regression: 1 passed, 0 failed.
- Root-shell authorized publication regression: 1 passed, 0 failed.
- Earlier same-slot focused module checks passed 7/7 contract and 18/18 executor
  tests before the final exact regressions; no broad acceptance claim is made.
- `git diff --check`: passed.

Source SHA-256 values:

- `mackes-mesh-types/src/device_control.rs`: `04e98161930d4c2251246dfb4931ae7d40216ab0ae55938f9573fad8772e3721`
- `mackesd/src/workers/device_control.rs`: `f0e2435055e245dbb33e35803a367fdde3131eeebb63e6927bfabfb42f6cde7e`
- `device_manager/mod.rs`: `65bd5b0b2699a09e7df765a7aeabfda52dc0887810a99e289963f38ca6987608`
- `device_manager/tests.rs`: `d3c6a2a1b25a13be817b40eef66bfb64c552859bc897c2e47cba2ed962b7fbf0`

## Remaining S3 blockers

Device controls still need staged preview/result evidence for every supported
operation, cancellation after replication, explicit multi-step partial-failure
recovery, package verification, and installed live-hardware proof. This slice
does not claim WL-UX-011 or S3 closure.
