# WL-UX-013 bounded remediation plan authority — 2026-08-13

## Result

Commit candidate changes `crates/mesh/mackesd/src/remediation.rs` so governed
health recovery plans are admitted only from bounded regular TOML files. Plan
files are sorted and capped before inspection; symlinks, non-regular files,
payloads over 16 KiB, malformed identifiers, more than 32 bindings, oversized
values, and attempts to pre-populate reserved `drift_*` event bindings are
refused. Duplicate on-disk plan names are refused as a group, preserving the
built-in recovery authority rather than allowing filesystem enumeration order
to choose a replacement.

The discoverable hostile regression is:

`remediation::tests::hostile_duplicate_symlink_and_unbounded_plans_cannot_substitute_recovery_authority`

It presents duplicate core-plan substitutions, a symlink to an outside plan,
an oversized sparse plan, and a reserved event-binding substitution, and
asserts that none acquires recovery authority.

## Exact farm gates

Host: `172.20.0.50`, slot 1:

```text
cargo test -p mackesd --features async-services \
  remediation::tests::hostile_duplicate_symlink_and_unbounded_plans_cannot_substitute_recovery_authority \
  -- --exact --nocapture
```

The command compiled the current shared worktree but did not reach test
discovery or execution. It stopped with `E0505` in concurrently owned
`crates/mesh/mackesd/src/workers/device_inventory.rs:1744` (`identity` moved
while `card` still borrows it). This is not a passing test claim.

Host: `172.20.0.50`, slot 2:

```text
cargo clippy -p mackesd --features async-services --all-targets -- -D warnings
```

Strict Clippy reached `mackesd` and stopped at the same concurrent
`device_inventory.rs:1744` `E0505`; no diagnostic named the remediation slice.
This is not a passing Clippy claim.

Per the stop cadence, no rerun, broader build, or formatting gate was launched.

## Remaining acceptance

- Execute the exact hostile regression with nonzero discovery after the
  concurrent device-inventory compile error is corrected.
- Run strict relevant Clippy/build/format gates in a later permitted wave.
- First-release packaging and deferred post-release one-node recovery proof
  remain outside this coding slice.
