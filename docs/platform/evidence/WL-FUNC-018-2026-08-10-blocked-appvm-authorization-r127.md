# Blocked Flatpak App-VM authorization checkpoint

Date: 2026-08-10  
Epic: `WL-FUNC-018` S3/S4

## Defect and correction

Front Door's ordinary peer-app launch wire refused targets carrying
`launch_blocked_reason`, but the catalog-backed `peer_app_provision_wire` path
did not repeat that admission check. A stale, unavailable, or otherwise blocked
Flatpak row could therefore reach the root-mutation authorizer and construct a
Workload request through the alternate App-VM path.

`peer_app_provision_wire_with` now checks the current launch-blocked state before
identity derivation, authorization, or Bus body creation. Every present blocked
marker fails closed; a blank malformed reason becomes the bounded diagnostic
`unspecified admission refusal` rather than silently restoring launchability.

## Focused farm proof

Host `.90`, slot `func018-appvm-policy-readiness-s4-r1`:

```text
cargo test -p mde-shell-egui \
  front_door::tests::blocked_flatpak_state_cannot_reach_app_vm_authorization \
  -- --exact --nocapture

1 passed; 0 failed; 1538 filtered out
```

The regression observes the authorization callback and proves neither a stale
catalog reason nor an empty blocked marker can invoke it. `git diff --check`
passed. This is source/farm admission evidence; image supply, live App-VM boot,
VDI presentation, and physical-seat proof remain open.
