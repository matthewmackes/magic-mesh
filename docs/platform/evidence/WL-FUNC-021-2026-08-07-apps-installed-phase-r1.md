# WL-FUNC-021 — installed-apps phase and write deduplication (2026-08-07)

## Finding

`apps_installed` scanned local desktop entries and rewrote the replicated
`apps-installed.json` document on every 60-second tick, with identical startup
boundaries across seats.

## Change

The worker now uses a stable hostname-derived phase bounded to 1.5 seconds
before its first scan. Identical serialized documents skip the atomic rename;
missing or changed files still publish normally.

## Verification

BigBoy farm lane `apps-running-phase-r2` passed:

```text
cargo test -p mackesd apps_installed --features async-services --locked -- --nocapture
5 passed, 0 failed, 4408 filtered
```

The `.50` lane was abandoned after `ENOSPC`; no live seat was changed and Dell
CPU acceptance remains open.
