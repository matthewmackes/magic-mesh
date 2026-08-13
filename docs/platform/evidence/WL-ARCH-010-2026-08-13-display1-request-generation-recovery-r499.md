# WL-ARCH-010 — Display1 request-generation recovery boundary (r499)

Date: 2026-08-13

## Result

Restart recovery now proves that a persisted `StartAndAttach` status generation
is the exact successor authorized by its durable owning request before calling
the Workload actuator. A structurally valid journal reassociation in which the
status and Display1 lease agree with each other but not with the request fails
closed: the adapter is not called, the exact persisted capability is revoked,
and the durable projection becomes unavailable.

This closes an exact-generation gap in the KMS/Display1 recovery boundary. The
existing checks already bound workload identity, protocol, expiry, operation
deadline, exact lease equality, listener registration, and validated first
frame; they did not previously bind the recovered status generation back to
the request's compare-and-swap generation.

## Farm evidence

- `.196`, slot `arch010-display1-request-generation-clippy-r499`:
  `cargo clippy -p mackesd --lib -- -D warnings` passed.
- `.196`, the same warmed slot:
  `cargo test -p mackesd --lib workers::workload_compute::tests::recovered_attachment_generation_must_be_authorized_by_owning_request -- --exact --nocapture`
  passed 1/1 with 4,949 tests filtered out.
- `.196`, slot `arch010-display1-request-generation-fmt-r499`: a
  file-scoped Rustfmt comparison reported no formatting delta in any changed
  hunk. The complete file/package check continues to expose pre-existing drift
  in untouched regions, so no unrelated formatting rewrite was included.
- Local `git diff --check` passed.

The initial `.50` and `.90` focused-test attempts were stopped while still in
cold compilation and were superseded by the completed `.196` gate; no result is
claimed from them.

## Remaining acceptance

Coding recovery now fails closed across the durable request, exact workload
generation, exact Display1 lease, listener registration, and validated-first-
frame boundaries. Full release RPM/repository transaction and post-release
installed-seat/fleet proof for real libvirt/Quadlet start-and-attach, KMS import,
Display1 presentation recovery, reboot, reconnect, and package upgrade remain.
