# WL-UX-011 S2 — Hardware probe publication-directory identity (r515)

- Date: 2026-08-13
- Epic: `WL-UX-011`
- Slice: S2 observation providers
- Base revision: `28596ccaa8c3f3409c4397d6ed9093c4105724ed`
- Source: `crates/mesh/mackesd/src/workers/hardware_probe.rs`
- Source SHA-256: `ebc74eba7b70de0dc1c4ae9e1ded1e3099c7f28e33f7ba8536f6ef227339fad7`

## Result

Hardware-probe publication now opens and pins the exact replicated node
directory before inspecting or creating its staging row. Staging creation,
cleanup, final rename, and directory durability sync all use that descriptor.
Immediately before finalization, the provider verifies that the pathname still
resolves to the pinned device/inode. Directory replacement therefore fails
closed: neither a replacement directory nor a detached former directory can
receive `probe.json`, and foreign staging content remains untouched.

This is distinct from the preceding class-projection bound. It closes a
publication ownership gap at the provider output boundary and preserves the
last trusted publication on failure.

## Farm verification

BigBoy was excluded as directed.

### Host `.90`, workspace `magic-mesh-farm-ux011probe-test`

```text
cargo test -p mackesd --lib replaced_probe_directory_cannot_capture_hardware_publication -- --nocapture
```

Result: passed 1/1; 4,959 filtered out. The hostile fixture replaces the
publication directory after the staged bytes are durable and verifies that the
provider rejects finalization, removes only its descriptor-owned staging row,
does not create either possible `probe.json`, and does not alter the foreign
replacement staging row.

### Host `.170`, workspace `magic-mesh-farm-ux011probe-clippy2`

```text
cargo clippy -p mackesd --lib --features async-services -- -D warnings
```

Result: passed.

```text
rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/hardware_probe.rs
```

Result: passed.

`git diff --check` also passed. A crate-wide formatting check was not used as
evidence because it reports unrelated existing formatting drift outside this
slice.

## Remaining WL-UX-011 acceptance

The complete provider coverage matrix, remaining safe staged controls,
first-release package integration, and deferred non-blocking installed one-node
provider/control/fleet proof remain. This slice claims only exact hardware-probe
publication ownership across directory replacement.
