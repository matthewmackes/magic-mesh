# WL-ARCH-008 — Browser VM focused-input authority (r536)

Date: 2026-08-13

## Production result

The Browser VM's VDI framebuffer is now the explicit authority for guest input.
Keyboard, text, IME, and clipboard shortcut events enter guest Chromium only
while the framebuffer has egui focus. Pointer presses must begin inside the
framebuffer. An admitted press captures pointer motion and its matching release
outside the framebuffer so Chromium cannot retain a stuck button, while
unrelated shell-chrome clicks, hover motion, and wheel input remain in Construct.

Presentation revocation clears pointer capture alongside keyboard/input
authority. A stale frame retained across reconnect, resize, or Workloads lease
replacement therefore cannot retain control of the replacement Browser
transport.

This is Browser VM presentation glue over the existing VDI transport. It adds
no host Browser path, guest chrome, lifecycle authority, or compatibility
fallback.

## Farm gates

- `.50`, slot `arch008-browser-focused-input-r536`:
  `cargo test -p mde-shell-egui --features live-vdi guest_input_requires_framebuffer_ownership_and_releases_pointer_capture -- --nocapture`
  passed 1/1.
- `.170`, slot `arch008-browser-input-revoke-r536`:
  `cargo test -p mde-shell-egui --features live-vdi revoked_browser_presentation_drops_pointer_capture -- --nocapture`
  passed 1/1.
- BigBoy `.130`, slot `arch008-browser-focused-input-clippy-r536`:
  `cargo clippy -p mde-shell-egui --all-targets --features live-vdi -- -D warnings`
  passed.
- `.196`, slot `arch008-browser-focused-input-fmt-r536`:
  `cargo fmt -p mde-shell-egui -- --check` passed.
- `git diff --check` passed.

Every owned disposable farm workspace was ownership-checked and removed after
its command completed. Concurrent worktree scopes were preserved.

## Residual ARCH-008 work

Pre-release coding remains for the reproducible signed Browser guest image and
readiness artifacts, any still-missing portable migration edge discovered by a
final S1-S5 audit, and remaining Browser-specific audio/reconnect/preference
integration gaps not already covered by generic VDI authorities.

Post-release proof remains separate and non-blocking for this coding slice:
audible guest playback, five-tab cadence and damage/latency measurements,
package cleanup/upgrade, corrected-forward recovery, and selected-seat live VDI
captures proving no host Browser helper exists.
