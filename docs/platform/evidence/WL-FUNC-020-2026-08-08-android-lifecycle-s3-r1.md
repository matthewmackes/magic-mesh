# WL-FUNC-020 Android governed lifecycle — 2026-08-08

The daemon now exposes one armed `android-lifecycle` verb for start, stop,
cancel, and retry. Admission binds stable request identity and exact workload
generation to the signed catalog, package manifest, and provider preflight.
Replays are idempotent and stale generations produce no runtime effect.

Transitions are journaled atomically before outer-VM mutation through the
existing `CloudRunner`/libvirt authority. Restart recovery stops an outer VM
left by an incomplete operation before accepting new work; failed start/retry
also cleans the VM and retained app state. Success requires typed guest
inventory proving the exact approved app installed, ready, launcher-resolved,
and launchable. A production framed Unix-socket guest adapter now bounds each
response at 256 KiB with three-second I/O timeouts and binds catalog, manifest,
image, workload, generation, request, launcher, and VDI identity. It publishes
a typed WebRTC VDI source only after readiness proof and clears it on failure,
stop, cancel, and cleanup. A missing relay remains explicitly unavailable.

## Verification

- BigBoy `.130`, slot `func020-android-lifecycle-s3-r1`: generation,
  idempotency, crash recovery, and cleanup passed 4/4.
- The Cuttlefish provider, readiness, and injectable launcher suites passed
  12/12.
- BigBoy `.130`, slot `func020-android-guest-s3-r2`: guest transport passed 3/3,
  lifecycle rerun passed 4/4, and adjacent Cuttlefish contracts passed 10/10.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

The guest relay must be packaged into the signed Cuttlefish image and expose its
workload socket. Shell VDI attachment, signed catalog/image deployment, nested-
KVM package/launcher/cleanup authorization, first frame, and live proof remain.
FUNC-020 stays `Remaining`.
