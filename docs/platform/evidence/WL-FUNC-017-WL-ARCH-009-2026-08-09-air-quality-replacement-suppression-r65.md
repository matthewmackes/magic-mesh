# WL-FUNC-017 / WL-ARCH-009 — air-quality replacement suppression r65

Date: 2026-08-09

## Corrected semantics

- Air-quality no-fix and unconfigured suppression is associated with the device/inode identity of the current `index.sqlite`, rather than a process-global boolean. Repeated identical polls append no additional row while that identity remains current.
- A same-path atomic index replacement has a different identity, so the same worker appends exactly one corrected-forward empty status to the replacement and suppresses later identical polls on that index.
- Publication verifies the index before and after opening and again after the write. A replacement during the transaction is reported as failure and cannot install a suppression marker for the wrong index.
- Suppression changes only after a successful write. No-fix `last_good` clearing and retry-cadence reset remain inside that same success branch; a failed write preserves both and retries with bounded cadence. Existing fresh per-transaction context reads and canonical system-Bus fallback are unchanged.

## Focused BigBoy proof

Host: BigBoy `172.20.0.130`

Slot: `air-quality-replacement-r65`

The helper was pinned explicitly for source synchronization:

```text
MCNF_BUILD_HOST=172.20.0.130 \
MCNF_BUILD_SLOT=air-quality-replacement-r65 \
install-helpers/xcp-build.sh sync

Result: PASS; helper selected 172.20.0.130 and synchronized the isolated slot.
```

Final exact commands in the warmed slot:

```text
cargo test -p mackesd --lib \
  workers::air_quality_overlay::tests::repeated_no_fix_publishes_once_per_replacement_index \
  -- --exact --nocapture

Result: PASS — 1 passed, 0 failed, 4548 filtered out.
```

```text
cargo test -p mackesd --lib \
  workers::air_quality_overlay::tests::failed_unconfigured_replacement_write_does_not_suppress_retry \
  -- --exact --nocapture

Result: PASS — 1 passed, 0 failed, 4548 filtered out.
```

The first regression proves one no-fix row on the initial index, one row after same-path replacement, and no repeated-poll churn on either. The second proves one unconfigured row, no marker change after an unopenable replacement write, one corrected-forward row after recovery, and no subsequent churn.

Final farm formatting command:

```text
rustfmt --edition 2021 --config skip_children=true --check \
  crates/mesh/mackesd/src/workers/air_quality_overlay.rs

Result: PASS — no formatting diff.
```

Final scoped checks:

```text
git diff --check -- crates/mesh/mackesd/src/workers/air_quality_overlay.rs
Result: PASS — no whitespace errors.

sha256sum crates/mesh/mackesd/src/workers/air_quality_overlay.rs
e930c5d4c44f9d9aba3ef332347a2192b9d466f73ca62fc5092e74e5ec778000
```

The local and farm source hashes matched exactly. The first helper-driven test attempt was blocked before test execution by an unrelated concurrent `wildfire_overlay.rs` private-import error. Verification preserved that local file and used its committed `HEAD` form only in the disposable r65 farm slot; the final exact results above are from that isolated workaround.

## Residual caveat

The suppression identity is worker-memory state scoped to one running process and one Linux index inode. A process restart may append the current empty status again; this slice corrects live same-worker index replacement without adding a durable suppression ledger. If replacement occurs after SQLite accepts a write but before the final identity check, the detached old index may contain that row, but the transaction is treated as failed and the current index is corrected on retry.
