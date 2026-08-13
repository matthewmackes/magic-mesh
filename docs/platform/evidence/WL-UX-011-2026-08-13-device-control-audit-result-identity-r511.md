# WL-UX-011 device-control audit/result identity — r511

Date: 2026-08-13

## Implementation

`device_control` now writes the exact request correlation ID, terminal result
ID, and typed terminal outcome into every privileged-control hash-chain event.
Cancellation events also carry their terminal result ID. Previously two
overlapping controls with the same host, operation, device, and requester could
not be unambiguously reconciled from the audit chain after authorization.

Production scope:

- `crates/mesh/mackesd/src/workers/device_control.rs`

The focused hostile regression creates two otherwise indistinguishable controls
with distinct IDs and opposite terminal outcomes, verifies the two-row chain is
intact, and parses each event envelope to require its exact request/result
identity and outcome.

## Farm evidence

- `.170`, slot `ux011-audit-id-clippy`: strict async-services library Clippy
  passed:

  `cargo clippy -p mackesd --lib --features async-services -- -D warnings`

- `.50`, slot `ux011-audit-id-fmt`: file-scoped Rustfmt passed:

  `rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/device_control.rs`

- BigBoy `.130`, slot `ux011-audit-id-test`: the original cold focused gate was
  interrupted externally during final test-code generation and is not claimed.
- `.196`, slot `ux011-audit-id-final`: the one authorized reroute completed the
  full 4,957-test binary build. Its first execution exposed a representation-
  specific test assertion (the detail is nested in the typed `Event` envelope),
  not a production failure. The assertion was corrected to deserialize
  `crate::events::Event`; file-scoped Rustfmt then passed on `.196`. Per operator
  direction, no duplicate farm rerun was launched.

`git diff --check` passed. Unrelated concurrent worktree changes were preserved.

## Remaining acceptance

WL-UX-011 still requires complete provider/control coverage, first-release
package integration, and the explicitly deferred non-blocking post-release
one-node installed provider/control/fleet proof.
