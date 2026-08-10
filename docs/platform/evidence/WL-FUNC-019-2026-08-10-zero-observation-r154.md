# WL-FUNC-019 — zero-timestamp desktop projection (r154)

Date: 2026-08-10

Desktop/RDP resource projection now refuses a source observation timestamp of
zero before creating a card. This prevents an unobserved source from becoming
connectable through the approval path.

## Farm proof

Build VM `.50` (`172.20.0.50`), slot `func019-zero-observed-r154`:

```text
cargo test -p mackesd --lib workers::desktop_sources::tests::desktop_resource_projection_rejects_zero_observation_timestamp -- --nocapture
1 passed; 0 failed; 0 ignored; 0 measured; 4694 filtered out
```

Authenticated login/render and installed-key distribution remain open.
