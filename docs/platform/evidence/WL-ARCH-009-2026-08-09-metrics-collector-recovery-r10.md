# WL-ARCH-009 metrics collector recovery — r10

Date: 2026-08-09

## Live fault and correction

Dell repeatedly logged a metrics-exporter failure because
`/var/lib/node_exporter/textfile_collector` did not exist. The worker retried
forever but could not recreate its owned publication directory.

`write_textfile` now creates a missing collector directory, rejects a symlink
or non-directory substitution, publishes through a unique create-new temporary
file, and removes that temporary file on write, sync, or rename failure. The
existing final-file rename remains the atomic publication boundary.

## Focused verification

Farm machine 194 (`172.20.0.170`), warm slot `1`:

```text
cargo test -p mackesd --lib --features async-services \
  metrics::tests::write_textfile_ --locked -- --nocapture
```

Result: **3 passed, 0 failed, 4,389 filtered out**. No unrelated broad test was
run.

## Remaining live boundary

The source correction is not yet installed on Dell. Live recovery proof must
show the directory and `mackesd.prom` recreated by the deployed grouped daemon.
