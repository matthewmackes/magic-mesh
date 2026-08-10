# WL-ARCH-010 — exact recovered attachment identity (r130)

Date: 2026-08-10

Base revision: `b2c1205d`

Prior missing-lease proof:
`docs/platform/evidence/WL-ARCH-010-2026-08-10-recovered-attachment-lease-r110.md`.

## Defect and correction

Terminal Workload recovery previously accepted an actuator's Display1 lease
when workload ID, generation, protocol, and validity matched the journal. A
buggy or hostile adapter could therefore substitute a new lease ID and socket
inside the same generation, publish that unjournaled capability as Ready, and
leave the durable lease identity behind.

Recovery now requires the returned lease to equal the complete journaled lease.
When an adapter substitutes a valid-looking lease, the reconciler revokes that
exact uncommitted endpoint and the journaled capability, clears attachment
state, and records an unavailable terminal result. A returned lease with no
journaled authority is also revoked. An identical but expired lease follows the
existing single revocation path rather than being revoked twice.

## Focused farm proof

BigBoy (`172.20.0.130`), slot
`arch010-recovered-lease-identity-r130`, first passed the focused recovery
filter after the revocation correction:

```text
cargo test -p mackesd recovered_ -- --nocapture
test result: ok. 6 passed; 0 failed; 4669 filtered out
```

The final source-only diagnostic wording was then synced into the warm machine
193 (`172.20.0.90`) slot and the new hostile identity case passed exactly:

```text
cargo test -p mackesd --lib \
  workers::workload_compute::tests::recovered_attachment_cannot_substitute_a_new_lease_in_the_same_generation \
  -- --exact --nocapture
test result: ok. 1 passed; 0 failed; 4674 filtered out
```

Source SHA-256:

- `d19c89fd27ad66ec6dbbddff55136ff92dbc94304bfa735395a74b5a6d17cd1e`
  — `crates/mesh/mackesd/src/workers/workload_compute.rs`

No broad suite, package build, or live Display1 restart was run. The finished
12-GiB BigBoy slot was removed after verifying no process owned it; it can be
recreated by the farm sync helper.

## Remaining boundary

Installed-seat crash/restart proof must still demonstrate that the production
libvirt adapter reproduces the exact journaled lease and first frame, with no
orphan socket after refusal.
