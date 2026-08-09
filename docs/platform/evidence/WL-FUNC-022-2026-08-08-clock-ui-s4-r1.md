# WL-FUNC-022 daemon-projected Clock UI S4 — 2026-08-08

The shell Clock surface now projects the daemon-owned World Clock, Alarms,
Timers, and Stopwatch model instead of persisting or scheduling local alarms.
Clock mutation emits one bounded signed `ClockCommandV1` on the canonical target
topic and fails visibly closed when the signer, credential, Bus, projection, or
IANA zone is unavailable.

Jiff is workspace-pinned exactly to 0.2.21. Display time uses admitted IANA zone
names; the former hand-coded US DST table and misleading UTC fallback are gone.
Ringing alarms expose Stop/Snooze, and expired timers expose Stop then the typed
Add-one-minute action supported by the current schema.

## Verification

- Farm `.50`, slot `func022-clock-ui-actions-s4-r2`:
  `cargo test --locked -p mde-shell-egui timers::tests --no-default-features`
  passed 5/5, including unavailable-zone and missing-signer refusal.
- `cargo check --locked -p mde-shell-egui --no-default-features` passed in the
  same slot; scoped diff checks passed.

## Remaining acceptance gap

The deployed seat needs matching shell/daemon signing configuration. Atomic
acknowledge-and-extend is absent from the current schema, and responsive render,
live daemon round-trip, direct-DRM, notification/bell, and curtain proof remain.
FUNC-022 stays `Remaining`.
