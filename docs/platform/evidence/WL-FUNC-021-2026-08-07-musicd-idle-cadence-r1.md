# WL-FUNC-021 — mde-musicd idle non-transport cadence (2026-08-07)

## Change

The daemon keeps the 500 ms queue, transport, and peer control path, but no
longer scans every browse topic or reloads credential files on every control
sweep. Browse-topic scans run at a bounded 1-second cadence; credential and
source-file reloads run at a bounded 2-second cadence. The first browse scan
still runs immediately after startup, and provider/client reuse is unchanged.

This removes repeated idle Bus-query, filesystem, JSON, and credential-path
work that was synchronized across seats while preserving bounded interactive
transport response latency.

## Verification

- BigBoy `.130`, slot `musicd-cadence-full-r1`: full `mde-musicd` library suite
  passed `189 passed; 0 failed`.
- Farm `.50`, slot `musicd-cadence-focused-r1`: the new cadence regression
  passed `1 passed; 0 failed` with 188 tests filtered.
- The test asserts both non-transport cadences remain slower than the 500 ms
  control cadence.
- A package-wide `cargo fmt -p mde-musicd -- --check` was attempted on `.90`;
  it reports pre-existing formatting differences in unrelated dirty regions of
  the crate. No formatter rewrite was applied to preserve user changes.

This is source/farm evidence only. Installed-seat CPU reduction and the
five-seat CPU/NWS acceptance still require reachable current-package seats.
