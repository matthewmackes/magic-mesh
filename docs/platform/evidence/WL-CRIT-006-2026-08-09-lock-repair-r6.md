# WL-CRIT-006 locked-build repair — 2026-08-09

## Scope

Repair the immutable candidate build's stale Cargo lock graph without changing
the manifest, release workflow, package policy, or current implementation work.

## Root cause and correction

The workspace manifest already pins `jiff = 0.2.21`, and the committed
`mackesd` manifest consumes it for `workers/clock.rs`. `Cargo.lock` contained
the registry records for `jiff` and `jiff-static`, but the local `mackesd`
package record did not list `jiff` in its dependency array. Therefore Cargo's
`--locked` resolver correctly refused the immutable candidate build.

On BigBoy `172.20.0.130`, isolated slot `crit006-lock-r6`, the canonical command
`cargo update -p mackesd` reported zero package updates and generated exactly
one lockfile delta: adding `jiff` to the `mackesd` dependency list. No version,
source, or checksum was edited or changed manually.

## Focused locked proof

The following BigBoy command passed:

```text
MCNF_BUILD_HOST=172.20.0.130 MCNF_BUILD_SLOT=crit006-lock-r6 \
  install-helpers/xcp-build.sh cargo test -p mackesd --lib \
  --features async-services --locked \
  workers::clock::tests::weekday_alarm_resolves_dst_and_advances_once_per_selected_civil_day \
  -- --exact
```

Result: one test passed, zero failed, zero ignored, and 4,376 filtered out.
The locked graph compiled `jiff v0.2.21` and `mackesd v12.1.6` successfully.

## Separate observed blocker

A diagnostic `cargo check -p mackesd --lib --features async-services --locked`
accepted the repaired lock and compiled `jiff`, then failed on an existing
non-test feature-gating defect: production modules import cloud gate symbols
that `workers/cloud/mod.rs` currently re-exports only under `#[cfg(test)]`.
That Rust defect is outside this lockfile-only ownership scope and does not
invalidate the successful locked Clock/Jiff proof above.

This repair does not create, sign, deploy, or attest a release candidate.
