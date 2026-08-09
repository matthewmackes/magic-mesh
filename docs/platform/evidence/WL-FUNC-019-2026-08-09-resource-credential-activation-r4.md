# WL-FUNC-019 resource credential activation — 2026-08-09

## Outcome

The resource-publisher credential helper was shipped but had no packaged boot
or upgrade activation path. A workstation could therefore retain an available
resource catalog while authenticated proof and every resource action stayed
disabled indefinitely.

The base package now ships and enables a bounded best-effort oneshot. Package
installation and upgrade rerun it before the existing controlled shell restart.
When `resource/publisher-hmac` is distributed, the helper idempotently stages
the host-bound systemd credential and exact shell drop-in; unchanged encrypted
material is preserved, and symlinked output leaves are refused. Missing secret
state remains an honest read-only mode and never blocks boot.

## Verification

- Farm `.90`, slot `func019-resource-credential-r4-20260809`.
- Helper `bash -n` and `--self-test`: passed.
- New unit parsed cleanly with `systemd-analyze verify` in an isolated root.
- Static RPM payload gate: all checks passed and named the new unit in the base
  asset set.

## Live limitation

Basement seat 15 currently lacks the replicated `resource/publisher-hmac`
secret and corresponding encrypted credential. This change closes automatic
activation once the approved secret is distributed; it does not fabricate or
silently initialize independent per-node keys. Live authenticated action proof
therefore remains open.
