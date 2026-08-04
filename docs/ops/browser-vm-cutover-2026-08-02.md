# Browser VM cutover evidence — 2026-08-02

This note records the implementation slice for WL-ARCH-008. It is evidence,
not a second active worklist.

## Current state — 2026-08-03

Dell's 21:01 EDT physical-seat alert retest passed. The target publish is audit
ULID `01KZ54K5V2763Z65AXS8PN9TDN`; the installed warning helper held the
mutation gate for 5.320 seconds. A CPU-linear EGL readback of the exact live
1366x768 DRM frame shows the red rounded `AI-GENERATED-ALERT` card at
`460x224+453+272`: its horizontal center is 682.5 px, exactly matching the
display center, and all copy and the Dismiss control remain contained. The PNG
SHA-256 is `5eb1be7bebf1d9443af809b604a6916039f39bc588761eb5ffab1be006697a22`.
A concurrent PipeWire monitor observed the shell-created `pw-play` node, both
stereo output ports, and links into Dell's active ALSA sink. The earlier 20:59
readback was rejected because it captured the boot splash before alert
admission. After the accepted proof, the proof-only environment was cleared,
the temporary guest SSH key and packet-key diagnostics were removed, the normal
shell returned active as PID 363398 with zero restarts, and the Browser VM RDP
endpoint remained reachable.

Seat-15 lock-layer proof at 16:59 EDT showed that commit `29e3951b` successfully
moved `AI-GENERATED-ALERT` above the lock curtain, but the same live capture
rejected the presentation: the centered red card's headline was translated
left and escaped its bounds. The failed-acceptance PNG SHA-256 is
`7340d2ef9b85c468be6c188dc78535b6667b80de6d78a3f6de28fa7550b06222`, and
live receipt `01KZ4PT1QW367W31BSDZ933WM5` carries the exact body shown in it.
Commit `18a1dfab` removes the duplicate half-width translation and adds a
containment regression; all 279 focused `mde-egui` tests pass. The corrected
second physical readback at 17:41 EDT passes presentation acceptance: the red
rounded card is centered above the lock curtain and the centered
`AI-GENERATED-ALERT` headline is fully contained. Its PNG SHA-256 is
`9a67da3593a91cd2580bf816e920a2ada08d1f1dd847e58556cf7fad8d9c0ad2`, and
capture receipt `01KZ4S66889QKKMS8PQ13YAZF6` carries the exact rendered body.
Seat 15 now runs corrected binary SHA-256
`5f2f1a9822d5c9e8ce5b5d220da6e71c1fc82eae65e122be35563d975a2f03ce`.
Cleanup receipt `01KZ4S94E48T9FHC0B35D2Z1DW` preceded removal of the temporary
proof hook and final restart by the full five seconds; the normal shell is
active as PID 132767 with zero restarts. The installed binary can be rolled
back from
`/var/lib/mcnf-release-backups/seat15-alert-headline-20260803T214040Z/`.

Dell's post-reboot alert preemption retest at 16:27 EDT published receipt
`01KZ4MX1CN3FM5A5MY3K4XNDMH` from production-feature shell SHA-256
`00861ae83731d8498f85db8631d4ccff79adef7d55e07cbc3b2c1c67592441be`.
Three persistent system-Critical receipts for Eagle, Surface, and seat 15 were
immediately ahead of it on the live alert lane. Commit `880814fa` makes the
deployment notice preempt any current alert while preserving the displaced
Critical to resume; all 277 `mde-egui` tests pass, including that exact queue
case and the centered/constrained geometry. The shell remained active with PID
3369 and zero restarts, while `browser-vm` and RDP remained reachable. This
proves the live delivery and preemption path; physical screen presentation
still requires operator observation.

Dell alert retest at 12:16 EDT published urgent receipt
`01KZ46JC1GQYBKP69G797ZF58J` with the exact `AI-GENERATED-ALERT` flag and
headline `DELL ALERT RETEST: centered red alert and sound.` The receipt is
present verbatim in Dell's local Bus history. A concurrent live PipeWire
monitor observed the shell-created `pw-play` WAV node and both `output_FL` and
`output_FR` ports; the shell remained active as PID `320874` with zero
restarts. This proves local receipt consumption and routing into Dell's real
audio graph. Physical screen presentation and speaker audibility still require
operator confirmation.

