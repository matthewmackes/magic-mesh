# WL-FUNC-017 S2 effective-location authority — 2026-08-08

A default-on Workstation worker now owns Auto/Manual weather location,
effective generation, and the action cursor in one atomically replaced,
root-only record. Auto accepts only a fresh, same-host, online GNSS fix with
supported NWS coverage, then falls back to the last verified saved place;
Manual uses only a validated verified place. Restart preserves the mode and
fallback. A material point or mode change increments generation and immediately
publishes explicit resets for current, forecast, and map projections so the old
location cannot remain visible.

State reads are byte-bounded, duplicate-key rejecting, regular-file-only, and
opened without following a final symlink. Writes use a 0600 temporary,
file-fsync, atomic rename, and parent-directory fsync.

## Verification

- BigBoy `.130`, slot `func017-effective-location-r1`:
  `cargo test --locked -p mackesd --lib weather_location -- --nocapture`
  passed 5/5 after the no-follow read hardening, with 4,384 filtered out.
- The suite covers fresh-fix preference, verified fallback, restart, stale and
  wrong-host refusal, replay/generation refusal, persistence failure, and
  projection clearing.
- `git diff --check` passed. Package-wide formatting still reports unrelated
  pre-existing crate formatting debt; no package-wide formatting pass is
  claimed.

## Remaining acceptance gap

The network current/forecast provider, map fields, Maps UI, launcher, offline
maps/routes, MG90 hardware recovery, and live-seat proof remain. FUNC-017 stays
`Remaining`.
