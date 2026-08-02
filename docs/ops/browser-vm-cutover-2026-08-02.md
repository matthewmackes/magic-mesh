# Browser VM cutover evidence — 2026-08-02

This note records the implementation slice for WL-ARCH-008. It is evidence,
not a second active worklist.

## Landed

- `magic-mesh-browser-stack` is public and history-bearing at
  `https://github.com/matthewmackes/magic-mesh-browser-stack`.
- Standalone publication provenance is recorded through commit `3b06da0d`.
  The public root workspace contains the extracted wire, policy, preview
  client, worker core, Bus, seal, mesh-type, and Browser worker crates; the
  separately locked native helper roots are checked without a `magic-mesh`
  checkout.
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
- The standalone boundary verifier and full `cargo test --workspace --locked`
  passed in the standalone farm workspace.
- `cargo clippy --workspace --lib --locked --offline -- -D warnings` and
  `cargo clippy -p mde-web-preview-client --all-targets --locked --offline --
  -D warnings` passed in that same workspace.
- On the same BigBoy clone, `mde-web-sandbox`, `mde-web-cef`, and
  `mde-web-preview` each passed locked offline `cargo check`; the client’s 87
  tests and strict clippy gate passed.
- `verify-browser-extraction.sh --check` passed for 168 paths: 92
  browser-owned, 33 mixed-purpose, and 43 shared.
- The dedicated Fedora 44 Browser VM image was built and statically verified on
  BigBoy from the `magic-mesh-12.1.6-1.x86_64.rpm` artifact. The image carries
  profile label `browser-vm-chromium-v1`, image digest
  `sha256:efe44106db4963b8b1aef9af0f3106905a89b964de032096bcabd9827e549767`,
  and base-image digest
  `sha256:307de440d3381256a6f7072755ad340d428a0e43b6a83823d120c6c774a4d5e7`.
  The static verifier found Chromium (Fedora's `chromium-browser` launcher),
  Sway, Wayland, Mesa, PipeWire/WirePlumber, libinput, the guest runtime, and
  the host-Browser prohibition contract.
- `bootc-image-builder` produced the qcow2 with an explicit XFS rootfs on
  BigBoy at `/home/mm/browser-vm-chromium-qcow2/qcow2/disk.qcow2`; size
  `2251755008` bytes and SHA-256
  `d00d5c8a4c35a3134d0e5f8c4a95ea31f6744e63aa820e849f3c1630b8d0fd72`.
  The build script now supplies that explicit rootfs because Fedora 44's local
  image metadata does not provide `DefaultRootFs` to the current builder.
- Dell (`172.20.146.225`) now has the verified artifact staged at
  `/var/lib/libvirt/images/browser-vm-chromium.qcow2`; the remote checksum
  matches, `qemu-img info` reports a non-corrupt qcow2 with a 10 GiB virtual
  size, and no Browser VM domain was created yet. Seat 15 was not modified:
  its root filesystem had only 1.5 GiB free, below the 2.25 GB artifact size.

## Still unproven

The standalone worker-family extraction and the dedicated image/qcow2 build are
complete, but signed publication/install, live Chromium framebuffer, focused
input, reconnect, GPU video, PipeWire audio, performance, and six-node
acceptance remain open. The 2026-08-02 seat audit still found no VDI listener
on seat 15 or Dell, so this change does not claim that the Chromium App VM is
production-ready.
