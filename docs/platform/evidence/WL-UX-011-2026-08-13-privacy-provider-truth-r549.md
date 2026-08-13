# WL-UX-011 — truthful privacy-provider readiness (r549)

Date: 2026-08-13

## Production result

- `privacy_provider` cross-checks the `mm` user session's
  `xdg-desktop-portal.service`, system `polkit.service`, and kernel SELinux LSM
  plus enforcement facts.
- The periodic hardware-probe worker publishes one bounded
  `privacy-provider/<node>.json` projection containing only schema, node,
  observation time, `Ready` / `Disconnected` / `Disabled` / `Unknown`, and a
  fixed reason.
- Missing, malformed, oversized, duplicate, contradictory, or substituted
  observations classify as `Unknown`. The projection contains no application
  names, grants, device labels, credentials, logs, or raw command output and
  grants no mutation authority.

## Exact gate evidence

- `.170`, slot 1 — `cargo build -p mackesd --features async-services --lib`:
  **passed** (`Finished dev profile` in 3m49s).
- BigBoy `.130`, slot 1 — requested focused selector completed successfully but
  selected **0 tests** (`4993 filtered out`), so it is recorded as insufficient
  and is not claimed as a passing hostile regression. No rerun was launched per
  the operator cadence instruction.
- `.90`, slot 1 — strict all-target Clippy stopped at the concurrent
  `device_inventory.rs:1744` borrow error. Before that stop it identified one
  slice-local `unused_mut` at `privacy_provider.rs:156`; that exact issue was
  corrected. No rerun was launched per cadence.
- `.196`, slot 1 — exact-file Rustfmt check found only line wrapping in the new
  file; those exact deltas were applied. No broader formatting or rerun was
  launched.
- Final repository `git diff --check`: **passed**.

## Deferred acceptance

Installed portal/polkit/SELinux transitions and one-node Workers rendering are
post-first-release, non-blocking acceptance. This slice does not claim them.
