# WL-ARCH-008 Browser digest canonicalization — 2026-08-13

## Scope

The typed `browser-provision` boundary now accepts only the canonical
`sha256:<64-lowercase-hex>` spelling. Equivalent uppercase spellings are
rejected before desired-state persistence, preventing textual identity drift
across replay and migration boundaries.

## Farm gate

- Host: `172.20.0.90` (farm build VM)
- Slot: `arch008-browser-digest-canonical-rerun-20260813`
- Command: `MCNF_BUILD_HOST=172.20.0.90 MCNF_BUILD_SLOT=arch008-browser-digest-canonical-rerun-20260813 install-helpers/xcp-build.sh cargo test -p mackesd --locked workers::cloud::verbs::browser --lib -- --nocapture`
- Result: PASS — 10 passed, 0 failed, 0 ignored, 4,914 filtered out.

## Changed files

- `crates/mesh/mackesd/src/workers/cloud/verbs/browser.rs`
- `docs/platform/evidence/WL-ARCH-008-2026-08-13-browser-digest-canonical-r477.md`

## Remaining acceptance

This closes only the Browser provisioning artifact-identity slice. ARCH-008
still requires the standalone repository/history, portable migration and
secret boundary, host-stack removal scan, reproducible Browser VM/image and
readiness proof, shell VDI controller/live captures, and the quality/upgrade
measurements from S1–S6. Post-release package and live-seat proof remain
non-blocking per the current release policy, but are not claimed by this
focused farm gate.