Dell alert retest at 11:56 EDT: the shell now includes a deterministic built-in
notification chime fallback and is running production-feature binary SHA-256
`51db5548780f342eb5450a28e464a7a39a5cc3b13dcad66583d51edb8cdada52`
as PID `320874`, active with zero restarts. Before replacement, the exact
five-second update notice was published as receipt
`01KZ45ECRS3MN66B6TDMWYA6R9`; the helper completed its full delay. The prior
binary (SHA-256 `f9e2cac14ffbf264d69f28a4eeb8292b85338dc0a7ff53de05581292bd1d2b3c`)
is recoverable at
`/var/lib/mcnf-release-backups/dell-chime-20260803-8605d708/mde-shell-egui`.

The post-update Critical retest is receipt
`01KZ45GCJZEZXXVPQBQS4GCYK9`, with exact flag `AI-GENERATED-ALERT` and
headline `DELL FINAL RETEST: red alert and sound are active.` While that receipt
was consumed, a live PipeWire monitor observed the shell fallback's `pw-play`
node and both `output_FL` and `output_FR` ports. This proves the alert reached
the real seat audio graph; operator confirmation is still required for the
physical screen presentation and speaker audibility. The alert receipt itself
is present and exact. A concurrent whole-Bus integrity probe was not globally
green: it reported one file without a database row on `state/boot-readiness`
and one database row without a file on `state/host/local/seat`. Those unrelated
lane mismatches were not repaired during this alert cutover, and no clean
whole-Bus claim is made from that probe.

Latest live state at 10:38 EDT: Dell is running the Fedora 44
production-feature shell built natively on `.131` with
`drm,live-vdi,media-mpv`. The installed `/usr/bin/mde-shell-egui` SHA-256 is
`8560999ebec43d0738c55ee5223d9e7afb3d68fcf7ca2ed8dfed0a7fb5cbddc5`;
`mde-shell-egui.service` is active with zero restarts and
`XDG_RUNTIME_DIR=/run/user/1000`. The prior binary and unit are recoverable at
`/var/lib/mcnf-release-backups/dell-r1-20260803-8560999e`.

Dell received the mandatory update warning as Bus receipt
`01KZ40SV1388BQ6V5FBGA0R7BB`, followed by the full five-second delay before
any release file changed. The new shell then consumed persistent Critical test
receipt `01KZ40VHZ90NE068QC6EQXTW0G`, whose exact flag is
`AI-GENERATED-ALERT`; it requests the dedicated red, centered, constrained
card and stays visible until acknowledged. The alert's focused farm tests pass
exact-flag routing, operator-alert preemption, and wide/narrow centered
geometry. Dell's root shell can also open the primary seat user's real
PipeWire graph: a live `pw-cat` probe linked to both ALC3246 playback channels,
and `/usr/share/sounds/alsa/Front_Center.wav` completed with exit zero.
Audibility still needs the operator's physical confirmation.

The `browser-vm` libvirt domain is running and QEMU guest-agent responds. The
next mutation is deliberately held behind acknowledgment of the persistent
Dell test card so the credential rotation's exact five-second warning is the
visible foreground alert. The host-bound RDP credential is therefore not yet
installed, the desired Workloads row is not yet admitted, and no RDP
frame/input/reconnect claim is made. After acknowledgment, the ordered R1 path
is credential rotation, authorized `browser-provision`, typed RDP attachment,
then Chromium frame/input/audio/reconnect proof.

Earlier live state at 07:59 EDT: the native Fedora 44 review RPM completed,
passed its real-payload and size gates, and was installed by digest on T480,
Eagle, seat 15, Dell, and the newly onboarded Microsoft Surface Pro 6. Surface
is now the healthy Workstation `SURFACE` at overlay `10.42.0.7`. T480's
separate old-mesh identity was archived and cleanly re-enrolled into the current
mesh as `peer:T480` at `10.42.0.8`; its etcd-client, Syncthing, role services,
and QEMU/KVM runtime are active. Eagle's missing QEMU/KVM runtime was also
installed. All three lighthouses then reported eight healthy nodes and HA
green. This closed the seat-package and Surface/T480 enrollment prerequisites,
not Browser VM acceptance. Full evidence and exact artifact digests are in
[`f44-seat-rollout-surface-2026-08-03.md`](f44-seat-rollout-surface-2026-08-03.md).

