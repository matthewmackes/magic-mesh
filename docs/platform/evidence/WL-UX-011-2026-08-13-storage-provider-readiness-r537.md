# WL-UX-011 — storage provider readiness (r537)

Date: 2026-08-13
Source branch: `agent/drain-worklist-20260725`

## Production change

`mackesd`'s physical block provider no longer labels every `/sys/block` row
healthy. It now publishes bounded kernel-owned readiness facts:

- allowlisted `device/state` values distinguish live/running/active media from
  offline, blocked, quiesced, and suspended media;
- a class without `device/state` is healthy only when both a valid kernel
  major:minor and non-zero media geometry exist;
- absent, malformed, zero-media, and unsupported state is `Unknown`, never
  invented `Ok`;
- read-only state and the admitted device number are observational event facts;
  malformed or credential-shaped values are not published.

The existing generation-bound device-control authority rejects every `Unknown`
provider row before mutation. Physical disks retain a `/sys/block` identity,
which has no enable/disable, module, or bus-rescan seam in the fixed executor;
therefore this slice adds truthful provider state without inventing unsafe disk
controls.

## Farm evidence

- `172.20.0.130`, slot 2: focused regression
  `cargo test -p mackesd --features async-services
  physical_block_provider_reports_kernel_readiness_and_refuses_invented_health
  -- --nocapture` passed 1/1. The cold test profile completed in 11m47s while
  BigBoy was fully occupied by existing commands.
- `172.20.0.196`, slot 1: strict production-library Clippy
  `cargo clippy -p mackesd --features async-services --lib -- -D warnings`
  passed.
- `172.20.0.196`, slot 1: focused package build
  `cargo check -p mackesd --features async-services` passed.
- BigBoy slot 3: `cargo fmt --all -- --check` ran and reported only pre-existing
  formatting drift in unrelated concurrent files and older regions of the
  provider module. The new storage-provider hunk was corrected to the Rust 1.94
  formatter output; no unrelated formatting was rewritten.
- Local scoped `git diff --check` passed.

The completed `.130` slot-3 and `.196` slot-1 workspaces were ownership-checked
and removed immediately. Slot 2 was removed after its focused test completed.

## Residual WL-UX-011 coding

Provider and safe-control coverage remains to be audited and completed for
Wi-Fi, audio, display, input, printers, services, privacy, and virtualization,
plus any remaining storage transitions not represented by Linux block-class
state. Safe-control preview/result, audit, cancellation, and recovery must stay
generation- and capability-bound as those providers become actionable.

## Deferred post-release proof

Live one-node hardware captures, stale/failed-provider transitions, physical
control outcomes, conflict/history views, scans, credential-free fleet exports,
installed-package identity, and restart/rejoin recovery remain deferred and
non-blocking until after the first full release. No live hardware acceptance is
claimed here.
