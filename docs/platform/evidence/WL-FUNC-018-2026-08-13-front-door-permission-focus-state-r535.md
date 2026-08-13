# WL-FUNC-018 — Front Door permission, progress, focus, and failure state (r535)

Date: 2026-08-13

## Implemented behavior

The Front Door now treats a catalog-backed App VM launch as an explicit typed
permission transition rather than a one-click launch:

- the first activation arms approval for the exact serving node, Flatpak ID,
  signed catalog revision, guest profile, and requested capability set;
- a changed catalog declaration cannot inherit that approval;
- only a second confirmation emits `LaunchPeerApp`, which continues through the
  typed App VM/Workloads path added by `29a59021`;
- connected or paused rows focus the existing VDI desktop source instead of
  provisioning a duplicate guest;
- installing, placement, startup, reconnecting, denied, stale, unsigned,
  unavailable, and failed states remain visibly truthful and non-actionable;
- approval is ephemeral and is cleared on query, filter, selection, panel-close,
  or target changes. No shell command, host Flatpak, or backend I/O was added to
  the render path.

Owned production file:

- `crates/desktop/mde-shell-egui/src/front_door.rs`

## Farm evidence

- XEN-BIGBOY `172.20.0.130`, slot
  `func018-frontdoor-ux-build-r1`: `cargo build -p mde-shell-egui` passed.
- XEN-196 `172.20.0.196`, slot
  `func018-frontdoor-ux-clippy-r1`:
  `cargo clippy -p mde-shell-egui --all-targets -- -D warnings` passed.
- XEN-HOME-SERVICES `172.20.0.50`, slot
  `func018-frontdoor-ux-test-r1`: focused exact behavior test passed 1/1
  (`app_vm_launch_requires_exact_permission_confirmation_and_focuses_existing_session`).
- XEN-HOME-SERVICES `172.20.0.50`, slot
  `func018-frontdoor-ux-fmt-r2`: Rust 1.94 exact-file `rustfmt --check` passed.
- Local scoped `git diff --check` passed; no local build/test command was used.

The initial focused command used `--exact` without the Rust module path and
therefore selected zero tests; it was not counted as evidence. The corrected
focused run above selected and passed the intended test.

## Residual WL-FUNC-018 acceptance

- Bind the governed current App VM image/profile and approved Flatpak runtime
  supply into the first full release.
- Finish any remaining truthful readiness/audio/persistence/stop/crash/reconnect
  and cleanup coding gaps found by the final pre-release audit.
- After the first full release, perform the deferred non-blocking one-node live
  App VM, sandbox/SELinux, package, persistence, VDI input/audio, and recovery
  acceptance with no host application installation.