## Earlier progression

The entries below preserve intermediate observations in chronological build
order. When an older reachability or fleet statement conflicts with the
timestamped current state above, the current state is authoritative.

Mesh recovery evidence at 05:05 EDT: the three reachable public lighthouses
(`104.236.118.177`, `46.101.219.245`, and `64.23.131.57`) had regressed to
self-only Nebula bundles. I backed up each bundle, restored the known three-
lighthouse public roster and peer records, and promoted the tested
`magic-mesh-lighthouse-12.1.6-1.x86_64` package on all three nodes. All three
`mackesd`, Nebula, and etcd services are active; `etcdctl endpoint health
--cluster` reports all three endpoints healthy, with a three-member quorum and
`10.42.0.2` leader. Seat-15 now has Nebula `10.42.0.5`, reaches all three
lighthouses, and can query etcd health. This clears the mesh-control-plane
blocker, but it does not create a VM on seat-15 or Dell.

At the 05:05 checkpoint, the Dell overlay target `10.42.0.4` still dropped ping
and timed out on SSH, RDP, SPICE, and VNC ports from the restored mesh; Dell
`.225` and `.2` were unavailable from the orchestrator. Seat-15 had no `/dev/kvm`, Browser VM
workload, or VDI listener. Therefore access to Chromium on Dell has no honest
time-of-day ETA yet: it begins after Dell is reachable and its host/KVM is
recovered, followed by image publication and live guest-frame/input checks.
A bounded seat-15 probe found the existing `qemu-img`, `virsh`, and
passwordless-sudo tools, but loading `kvm`/`kvm_intel` returned `Operation not
supported` and `/dev/kvm` remained absent. No package, domain, or image
installation mutation was attempted because this host is not exposing a
KVM-capable execution path.

Earlier rollout handoff at 07:05 EDT (source tree `a7b26cec`, superseded by the
latest state above): the farm/Fedora 42
RPM `magic-mesh-12.1.6-1.x86_64.rpm` was built on BigBoy, but its transaction
was rejected on T480 `.138`, Eagle `.145`, seat-15, and Dell because the
Fedora 42 multimedia sonames are not present on Fedora 44. No workstation
upgrade was accepted. The documented Fedora 44 builder `mcnf-build-f44` was
started on BigBoy after confirming the regular F42 builder had no active job;
the native F44 container build was then running at
`/home/mm/magic-mesh-farm-seat-fleet-f44-20260803` and has not produced an RPM
at that handoff point.

The current workstation rollout targets are T480 `.138`
(`172.20.146.138`), Eagle `.145` (`172.20.146.145`), seat-15
(`172.20.0.15`), Dell (`172.20.146.225`), and the separate Microsoft Surface
Pro 6 (`172.20.146.79`). All five now carry the same verified Fedora 44 review
RPM; Surface is enrolled separately and is not seat 15. No seat currently has
live Browser VM acceptance evidence.

Continuation evidence at 04:36 EDT: the current Fedora 44 workstation RPM was
built on BigBoy `.130` in slot `seat15-base-f44-20260803`, passed the 90-MiB
payload gate at 82.0 MiB, and was installed on seat-15 after a successful
read-only RPM transaction test. The artifact SHA-256 is
`6dcb49523fd4dfdb57e7467d0e6341c35aa67eff3937d70386c46825ddbeec67`; the
installed `/usr/bin/mackesd` SHA-256 is
`e5564cd5bec225d85719d05ed2b6f8d37cd5969a3241ce6275541f749583668c`.
`mackesd` and `nebula` are active after a bounded recovery start. The normal
dependency-ordered start remains degraded because
`mcnf-cloud-arm-credential.service` is waiting on unreachable etcd; the
credential file itself is present and mackesd is running with its credentials.
Fresh discovery still reports seat-15 reachable with no VDI endpoint, Dell
`.225`, `.2`, and overlay `.4` unavailable, and no Browser VM acceptance is
claimed.

Continuation evidence: commit `8bc40ea5` adds the credential-free
`packaging/browser-vm/deploy-image.sh` operator path. Its default preflight is
read-only; only an explicit `publish --apply` can upload and atomically install
the qcow2 after KVM, qemu-img, sudo, path, and digest checks. The package
contract, extraction manifest, and worklist self-tests pass locally and the
Browser contract passes on farm `.50`.

