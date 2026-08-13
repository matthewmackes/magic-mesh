# WL-UX-011 — truthful virtualization-provider readiness (r545)

## Production result

The existing per-node KVM health worker now publishes a second, bounded,
credential-free `event/provider/virtualization` projection. It cross-checks
four independent authority surfaces before reporting `Ready`:

- `/dev/kvm` is a real character device and the kernel KVM module exists;
- the canonical libvirt systemd unit is active;
- `virsh` reaches the exact `qemu:///system` URI; and
- the canonical `default` libvirt network and storage pool are active.

An explicitly absent KVM device/module, disabled libvirt unit, unavailable
connection, and absent network/pool can report `Disabled`. A present but
incomplete stack reports `Disconnected`. Missing, malformed, oversized,
contradictory, or substituted facts report `Unknown`; in particular, a
non-character `/dev/kvm`, a live connection behind an inactive unit, and active
resources behind a failed connection cannot become healthy.

The projection contains only schema version, node identity, observation time,
the four-state readiness, and a fixed reason. It publishes no command output,
domain names, image paths, device labels, credentials, or secrets and adds no
mutation authority. Integration is self-contained in the already-running KVM
health worker because concurrent work owned the general hardware/provider
registries.

## Gates

- `.170` slot 2: `cargo build -p mackesd --features async-services
  --all-targets` passed. The run compiled the complete production/library test
  surface and reported only the slice-local `unused_mut` warning corrected
  immediately afterward.
- `.90` slot 2: strict all-target Clippy completed its one permitted run and
  stopped only on that same slice-local `unused_mut` at
  `kvm_health.rs:142`. The exact `mut` token was removed; cadence prohibited a
  rerun.
- `.90` slot 1: the intended focused hostile selector compiled the test surface
  successfully but selected 0 tests because `--exact` was paired with the
  unqualified test name. This is recorded as insufficient test evidence, not a
  passing focused regression, and was not rerun per cadence.
- Exact scoped `git diff --check` passed after the correction.
- Exact-file Rustfmt was not started because the stop cadence arrived while the
  unique focused gate was still running; no formatter result is claimed.

## Residual WL-UX-011 acceptance

Display, printers, services, privacy, and further capability-gated safe-control
coverage remain. Physical virtualization transitions and installed one-node
acceptance remain deferred, non-blocking post-release proof.
