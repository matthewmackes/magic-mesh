# WL-ARCH-009 S1/S4 Nebula dispatcher ownership — 2026-08-09

## Correction

The process-local `nebula-signal-dispatcher` was absent from the canonical
registry and bypassed the group-admission guard. Consequently, each of the six
`mackesd serve --group` services opened a dispatcher against the same Bus topic,
although only Control's enrollment watcher and Observation's health reconciler
produce through their respective in-memory sender slots.

The start now has two explicit adapter identities:

- `nebula_control_signal_dispatcher`, owned only by Control;
- `nebula_observation_signal_dispatcher`, owned only by Observation.

Both are `ResponderThread` rows with bounded registry contracts and literal
runtime-roster registrations. Actions, Data, Compute, and Integrations fail
closed before opening a dispatcher. Keeping two identities preserves both
process-local producers without pretending an in-memory channel crosses the
service boundary.

The bidirectional responder source guard now covers both starts. A negative
regression checks each identity against all six groups and admits exactly its
declared producer group.

## Focused farm verification

Machine 196 (`172.20.0.196`), slot
`arch009-nebula-dispatch-r95`:

- `cargo test -p mackesd process_group_thread_admission_tests -- --nocapture`:
  4 passed, 0 failed, 56 bin tests filtered. This covers accepted/hostile group
  argv, bidirectional responder registration/guard equality, and both dispatcher
  identities against all six groups.
- The canonical inventory-hash bootstrap produced
  `2a444300c05136dbdbe08420d7fce51efc7e9cc418b7adf1031ceed41e6588c4` from
  all 145 rows and the value is pinned in source.
- The final worker-role rerun could not relink after the cold target reduced
  `.196` to less than 1 GiB free; `mold` reported a full output filesystem.
  Therefore no final passing hash rerun or package/live claim is made.
- `git diff --check` passed locally. No package was built and no host was
  deployed or restarted.

## Remaining S1/S4 gaps

- `WorkerSpec` still leaves typed publications, subscriptions, dependencies,
  and action descriptors empty, so capability ownership is not complete.
- `worker-runtime-status` remains uncensused and every grouped process writes
  the same `/run/mde/mackesd-status.json`; it still needs one aggregate authority
  or group-specific outputs plus a fold.
- Other daemon infrastructure starts (service-key retry, etcd startup probe,
  watchdog, and signal thread) remain outside the worker registry and need an
  explicit worker-versus-process-infrastructure policy and drift guard.
- A corrected package and live six-cgroup census/restart proof remain required
  before S4 can claim fleet isolation and recovery.

WL-ARCH-009 remains `Remaining`.