Earlier target inventory at that point was negative: seat-15 (`172.20.0.15`) was SSH
reachable with no VDI listener and no readable `/dev/kvm`; Dell
(`172.20.146.225`), its direct address (`172.20.146.2`), and overlay
(`10.42.0.4`) are unavailable. No image publication or live acceptance was
attempted against those targets.

Fresh seat-15 read-only diagnostics at 2026-08-03 03:36–03:39 EDT show the
mesh itself is degraded: `mackesd` timed out connecting to etcd at
`10.42.0.1:2379`, `10.42.0.2:2379`, and `10.42.0.3:2379`, and its Nebula
supervisor repeatedly reported that `nebula-lighthouse.service` was missing.
This is an operator/fleet recovery blocker in addition to the seat's missing
KVM and VDI endpoint; no service restart or network repair was attempted.

Recovery hardening added after that audit makes the optional lighthouse-unit
reload best-effort once the base `nebula.service` reload succeeds. The farm
`.90` `mackesd` regression
`missing_lighthouse_unit_does_not_block_base_config_acknowledgement` passes;
the fix still needs to be deployed and observed on seat-15 before it can count
as live mesh recovery evidence.

Continuation evidence: the current cutover commits are `7816f781` (typed
Browser image-source propagation, seat preflight/NoCloud preparation, and
RDPSND/PipeWire audio wiring) and `ef1dc5ed` (85-path extraction manifest
refresh). Farm verification passes `mackesd` cloud 236/236 and
`mde-vdi-rdp --features live-connect` 88/88 plus loopback tests. The audio
capability is intentionally endpoint/PCM evidence only; it does not claim
audible playback without a live host capture.

The guest-image artifact is bound to source commit `9e7697b5`; the source-side
cutover and VDI metrics are current through `25fb1cc1`. The extraction verifier
passes for 85 paths and the public
standalone repository is at `996d3d27cfc4c52776c2289a0069d92e2bede66d`. A
corrected guest image containing the explicit `Virtual-1` output configuration
was built on farm `.90`, passed static image verification, and has SHA-256
`99614eaace96365fe4527ad65e3239f76923cb58905c9580b0af266aa40ba7e0`.

A fresh current-source Fedora 44 Browser VM image was also built and statically
verified on BigBoy `.130`; its earlier qcow2 artifact is retained at
`/home/mm/browser-vm-chromium-qcow2-cutover-20260803/qcow2/disk.qcow2` with
SHA-256 `9c5d687c7fa378cb8cfe767bf0d46fb0f55a7889cf5c1eebc1bbfc8003a8c0c6`.
`qemu-img info` reports a non-corrupt 10-GiB qcow2 with a 1.72-GiB compressed
footprint. It is retained historical evidence only: the deployment preflight
now rejects it because the typed Browser VM contract requires a 64-GiB virtual
disk.

Image-contract evidence from that build sequence: the image builder binds its disk output to
`BROWSER_VM_DISK_GB=64` and resizes raw/qcow2 artifacts to that virtual size.
After the profile pin advanced in `3fbf2223` to corrected source
`dd973836` (which includes the guest-readable NoCloud permissions fix), a
matching qcow2 was rebuilt on farm `.90` at
`/home/mm/browser-vm-chromium-qcow2-final-64g-20260803/qcow2/disk.qcow2`.
Its SHA-256 is
`d41b322a658e02e7c4303a3d0580e7e702a4e39cb8e2889eb4ed2614c46a946b`.
`qemu-img check` reports no errors and `qemu-img info` reports a 64-GiB
virtual size with a 1.72-GiB compressed footprint. The image carries source
label `dd973836` (container image ID
`3282faa9a795df6750cd054e13fef2a612e616e3a111b2fe15b9980f8edf081a`); the old
10-GiB artifact is rejected by preflight. This remains the publish candidate.
It was uninstalled at that checkpoint because Dell was unreachable; Dell later
returned, as recorded in the current state, but live publication is still open.

