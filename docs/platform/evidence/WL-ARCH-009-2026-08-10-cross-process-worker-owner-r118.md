# WL-ARCH-009 — cross-process worker ownership (r118)

Date: 2026-08-10

## Correction

The six grouped `mackesd` services selected workers by registry group, but a
second `mackesd serve --group <same-group>` process could still acquire and
publish that same group's workers. Each installed group process now holds an
exclusive kernel-backed lease under its shared `MDE_BUS_ROOT`. The supervisor
also canonicalizes kebab/snake registry aliases and rejects a second live owner
inside one process; completed workers release their identity for replacement.

Embedded or development supervisors without an explicit shared bus root retain
process-local protection only. All six installed services explicitly set
`MDE_BUS_ROOT=/run/mde-bus`.

## Focused farm proof

BigBoy (`172.20.0.130`) passed the exact library regression:

```text
cargo test -p mackesd --lib \
  workers::tests::group_lease_refuses_second_process_and_canonicalizes_replacement \
  -- --exact --nocapture

test result: ok. 1 passed; 0 failed; 4667 filtered out
```

The test launches child processes and proves rejection while the first process
holds the group, admission after release, alias collapse, and replacement after
a worker exits. The abandoned broad Cargo invocation was stopped and is not
counted.

## Remaining boundary

An installed-service duplicate-start probe and deployment proof remain. This
checkpoint does not claim completion of the wider process-isolation epic.
