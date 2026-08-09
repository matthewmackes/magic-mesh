# WL-ARCH-009 — desktop sources Bus transaction recovery (r83)

Date: 2026-08-09

Baseline: `222a06ef5cf14ce08dd1174af712b6ff2149b38b`

Farm: machine194 (`172.20.0.170`), `MCNF_BUILD_SLOT=desktop-sources-bus-r83`

## Production result

- `desktop_sources` now resolves the configured/current Bus on every cycle, with canonical `SYSTEM_BUS_ROOT` fallback when no explicit root is configured. An absent or unopenable Bus defers and the same worker retries with shutdown-aware bounded cadence.
- Each cycle fresh-opens `Persist` and binds the connection to the live `index.sqlite` device/inode. A same-path replacement or connection/path race cannot activate a stale connection; each new index atomically tail-primes all three transient desktop action lanes before forward work is admitted.
- Add/remove/refresh reads are staged completely before authorization or manual-store effects. A failed lane preserves the complete prior cursor set.
- Peer records are read fail-closed with 4,096-record and 1-MiB-per-record bounds, regular-file/no-symlink checks, opened-file identity checks, strict JSON, and filename/hostname agreement. Every required remote and local Workload projection is then read and validated from the same Bus connection before any action effect or roster publication. Read/decode/validation failure defers the whole candidate rather than publishing a partial/empty roster.
- Publication is identity-checked before and after the write. Replacement during publication clears the fold gate and leaves corrected-forward publication pending. Action cursors install only after complete source staging and the action sweep; the publication fingerprint advances only after the Bus write.

## Focused hostile verification

The farm slot was populated through:

```text
MCNF_BUILD_HOST=172.20.0.170 MCNF_BUILD_SLOT=desktop-sources-bus-r83 install-helpers/xcp-build.sh cargo test -p mackesd --features async-services desktop_sources::tests::desktop_bus_root_preserves_override_and_has_system_fallback --no-run
```

That first shared-worktree compile exposed unrelated in-progress `compute_migrate.rs` errors. Verification preserved those local edits and restored only unrelated tracked files to `HEAD` inside the disposable r83 farm slot, then overlaid the owned source. A later relink exhausted the slot; `cargo clean -p mackesd` removed only r83 package artifacts, and the final exact `--lib` build used `CARGO_INCREMENTAL=0`.

Final exact results from the formatted source:

```text
CARGO_INCREMENTAL=0 cargo test -p mackesd --lib --features async-services workers::desktop_sources::tests::failed_complete_source_read_preserves_action_and_publication_until_retry -- --exact
test workers::desktop_sources::tests::failed_complete_source_read_preserves_action_and_publication_until_retry ... ok
test result: ok. 1 passed; 0 failed; 4578 filtered out

target/debug/deps/mackesd_core-f45dbb8b3376b6e0 --exact workers::desktop_sources::tests::late_and_same_path_replacement_skip_retained_and_run_forward_without_restart
test workers::desktop_sources::tests::late_and_same_path_replacement_skip_retained_and_run_forward_without_restart ... ok
test result: ok. 1 passed; 0 failed; 4578 filtered out

target/debug/deps/mackesd_core-f45dbb8b3376b6e0 --exact workers::desktop_sources::tests::connection_path_identity_race_is_rejected_before_activation
test workers::desktop_sources::tests::connection_path_identity_race_is_rejected_before_activation ... ok
test result: ok. 1 passed; 0 failed; 4578 filtered out

target/debug/deps/mackesd_core-f45dbb8b3376b6e0 --exact workers::desktop_sources::tests::desktop_bus_root_preserves_override_and_has_system_fallback
test workers::desktop_sources::tests::desktop_bus_root_preserves_override_and_has_system_fallback ... ok
test result: ok. 1 passed; 0 failed; 4578 filtered out

rustfmt --edition 2021 --check crates/mesh/mackesd/src/workers/desktop_sources.rs
exit 0

git diff --check -- crates/mesh/mackesd/src/workers/desktop_sources.rs
exit 0
```

The late/replacement test writes forward commands through separate `Persist` handles, proves retained startup and replacement actions are skipped, and proves the first post-activation command on each live index executes without restarting the worker. The source-failure test proves malformed final Workload input causes zero manual-store, cursor, or roster-publication advancement and that a later external correction is consumed by the same worker.

## Hash

```text
a23cf39a59e3df7f2fcd7f9fad09e240169cf53467063ea9feff877f1f1e40e6  crates/mesh/mackesd/src/workers/desktop_sources.rs
```

No live mDNS/LAN discovery claim is made; the verification is limited to the Bus/source transaction and retained/forward action semantics. `WORKLIST.md`, Browser VM paths, commits, pushes, and production catalogs were not touched.