The new `install-helpers/verify-browser-vm-performance.py` gate is now wired
into the Browser contract and farm-verified on `.50`. It accepts only a
source-commit/image-digest-bound live record covering five 1080p tabs for at
least 15 minutes, 30-FPS/no-500-ms-stall limits, pointer activity,
navigation/session latency, partial uploads, hidden repaint, and reconnect
recovery. Its self-test is not live evidence; no qualifying Dell or seat
performance record exists yet.

The OpenTofu deployment contract now rejects direct Browser VM declarations
that are not `desktop_vm`, 4 vCPU/8192 MiB/64 GiB, or bound to a full image
digest. Browser domains bind SPICE to loopback. Host-dependent virtio
3D/OpenGL is opt-in through `browser_gpu_acceleration`; the default is the
2D virtio compatibility overlay because Dell's historical QEMU backend had no
proven GL capability.

The bounded live-proof runner now also supports the Browser VM's primary RDP
path by invoking the existing ignored `mde-vdi-rdp` integration test. It records
only the initial guest `FRAME OK` marker, bounded input observation, explicit
tier-reconnect observation, and the probe-log digest; RDP credentials are
accepted only as process-local target input and are omitted from evidence. The
self-test and farm compile passed on
`.50` slot `vdi-proof-rdp-helper-20260803`, but no RDP target is currently
reachable to execute it.

The first bounded real-worker RDP attempt was run on `.50` against Dell
`172.20.146.225:3389` on 2026-08-03. It returned `failed` with worker exit
code `101` and no accepted framebuffer marker; the validated bounded record is
`/tmp/browser-vm-rdp-proof-20260803.json` (SHA-256
`f8ee07239ad8b4745f2b0ba1b09dcd4141ca094a2d2aca64ac537d1c1d7268d5`). This is
negative reachability evidence, not live guest readiness.

The rebuilt RDP image has not been installed on Dell. Dell is reachable again,
but its current `browser-vm` remains the historical QXL/SPICE domain rather
than the new RDP candidate. Seat 15 (`172.20.0.15`) is reachable but has no
usable `/dev/kvm`, libvirt domain, or RDP/SPICE/VNC listener. Therefore no current live
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
uploads, upload timing, reconnects, shell repaints, and best-effort host
process CPU/DRM render-GPU busy samples. The focused metrics test and the
72-test VDI regression set pass on BigBoy. These are host-side measurements and
do not stand in for the still-missing guest GPU, audio, or live endpoint
evidence.

The Browser VM image build now stamps the immutable profile source revision in
the container metadata and the static verifier requires the image label to
match `profile.env`. A BigBoy rootless rebuild tagged
`localhost/magic-mesh-browser-vm-chromium:provenance-20260803` passed this gate
(image ID `3768fd23fbffc8cb5da5698d9729912373de83a50d9a36d2acad7088a20adaf2`).
This is container provenance evidence, not qcow2 publication or live guest
acceptance.

The guest runtime now also emits a guest-owned, mode-0600 bounded
`/var/lib/mcnf-browser/runtime-evidence.json` record with transport health,
VA-API status, and PipeWire playback/capture endpoint counts. `audio_status=wired`
means endpoint wiring only; it does not claim audible Chromium playback,
capture, or recovery. The refreshed BigBoy image tagged
`localhost/magic-mesh-browser-vm-chromium:runtime-evidence-20260803` passed
static verification (image ID
`9c9307cbbf4940c6c825165edf492386e0c4ca811d6637bd6a36975fcddad0f0`). This is
guest collection/static evidence, not live GPU/audio/performance acceptance.
The collected record has a fail-closed verifier at
`install-helpers/verify-browser-vm-runtime-evidence.py`; it accepts endpoint
wiring as a separate evidence class and always leaves live media proof
unavailable.

The image now also runs a fixed guest-local Chromium media probe against the
64x64 VP8/Opus fixture and emits bounded `media-evidence.json` with media
readiness and decoded/dropped-frame counters. Its verifier is
`install-helpers/verify-browser-vm-media-evidence.py`; the evidence class is
`guest_media_decode` and deliberately leaves live GPU, audible audio, VDI, and
reconnect proof unavailable.

The composite live-acceptance boundary is now implemented at
`install-helpers/verify-browser-vm-live-acceptance.py`. It binds VDI
frame/input/reconnect, guest runtime, guest Chromium decode, GPU-video,
performance, and sample-backed audio records to one source commit and image
digest, rejects stale or credential-shaped evidence, and passes its positive
plus negative self-tests. The runtime/media records and guest image carry the
same pinned source marker. The contract, composite, and image self-tests pass
on separate farm nodes.

