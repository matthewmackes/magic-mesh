# WL-FUNC-021 — running-apps phase and write deduplication (2026-08-07)

## Finding

`apps_running` walked `/proc` and atomically rewrote the replicated
`running-apps.json` document every ten seconds, even when the running set was
unchanged. Its first scan was also aligned to the worker start boundary on
every seat.

## Change

The worker now uses a stable hostname-derived phase bounded to 1.5 seconds
before its first scan. Identical serialized documents skip the write, while a
missing or changed file is still recreated atomically.

## Verification

BigBoy farm lane `apps-running-phase-r2` passed:

```text
cargo test -p mackesd apps_running --features async-services --locked -- --nocapture
8 passed, 0 failed, 4401 filtered
```

The initial `.170` lane could not sync because that farm VM reported `ENOSPC`;
no source compilation was attempted there. Dell/live CPU proof remains open.
