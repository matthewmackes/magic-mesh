# WL-FUNC-019 S3 resource metadata search

Date: 2026-08-13

Remote Sessions previously displayed availability, authentication, provenance,
and reachability scope on each admitted card but did not include those fields in
its pure in-memory search projection. A user therefore could not find cards by
visible terms such as `unavailable`, `auth required`, `operator`, or
`trusted LAN`.

`crates/desktop/mde-shell-egui/src/vdi/resources.rs` now applies one deterministic
query predicate across display name, summary, class, transport, current
availability, authentication, discovery source, and scope. Availability search
uses the render-provided timestamp and feed state; it performs no I/O. The
existing deterministic search/filter regression was extended to cover the new
metadata dimensions and expired-card status.

## Farm gates

Host `172.20.0.170`, slot `func019-search-test`:

```text
cargo test -p mde-shell-egui vdi::resources::tests::remote_sessions_model_search_filter_and_capability_projection_are_deterministic -- --exact --nocapture
```

Result: 1 passed, 0 failed, 1,579 filtered out.

Host `172.20.0.170`, slot `func019-search-clippy`:

```text
cargo clippy -p mde-shell-egui --bin mde-shell-egui -- -D warnings
```

Result: passed. The broader `--all-targets` probe was not used as acceptance
because it failed on pre-existing warnings in out-of-scope test code
(`car_keymap.rs`, `status_bar.rs`, and `system/mesh.rs`).

## Remaining acceptance

S3's pure browser behavior now covers deterministic grouping/filtering and
search across all displayed resource metadata, truthful stale/reconnecting and
unavailable presentation, and render-time I/O isolation. The epic still needs
its deferred post-release route captures and installed live recovery/RDP login
proof; adapter/action and release acceptance remain governed by their existing
evidence and worklist slices.
