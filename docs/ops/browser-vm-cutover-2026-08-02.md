# Browser VM cutover evidence — 2026-08-02

This note records the implementation slice for WL-ARCH-008. It is evidence,
not a second active worklist.

## Current state — 2026-08-03

The source-side cutover and standalone provenance are current through
`3f97dd56`; the extraction verifier passes for 84 paths and the public
standalone repository is at `996d3d27cfc4c52776c2289a0069d92e2bede66d`. A
corrected guest image containing the explicit `Virtual-1` output configuration
was built on farm `.90`, passed static image verification, and has SHA-256
`99614eaace96365fe4527ad65e3239f76923cb58905c9580b0af266aa40ba7e0`.

It has not been installed on Dell: `172.20.146.225` currently returns no route
and overlay `10.42.0.4` times out. Seat 15 (`172.20.0.15`) is reachable but
has no libvirt domain or RDP/SPICE/VNC listener. Therefore no current live
guest-frame, focused-input, GPU-video, guest-audio, reconnect, performance, or
six-node acceptance claim is made here.

The Browser shell's RDP auth path is now fail-closed and deployable: broker
authorization remains on the typed session record, while xrdp guest login is
resolved separately from a remembered sealed credential or a one-time masked
prompt. A bare mesh identity can no longer reach the RDP decoder and fail later
without an actionable login path. Farm `.130` passed the focused auth module
(20 tests, including the Browser VM regression), and the Browser resume seam
now re-arms the typed VDI handoff after an explicit return to shell chrome.
The VDI seam now exposes bounded local metrics for frame cadence, full/partial
uploads, upload timing, reconnects, and shell repaints; the focused metrics
test and the 64-test VDI regression set pass on BigBoy. These measurements do
not stand in for the still-missing guest GPU, audio, or live endpoint evidence.

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
- The serving broker now consumes an explicit `DesktopSessionProfile::BrowserVm`
  on the shared `SessionRequest::Open` wire type. It discovers the guest IPv4
  from libvirt's agent/lease tables, probes guest xrdp on TCP 3389, and relays
  private libvirt addresses over the serving peer's Nebula address without
  placing credentials in the mesh record. RDP falls back to the native SPICE
  console when the guest endpoint is unavailable; dead relays and previously
  unavailable records are retried by the broker's normal refresh loop.
- The Browser shortcut now resolves optional remembered guest credentials through
  the Chooser's existing sealed-credential seam before constructing the typed VDI
  request. The credential remains in the in-memory `DesktopAuth` only; Workloads
  and broker records still carry no password. With the honest production store
  still gated, a missing credential remains an explicit RDP readiness gate.

## Farm evidence

The next guest-image slice is now explicit in source: the Browser VM profile
selects RDP as its default transport with the retained SPICE compatibility
path, installs Fedora `xrdp`/`xorgxrdp`, QEMU guest-agent, VA-API diagnostics,
Mesa userspace, and
the PipeWire/ALSA bridge, and routes authenticated xrdp sessions through the
guest-owned Sway/Chromium runtime. Sunshine/Moonlight is intentionally not
advertised until a guest endpoint and host decoder exist. The focused Browser
VM contract verifier passes locally; the image itself has not yet been rebuilt
with this payload. Fedora 44's enabled repositories do not provide
`mesa-va-drivers`, so GPU video decode remains an explicit live gate rather
than an image claim.

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
- A clean standalone clone from public `main` also passed the locked root
  workspace test; the native helper checks and strict clippy gates passed on the
  farm after the initial offline cache miss for `tokio-macros` was resolved by a
  network-enabled locked run.
- `verify-browser-extraction.sh --check` passed for 85 paths: 18
  browser-owned, 25 mixed-purpose, and 42 shared.
- The checked-in Browser VM image contract has not yet been rebuilt with this
  payload. The current farm rebuild attempt reached the supported builder but
  stopped at the unavailable `quay.io/fedora/fedora-bootc:44` base registry;
  its temporary farm workspace and RPM were removed. The existing Pixman
  candidate at `/home/mm/browser-vm-chromium-qcow2-pixman-20260802/qcow2/disk.qcow2`
  has SHA-256
  `f7376cb8892cec011ca5c8651e5fa68e0a3a1ba7607df5694bbfb84a1a09ff1a` and is
  retained as prior evidence only; it must not be treated as proof of the
  current RDP/PipeWire guest payload.

## Historical Dell live evidence (not current)

