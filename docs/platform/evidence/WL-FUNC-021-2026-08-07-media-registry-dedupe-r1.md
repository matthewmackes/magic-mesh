# WL-FUNC-021 — media registry mirror dedupe (2026-08-07)

`mackesd`'s media registry worker already coalesced retained Bus publication,
but its replicated `media-registry.json` mirror still performed an atomic
rename on every 30-second health tick. The mirror now compares the candidate
bytes with the existing file and skips the rename when unchanged; missing,
unreadable, or changed files still use the existing atomic write path. Both
legacy Navidrome registrations and operator-configured endpoint rosters use
the shared helper.

Farm `.90`, slot `media-registry-dedupe-r1`:

```text
cargo test -p mackesd --lib media_registry --features async-services --locked -- --nocapture
test result: ok. 12 passed; 0 failed; 4387 filtered out
```

The regression checks that an identical mirror write preserves the original
inode, while the existing atomic-write and credential-redaction tests remain
green. This is source/farm evidence; installed-seat Syncthing and CPU counters
remain open while Dell is unreachable.
