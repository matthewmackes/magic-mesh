# WL-UX-011 S2 — bounded input-provider unavailable truth (r503)

## Production gap and correction

The Fedora sysfs input provider walked `/sys/class/input` without an entity
budget and converted a missing or unreadable kernel `name` attribute into a
healthy (`Ok`) row named only from the node. That made unavailable provider
data look observed and allowed a malformed class tree to expand one inventory
generation without bound.

`device_inventory::input_devices` now:

- admits only `input*` provider rows before applying a deterministic 256-row
  budget, so sibling `event*` and `mouse*` nodes cannot consume capacity;
- retains the exact `/sys/class/input/input*` path as the provider-owned source
  identity used by final-generation sysfs re-attestation; and
- publishes an unreadable/missing kernel name as `Unknown` with `input device
  name unavailable`, never as healthy or as a fabricated hardware name.

No hardware value, credential, control authority, or second provider owner was
introduced.

## Farm evidence

- `.130`, slot `ux011-input-provider-test-r503`:
  `cargo test -p mackesd --lib workers::device_inventory::tests::input_provider_is_bounded_and_reports_unavailable_names_truthfully -- --exact --nocapture`
  — passed 1/1, with 4,951 tests filtered out. The initial `.90` transport
  stalled after compilation and was terminated before this non-duplicated
  reroute.
- `.170`, slot `ux011-input-provider-clippy-r503`:
  `cargo clippy -p mackesd --lib --no-deps -- -D warnings` — passed after the
  unrelated concurrent `clock.rs` edit was restored to branch `HEAD` only in
  the disposable farm workspace.
- `.170`: `rustfmt --edition 2021 --check` identified one changed-hunk wrapping
  correction, which was applied; remaining reported drift is pre-existing and
  outside this slice.
- `git diff --check` — passed.

## Remaining WL-UX-011 acceptance

This closes one S2 input-provider coverage gap. The epic still requires a
complete audited coverage matrix for every named provider, completion of safe
staged controls, first-release package integration, and the deferred
post-release one-node live provider/control/fleet proof. This static slice does
not claim hardware acceptance.