- Dell (`172.20.146.225`) is running the persistent `browser-vm` libvirt domain
  from `/var/lib/libvirt/images/browser-vm-chromium-pixman-v7.qcow2`, with the
  normal cloud-init seed at
  `/var/lib/libvirt/images/browser-vm-cloudinit-pixman-final-20260802.iso` and a QXL SPICE video
  device. The domain has 4 vCPUs, 8 GiB RAM, the virtio input devices, and
  SPICE on Dell loopback `127.0.0.1:5900`.
- Dell's QEMU display backend has no OpenGL support, so the domain deliberately
  remains on QXL/SPICE and the guest runtime exports `WLR_RENDERER=pixman`.
- A persistent `virtio` sound device was attached to the domain configuration,
  then the VM was power-cycled to apply it. Post-restart QEMU inspection showed
  `virtio-sound-pci` wired to the SPICE `audio1` backend, and a fresh QEMU
  capture still showed the Chromium New Tab page. The post-change capture has
  SHA-256 `3516b6e5a0233721921d618c0d5d54bc91c6e1381e68a22c4d43acaf2d4758e`.
  This proves device attachment and rendering continuity, not guest audio
  playback.
- After correcting the NoCloud image digest to the full 64-hex artifact digest,
  a fresh QEMU capture reached a 1920x1080 Chromium New Tab page. Capture
  SHA-256: `fd57e470c45cf0ea4e9f05bb284b3b08612aa021debafd8ec8525e52e9479eaf`.
- The farm `mde-vdi-spice` live test connected through the SSH-forwarded Dell
  SPICE console and passed the decoded-frame/input gate: `1024x768`, frame
  FNV-1a64 `0xface601842022325`, input observation `echoed`, source commit
  `71c19235319c89c41eb8888d4804442a7860465c`. The bounded evidence record is
  `/tmp/browser-vm-spice-proof-final-20260802.json` with probe-log SHA-256
  `954e41154795e9f861ffbc230617484ab000fa0ca7ba9a29de3ae2d61fb4fd73`.
- After the virtio-sound change and VM power-cycle, the same farm runner was
  rerun from source commit `d1cb14d28843c6e9e886a8c51712c9dabf6106de` through
  the forwarded Dell console. It again passed the decoded-frame/input gate:
  `1024x768`, frame FNV-1a64 `0xface601842022325`, and input observation
  `echoed`.
- Dell now routes the running domain's `virtio-sound-pci` device through
  QEMU's named Pulse backend (`MCNF-Browser-VM`) to a localhost-only
  PipeWire-Pulse endpoint at `127.0.0.1:4713`. The endpoint is kept alive by
  the user service `mcnf-qemu-pulse-endpoint.service` and survived a
  `pipewire-pulse` restart; the `qemu` service account authenticated and a
  `MCNF-Browser-VM` sink-input appeared in the Dell mixer during a bounded
  playback probe. This proves the host audio route, not Chromium guest
  playback from the stale image.
- The live proof runner now forwards only the approved VDI target variables
  through `xcp-build.sh`; its route self-test and the source-bound run pass.
- Farm `.50` passed 26 focused `mackesd` console-broker tests, including the
  typed Browser profile selecting guest RDP, private guest-address relay, and
  healthy-relay heartbeat preservation. Farm `.90` passed 9 shared
  `mackes-mesh-types` VDI-session tests, including the explicit
  `profile:"browser_vm"` wire shape.
- On farm `.50` slot `browser-rdp-shell-20260802`, the updated shell binary
  passed 5 focused Browser surface tests and 5 focused brokered VDI protocol
  resolution tests, including authoritative broker protocol selection and
  old-record compatibility.
- Seat 15 (`172.20.0.15`) remains untouched; its low free-space condition makes
  it unsuitable for this image without operator capacity work.

## Still unproven

The host cutover, prior qcow2 provenance, Chromium guest capture, decoded SPICE
pixels, focused-input visual echo, and the Dell host PipeWire audio route are
evidenced. The current Dell domain is still QXL/SPICE and is not the newly
rebuilt RDP image. RDP endpoint publication/authentication and live proof, GPU
video, guest Chromium audio playback, five-session performance,
signed publication/install/upgrade, reconnect/failover,
and six-node acceptance remain open. The image rebuild is also blocked by the
unavailable Fedora base registry; separately, Fedora 44's enabled repositories
lack the multimedia RPM dependencies required by the repository-only image
lane. Seat 15 still needs operator capacity work. This change does not claim
that the Chromium App VM is production-ready. The broker and wire gates are
green; the live guest remains not ready for RDP because Dell currently refuses
TCP 3389 and the running image is the stale Pixman/QXL candidate.
