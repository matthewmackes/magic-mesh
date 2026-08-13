# WL-UX-009 status-bar semantic palette (r509)

Date: 2026-08-13

## Implementation

`crates/desktop/mde-shell-egui/src/status_bar.rs` now resolves the top status
bar's background, border, text, and hover tokens through the active Quazar
appearance instead of painting Dark constants into Light mode. The admitted
weather projection also carries a typed `Live` / `Stale` / `Unavailable` tone.
Stale and unavailable observations use shared quiet/disabled semantics in the
top rail and bounded taskbar-safe variants over the intentionally black bottom
taskbar, so provider loss cannot retain the live visual tone.

This changes presentation only. Weather freshness and authority continue to
come from the existing validated off-render projection, and no render-path I/O
or alternate weather state was introduced.

## Farm evidence

- BigBoy `172.20.0.130`, slot `ux009-status-tone`:
  `cargo test -p mde-shell-egui stale_and_unavailable_weather_never_retain_the_live_status_tone -- --nocapture`
  passed 1/1 with 1,585 filtered tests.
- `172.20.0.170`, slot `ux009-status-clippy`:
  `cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings` passed.
- `172.20.0.50`, slot `ux009-status-fmt`:
  `rustfmt --edition 2021 --check crates/desktop/mde-shell-egui/src/status_bar.rs`
  passed.
- `git diff --check` passed in the integration worktree.

## Remaining epic acceptance

WL-UX-009 still requires the complete Construct surface inventory/migration,
the first full release payload report, and the deferred post-release
Dark/Light/narrow/largest-text direct-DRM capture and human-review matrix. This
slice does not claim those release or live-seat criteria.