The acceptance bundle now also requires a private deployment receipt. After a
real image install and running `browser-vm` domain, use
`packaging/browser-vm/deploy-image.sh receipt` to bind the target hostname,
domain UUID, attached disk, source revision, and local/remote image digests.
`verify-browser-vm-deployment.py` validates that receipt; the composite gate
also requires audio provenance to match the same source and image.

A fresh root-backed 64-GiB qcow2 was rebuilt on BigBoy `.130` at
`/home/mm/browser-vm-chromium-qcow2-provenance-final2-root-20260803/qcow2/disk.qcow2`.
It passed static image verification and `qemu-img check`; SHA-256 is
`10f1db47c453c4f9b269b8df841510d6a7bd8517ffab7e8c0965ab655a68498f`. It is a
ready publication candidate, not a published or live-accepted image.

The latest read-only audit finds seat-15 reachable but without a `browser-vm`
workload, `/dev/kvm`, or an RDP/SPICE/VNC listener. Dell remains unreachable
(`172.20.146.225` has no route from the orchestration host), and no real
six-node live-attestation bundle exists. A read-only probe through seat-15's
Nebula interface (`10.42.0.5`) also timed out to Dell overlay `10.42.0.4` on
SSH and RDP, so seat-15 cannot currently serve as a deployment jump path.
These are the remaining external gates for user access and production
acceptance.

The current standalone repository revision was rechecked on BigBoy `.130`:
the admitted root workspace passed `cargo test --workspace --locked` with 424
tests, and the native sandbox/CEF/Servo check plus workspace/client clippy
boundary passed in a separate farm slot. The source review copy is staged on
seat-15 at `/home/mm/browser-vm-review/6eca9b79`; this is source review only,
not a VM installation.

## Landed

- `magic-mesh-browser-stack` is public and history-bearing at
  `https://github.com/matthewmackes/magic-mesh-browser-stack`.
- The typed `browser-provision` verb now rejects workload aliases and admits only
  the stable name `browser-vm`, matching the name consumed by `Surface::Browser`;
  the focused farm test passes with four tests and no failures.
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
VM contract verifier passes locally; the current-source qcow2 was rebuilt and
statically verified on BigBoy. Fedora 44's enabled repositories provide
`xrdp`/`xorgxrdp` but no xrdp PulseAudio/PipeWire bridge package, and do not
provide `mesa-va-drivers`; default-RDP audio and GPU video decode therefore
remain explicit live gates, with SPICE/QEMU retained as the compatibility path.

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
- The 2026-08-03 clean clone was pinned to public `main`
  `996d3d27cfc4c52776c2289a0069d92e2bede66d`; its root test, native helper
  checks, and strict clippy gates passed on BigBoy. The extraction verifier on
  the pushed cutover branch passed for 85 paths: 18 browser-owned, 25
  mixed-purpose, and 42 shared.
- The current-source Browser VM container and qcow2 build both passed their
  static gates. The artifact remains uninstalled on Dell because the target
  and its overlay address are unreachable from the orchestrator and every
  farm build VM; the current image therefore must not be treated as live VDI
  proof.

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
- Seat 15 (`172.20.0.15`) received the F44 base review RPM, but its Browser
  image remains untouched; low free space and absent KVM make it unsuitable for
  this image without capacity/hypervisor work.

## Still unproven

The host cutover, prior qcow2 provenance, Chromium guest capture, decoded SPICE
pixels, focused-input visual echo, and the Dell host PipeWire audio route are
evidenced. The current Dell domain is still QXL/SPICE and is not the newly
rebuilt RDP image. RDP endpoint publication/authentication and live proof, GPU
video, guest Chromium audio playback, five-session performance,
signed publication/install/upgrade, reconnect/failover,
and six-node acceptance remain open. Fedora 44's enabled repositories still
lack the multimedia RPM dependencies required by the repository-only image
lane, so GPU video remains a live hardware gate. Seat 15 still needs operator
capacity work. This change does not claim that the Chromium App VM is
production-ready. The broker and wire gates are green; the live guest remains
not ready for RDP because Dell currently refuses TCP 3389 and the running
image is the stale Pixman/QXL candidate.
