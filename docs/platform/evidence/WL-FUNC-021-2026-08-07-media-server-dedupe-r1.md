# WL-FUNC-021 — media-server manifest write deduplication (2026-08-07)

## Finding

The media-server worker rescans shared folders every 30 seconds and rewrote
the replicated `media-library.json` manifest on every pass, even when the
serialized manifest was unchanged.

## Change

`write_manifest` now compares the existing complete body before replacing it.
Changed or missing manifests still use the existing atomic temporary-file and
rename path; identical manifests preserve their inode and avoid unnecessary
replicated-file churn.

## Verification

BigBoy farm lane `apps-running-phase-r2`:

```text
cargo test -p mackesd media_server --features async-services --locked -- --nocapture
23 passed, 0 failed, 4390 filtered
```

The initial `.90` lane stopped during cold dependency output with `ENOSPC`;
the rerouted BigBoy test completed successfully. Live renderer and Dell proof
remain open.
