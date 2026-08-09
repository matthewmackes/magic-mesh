# WL-FUNC-022 S6 display-zone migration hard cut r14 — 2026-08-09

## Scope

This checkpoint implements the narrow upgrade hard cut for the shell-owned
display-zone preference. It does not claim the rest of S6 or FUNC-022 complete.

- `settings-clock.json` now stores a named IANA zone string, not the retired
  five-variant `ClockZone` enum or an atomic numeric index.
- Exactly the five serialized legacy values migrate once:
  `eastern_standard` → `America/New_York`, `central_standard` →
  `America/Chicago`, `mountain_standard` → `America/Denver`,
  `pacific_standard` → `America/Los_Angeles`, and `utc` → `UTC`.
- Migration writes pretty JSON to a sibling temporary file and renames it over
  `settings-clock.json`. A failed replacement falls back safely instead of
  claiming a migration that was not persisted.
- Unknown legacy-looking values, malformed JSON, unknown fields, and IANA names
  absent from Jiff's configured zoneinfo database fail to the deterministic
  Eastern default without rewriting the rejected file.
- The migration reads only the explicit `settings-clock.json` path. A focused
  fixture places `timers-alarms.json` beside it and proves that file remains
  byte-for-byte untouched. No alarm or timer is imported into the daemon Clock
  database.
- Live display offsets continue to come from workspace-pinned Jiff and platform
  zoneinfo. The old enum/index timezone authority was removed; the five visible
  Settings choices are now labels over IANA identifiers only.

## Focused farm verification

Host: machine 9 (`172.20.0.50`)

Slot: `func022-zone-migration-r14`

Commands:

```text
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func022-zone-migration-r14 install-helpers/xcp-build.sh cargo test --locked -p mde-shell-egui persisted_config_tests::clock_config_ -- --nocapture
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func022-zone-migration-r14 install-helpers/xcp-build.sh cargo test --locked -p mde-shell-egui timers::tests::exact_iana_zoneinfo_handles_dst_and_refuses_unknown_zones -- --exact --nocapture
MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=func022-zone-migration-r14 install-helpers/xcp-build.sh cargo test --locked -p mde-shell-egui timers::tests::shell_has_no_scheduling_or_store_authority -- --exact --nocapture
ssh machine-9 'rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/timers.rs crates/desktop/mde-shell-egui/src/system/mod.rs'
```

Results:

- display-zone migration/refusal/non-import tests: **3 passed, 0 failed**;
- exact Jiff IANA/DST/refusal test: **1 passed, 0 failed**;
- shell scheduling/store-authority guard: **1 passed, 0 failed**;
- changed-file Rust formatting check: **passed**;
- `git diff --check`: **passed**.

The focused build emitted existing workspace warning debt, including warnings
from concurrently edited device-control files; no warning was introduced as a
failure or treated as proof for this checkpoint.

## Remaining blockers

FUNC-022 remains `Remaining`. S6 still needs package/service and documentation
cutover, installed payload checks, fresh-install and upgrade proof, and named
live-seat/fleet evidence. Other epic gaps listed in the active worklist also
remain outside this checkpoint.
