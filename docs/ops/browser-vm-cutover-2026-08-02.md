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
- `verify-browser-extraction.sh --check` passed for 85 paths: 18
  browser-owned, 25 mixed-purpose, and 42 shared.
- The dedicated Fedora 44 Browser VM image was rebuilt and statically verified
  on BigBoy from the local `magic-mesh-12.1.6-1.x86_64.rpm` artifact. The
  Pixman candidate is profile `browser-vm-chromium-v1`, with build qcow2
  SHA-256
  `f7376cb8892cec011ca5c8651e5fa68e0a3a1ba7607df5694bbfb84a1a09ff1a`.
  It includes cloud-init and seatd, disables weak workstation dependencies,
  explicitly excludes `magic-mesh-browser`, and the static verifier found
  Chromium, Sway, Wayland, Mesa, PipeWire/WirePlumber, libinput, the guest
  runtime, and the host-Browser prohibition contract.
- `bootc-image-builder` produced the Pixman qcow2 with an explicit XFS rootfs
  on BigBoy at `/home/mm/browser-vm-chromium-qcow2-pixman-20260802/qcow2/disk.qcow2`; it is
  a 10 GiB virtual disk with a 1.7 GiB sparse payload. The explicit rootfs is
  required because Fedora 44's local image metadata does not provide
  `DefaultRootFs` to the current builder.

## Dell live evidence

- Dell (`172.20.146.225`) is running the persistent `browser-vm` libvirt domain
  from `/var/lib/libvirt/images/browser-vm-chromium-pixman-v7.qcow2`, with the
  normal cloud-init seed at
  `/var/lib/libvirt/images/browser-vm-cloudinit-pixman-final-20260802.iso` and a QXL SPICE video
  device. The domain has 4 vCPUs, 8 GiB RAM, the virtio input devices, and
  SPICE on Dell loopback `127.0.0.1:5900`.
- Dell's QEMU display backend has no OpenGL support, so the domain deliberately
  remains on QXL/SPICE and the guest runtime exports `WLR_RENDERER=pixman`.
- After correcting the NoCloud image digest to the full 64-hex artifact digest,
  a fresh QEMU capture reached a 1920x1080 Chromium New Tab page. Capture
  SHA-256: `fd57e470c45cf0ea4e9f05bb284b3b08612aa021debafd8ec8525e52e9479eaf`.
- The farm `mde-vdi-spice` live test connected through the SSH-forwarded Dell
  SPICE console and passed the decoded-frame/input gate: `1024x768`, frame
  FNV-1a64 `0xface601842022325`, input observation `echoed`, source commit
  `71c19235319c89c41eb8888d4804442a7860465c`. The bounded evidence record is
  `/tmp/browser-vm-spice-proof-final-20260802.json` with probe-log SHA-256
  `954e41154795e9f861ffbc230617484ab000fa0ca7ba9a29de3ae2d61fb4fd73`.
- The live proof runner now forwards only the approved VDI target variables
  through `xcp-build.sh`; its route self-test and the source-bound run pass.
- Seat 15 (`172.20.0.15`) remains untouched; its low free-space condition makes
  it unsuitable for this image without operator capacity work.

## Still unproven

The host cutover, dedicated image/qcow2 provenance, Chromium guest capture,
decoded SPICE pixels, and focused-input visual echo are evidenced. RDP/Sunshine
endpoint implementation and live proof, GPU video, PipeWire audio,
five-session performance, signed publication/install/upgrade, reconnect/failover,
and six-node acceptance remain open. The image was built through the supported
local-RPM lane because the Fedora 44 repository lane lacks the multimedia RPM
dependencies required by the image build. Seat 15 still needs operator capacity
work. This change does not claim that the Chromium App VM is production-ready.
