# Browser VM cutover evidence — 2026-08-02

This note records the implementation slice for WL-ARCH-008. It is evidence,
not a second active worklist.

## Landed

- `magic-mesh-browser-stack` is public and history-bearing at
  `https://github.com/matthewmackes/magic-mesh-browser-stack`.
- Standalone publication provenance is recorded in commit `c307da31`.
- Its admitted root workspace contains `mde-web-wire` and `mde-adblock`, has a
  clean-clone boundary verifier and CI, and has no Cargo path/git dependency
  back to `magic-mesh`.
- Construct commit `95f3ad21` makes Browser consume the folded Workloads row
  named `browser-vm` (`DesktopVm`, running, reachable), publish the normal typed
  `action/vdi/session` open, and hand the session to the existing VDI decoder.
  Once a VDI target is requested, `Surface::Browser` paints the VDI framebuffer
  and forwards its focused input; it no longer paints a host page surface.
- Commit `ae2c14e0` refreshes the extraction manifest and classifies the
  Browser VM profile/validation files as retained shared guest-boundary
  contracts.

## Farm evidence

On BigBoy (`172.20.0.130`):

- `cargo check -p mde-shell-egui --features live-vdi` passed.
- `cargo test -p mde-shell-egui --features live-vdi web` passed: 6 Browser
  tests, including Workloads wait, typed VDI handoff, and no-host-runtime UI.
- The standalone boundary verifier, `cargo test --workspace --locked` (48
  tests), and `cargo clippy --workspace --all-targets --locked -- -D warnings`
  passed in the standalone farm workspace.
- `verify-browser-extraction.sh --check` passed for 168 paths: 92
  browser-owned, 33 mixed-purpose, and 43 shared.

## Still unproven

The public repository currently admits a narrow dependency-complete root; the
native CEF/Servo helper workspaces, full old-stack extraction, Browser VM image,
live Chromium framebuffer, focused input, reconnect, GPU video, PipeWire audio,
performance, and six-node acceptance remain open. The 2026-08-02 seat audit
still found no VDI listener on seat 15 or Dell, so this change does not claim
that the Chromium App VM is production-ready.
