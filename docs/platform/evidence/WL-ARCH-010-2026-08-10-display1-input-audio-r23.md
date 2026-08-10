# WL-ARCH-010 — lease-bound Display1 input and local VM audio

Date: 2026-08-10

Input implementation revision: `458cbd31` (`add lease-bound Display1 guest input`)

## Delivered boundary

The DRM owner now routes guest input only after a native frame has completed
KMS presentation and a pointer press has explicitly focused that frame. The
shell sends bounded `SOCK_SEQPACKET` input envelopes carrying the exact
Workload lease, workload ID, generation, and a strictly increasing sequence.
The daemon rejects ancillary descriptors, stale or mismatched identity,
replay/regression, unsupported keys/buttons, out-of-frame pointer coordinates,
and input before a retained scanout.

Super/meta, media keys, and Escape remain host-only. The retained QEMU
Display1 control connection exposes only the advertised Keyboard and Mouse
interfaces. Every successful key/button press is retained daemon-side and is
released on the matching edge, focus loss, relay replacement, disconnect,
lease expiry/revocation, QEMU loss, or shutdown. Failed QEMU releases remain
held and retry behind a bounded D-Bus timeout; fresh input cannot overtake
that cleanup.

The reconciler's VM definition no longer asks system QEMU to discover a
nonexistent per-user native PipeWire socket. It uses the packaged,
localhost-only PipeWire-Pulse endpoint at `127.0.0.1:4713`, with separate
capture and playback streams named from the escaped Workload domain. VM start,
restart-start, and migration-start now refuse with actionable retry state when
that endpoint is unreachable.

## Farm verification

- BigBoy `.130`, slot `arch010-s6-input-daemon`:
  `cargo test -p mackesd --features async-services --lib display1 -- --nocapture`
  passed 21 tests after the exact sequence replay/regression seam was added.
- Machine 196, slot `arch010-s6-input-shell`:
  `cargo test -p mde-shell-egui --features drm display1_client::tests -- --nocapture`
  passed 10 tests.
- The worker's focused DRM Display1 run passed 5 tests; exact rustfmt and
  `git diff --check` passed.
- Machine 194 build VM `.170`, slot `arch010-s6-input-drm`:
  `cargo test -p mde-egui --features drm display1_absolute_pointer_uses_bounded_console_pixels --locked -- --nocapture`
  passed 1 test. An earlier zero-test `--exact` invocation was rejected and is
  not evidence.
- KVM-XCP1 build VM `.90`, slot `workload-audio-r23`:
  `cargo test -p mackesd --features async-services --lib workers::workload_vm::tests::definition_uses_display1_and_escapes_untrusted_fields --locked -- --exact --nocapture`
  passed 1 test. The same warmed slot then ran
  `workers::workload_compute::tests::vm_start_requires_a_reachable_local_audio_endpoint`
  with `--exact`; 1 test passed and proved both reachable admission and
  actionable refusal of a closed endpoint.

## Live audio baseline

Read-only inspection found Dell and seat 15 running `browser-vm` with the
same proven libvirt contract now used by Workloads: virtio sound, a
`pulseaudio` backend at `tcp:127.0.0.1:4713`, and separate capture/playback
streams. On Dell, seat 15, and T480,
`mcnf-qemu-pulse-endpoint.service` was active and its installed `--health`
probe reported `healthy user=mm address=127.0.0.1 port=4713`.

## Source binding

```text
display1_client.rs  5847c813316cc36f5efe85a072a7951bc4e47675447c873005c3fc078001c06e
display1_broker.rs  c461c0cd88c78a4ed18db6048545242b3bb6b8e7d42f23879303f15515199abf
workload_compute.rs 77fcd99e12cc9bd9b1cc799138f53b1041e0cfede3e500ec75543fa716bc0c17
workload_vm.rs      806257d0c1fe3857ddf0207808b85964d12ed762106676db04ed903cafc51814
drm.rs              ee7458af8d03861c66663097ee3ebf15dbb486cda136037119e7ed3719705ddf
```

## Remaining limitation

No physical-seat test injected keyboard, pointer, or PCM through a newly
created typed Workload VM in this slice. Native first-frame/input/audio and
device-loss proof on real QEMU/KMS hardware therefore remains required before
ARCH-010 S6 or S8 can close.
