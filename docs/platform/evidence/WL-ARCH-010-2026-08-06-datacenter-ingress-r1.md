# WL-ARCH-010 — Datacenter Execute ingress bounds (2026-08-06)

Status: implementation slice complete; the parent epic remains `Remaining`
because live provider, Workload, Display1/KMS, Dell, and seat acceptance proofs
are still open.

## Invariant

The Datacenter Bus responder remains the sole authority for the registered
`action/dc/*` VM, storage, genesis, and lighthouse verbs. Each verb now admits
at most 64 retained action messages per responder tick. The limit is enforced
by the shared SQL query before message bodies are decoded, and the existing
exclusive ULID cursor preserves oldest-first progress without skipping or
materializing an unbounded backlog.

## Implementation

`crates/mesh/mackesd/src/ipc/datacenter.rs` adds the public
`MAX_MESSAGES_PER_POLL` contract and routes every Datacenter action topic
through `Persist::list_since_limit`. No second responder, alternate Bus, or
caller-side authority was introduced.

## Hostile regression

`datacenter_action_recovery_reads_a_bounded_page_and_advances_cursor` seeds
65 authorized `genesis-plan` requests, proves the first poll admits exactly 64
and replies to those requests, proves the 65th request has no reply yet, then
proves the next poll advances the cursor and replies to the final request.

## Farm verification

- BigBoy (`172.20.0.130`): `cargo test -p mackesd --features
  async-services ipc::datacenter::tests -- --nocapture` — **80 passed, 0
  failed**.
- BigBoy focused regression: **1 passed, 0 failed**.
- BigBoy direct touched-file check: `rustfmt --edition 2021 --check
  crates/mesh/mackesd/src/ipc/datacenter.rs` — passed.
- Local `git diff --check` — passed.
- Package-wide `cargo fmt -p mackesd -- --check` remains mixed because the
  large dirty module contains unrelated pre-existing formatter drift; no
  whole-file rewrite was applied. The changed Datacenter hunk is clean.

Source SHA-256:

```text
a51017676cf3d56b1356544e81f6d6c05651804590cdfb8eecc3ecc8f03ab5c4  crates/mesh/mackesd/src/ipc/datacenter.rs
```

## Remaining acceptance

Live libvirt/XAPI/Quadlet execution and recovery, Workload caller migration,
Display1/KMS scanout, Dell/seat-15 acceptance, and full RPM promotion remain
unproven. This fixture-backed farm result does not claim live provider success.
