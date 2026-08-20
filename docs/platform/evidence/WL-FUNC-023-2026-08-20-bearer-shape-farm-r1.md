# WL-FUNC-023 bearer-shape handoff evidence — 2026-08-20

## Scope

This slice hardens the existing `SshBootstrap` enrollment handoff. Before a
bearer is written to the SSH child stdin, the transport now requires the exact
43-character URL-safe unpadded-base64 shape emitted by the existing
`bearer_ledger::issue` primitive. Invalid short, whitespace, or punctuation
values are rejected as `BundleRejected`; valid bearers remain absent from SSH
argv and redacted action output. No live SSH success is claimed.

Changed implementation:

- `crates/mesh/mackesd/src/onboard/remote_push.rs`

The focused regression adds malformed-bearer refusal before any credential
handoff and keeps the valid-bearer path at the typed `NotWired` boundary when
live SSH prerequisites are unavailable.

## Farm verification

Admission and execution:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=3 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  onboard::remote_push -- --nocapture
```

- Host: BigBoy `172.20.0.130`, slot `3`
- Admission: `28,500,624 KiB` free; required `8,388,608 KiB`
- Result: `27 passed, 0 failed, 0 ignored, 5001 filtered out`
- Exit: `0`
- Elapsed: `598.5s` (including farm admission, sync, and compile)
- Source revision at dispatch: `fb1a75df49435777ff6e0c1f14f6a0d2e4812f05`

The farm emitted two pre-existing unreachable-pattern warnings in
`workers/call_media.rs`; they are outside this slice. The focused tests,
including `ssh_bootstrap_rejects_a_malformed_bearer_before_secret_handoff`,
passed.

## Local checks and boundary

Targeted `rustfmt --edition 2024` completed successfully for the changed Rust
file. Workspace-wide `cargo fmt --check` remains non-green because of an
unrelated pre-existing formatting difference in
`crates/mesh/mackesd/src/workers/call_media.rs`; that file is outside the
authorized write scope and was not changed.

This is farm contract/hostile-input evidence only. Provider-side token
materialization, live SSH enrollment, installed first boot, and physical-seat
acceptance remain open under the epic's existing boundaries.
