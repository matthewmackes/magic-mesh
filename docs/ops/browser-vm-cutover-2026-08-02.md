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
- The dedicated Fedora 44 Browser VM image was rebuilt and statically verified
  on BigBoy from the `magic-mesh-12.1.6-1.x86_64.rpm` artifact. The final
  candidate is profile `browser-vm-chromium-v1`, with qcow2 SHA-256
  `ec1958b0dfaaccbed63348c45afa5ff68951aa63034fa5ba1d13bc518e8581a7` and
  base-image digest
  `sha256:307de440d3381256a6f7072755ad340d428a0e43b6a83823d120c6c774a4d5e7`.
  It includes cloud-init and seatd, disables weak workstation dependencies,
  explicitly excludes `magic-mesh-browser`, and the static verifier found
  Chromium, Sway, Wayland, Mesa, PipeWire/WirePlumber, libinput, the guest
  runtime, and the host-Browser prohibition contract.
- `bootc-image-builder` produced the final qcow2 with an explicit XFS rootfs
  on BigBoy at `/home/mm/browser-vm-chromium-qcow2-v6/qcow2/disk.qcow2`; it is
  a 10 GiB virtual disk with a 1.7 GiB sparse payload. The explicit rootfs is
  required because Fedora 44's local image metadata does not provide
  `DefaultRootFs` to the current builder.

## Dell live evidence

- Dell (`172.20.146.225`) is running the persistent `browser-vm` libvirt domain
  from `/var/lib/libvirt/images/browser-vm-chromium-v6.qcow2`, with the v6
  cloud-init seed updated for the final image digest and a QXL SPICE video
  device. The domain has 4 vCPUs, 8 GiB RAM, the virtio input devices, and
  SPICE on Dell loopback `127.0.0.1:5900`.
- A QEMU framebuffer capture from the running guest is 1920x1080 and visibly
  shows guest Chromium's New Tab page, focused omnibox, tabs, and browser
  chrome. The capture is the first positive guest Chromium render evidence;
  it is not being used as a substitute for the VDI decoder gate.
- The farm `mde-vdi-spice` live test connected through an SSH-forwarded Dell
  SPICE console and emitted `live: FRAME OK 1024x768 fnv1a64=0x3fe01b4acfe22325`.
  The pinned `spice-client 0.2.0` then panicked in its display draw-copy
  bounds arithmetic (`display.rs:941`, subtraction overflow) before the test
  could complete input echo. This leaves the SPICE input/reconnect proof open.
- Seat 15 (`172.20.0.15`) remains untouched; its low free-space condition makes
  it unsuitable for this image without operator capacity work.

## Still unproven

The host cutover, dedicated image/qcow2 build, Dell boot, guest Chromium
framebuffer, and first SPICE frame are evidenced. Signed publication/install,
completed focused input, reconnect, GPU video, PipeWire audio, performance,
and six-node acceptance remain open. The SPICE decoder panic is the immediate
transport blocker, and Seat 15 still needs operator capacity work; this change
does not claim that the Chromium App VM is production-ready.
