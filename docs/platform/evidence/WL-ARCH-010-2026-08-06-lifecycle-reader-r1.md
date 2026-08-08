# WL-ARCH-010 — retired lifecycle reader bounds (2026-08-06)

Status: implementation and focused farm verification complete; the epic remains
`Remaining` because live Workload adapter/recovery, Dell, seat-15, and release
acceptance evidence are still open.

## Change

The daemon's retained compatibility readers in
`crates/mesh/mackesd/src/ipc/directory.rs` now use the SQL-bounded
`Persist::list_since_limit` API for both:

- `action/services/lifecycle`, which remains a fail-closed refusal lane for
  old clients; and
- `action/services/lifecycle-result`, which remains an authenticated,
  single-use result-file reader during cutover.

Each responder admits at most `MAX_LIFECYCLE_MESSAGES_PER_POLL` (`64`) messages
per sweep. The cursor is still advanced after each admitted message, so a
backlog is drained oldest-first over successive ticks without materializing the
entire retained topic.

The dead `crates/mesh/mackesd/src/workers/lifecycle_exec.rs` actuator was
removed from the worker registry. The compatibility `lifecycle.rs` module was
narrowed to safe result write/read support; its replicated request writer and
raw `podman`/`virsh` command planner were removed. The typed Workload executor
remains the only spawned VM/container actuator.

## Hostile regression coverage

Two directory tests write 65 retained messages, run one poll, and assert that
exactly the first 64 replies exist, the cursor points at message 64, and the
65th reply is absent. A second poll admits the final message. The same shape is
covered for both the retired refusal lane and the result-reader lane.

## Verification

- Farm `.50`, slot `lifecycle-retire-r1`:
  `MCNF_BUILD_HOST=172.20.0.50 MCNF_BUILD_SLOT=lifecycle-retire-r1
  ./install-helpers/xcp-build.sh cargo test -p mackesd directory`
  passed: 49 directory-library tests, 4 binary tests, and 1 selected
  integration test; 0 failed.
- Farm `.90`, slot `lifecycle-retire-fmt-r1`:
  `rustfmt --edition 2021 --check
  crates/mesh/mackesd/src/lifecycle.rs
  crates/mesh/mackesd/src/ipc/directory.rs` passed.
- Local `git diff --check` passed.
- Source SHA-256:
  `crates/mesh/mackesd/src/ipc/directory.rs`
  `bb7b65e77a4fe9beb096a57b9556eb3cc83fb18311cce3c4a197f876d88c1eae`;
  `crates/mesh/mackesd/src/lifecycle.rs`
  `56b382de25b7fdf71fb8cca951dd4876726ba947f6bc3e2ecbb7ec25464bbc2a`;
  `crates/mesh/mackesd/src/workers/mod.rs`
  `ddb77e7dfa2525f2f687d13ac600a8fd6adad4ab6cee14cd0a96a1f92b7a2895`.

The directory refusal/result vocabulary remains compatibility evidence, not a
second actuator. Negative source search found no `lifecycle_exec` module use,
`LifecycleRequest`, `write_request`, `take_requests`, or `command_plan` in
`mackesd`; production VM/container mutations use the typed Workload lane. Live
libvirt/Quadlet and restart/recovery proof remains open.
