# WL-FUNC-018 active App-session catalog revision — 2026-08-11

- Scope: an active App-VM session remains bound to its admitted catalog revision across retries.
- Hostile boundary: stale or future revision substitution preserves desired state and emits no extra Workload/OpenApp publication.
- Focused gate: `cargo test -p mackesd workers::cloud::verbs::app::tests::active_app_session_cannot_publish_a_substituted_catalog_revision -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 2, admitted with 24,939,660 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,858 filtered out.
- Remaining boundary: installed App-VM catalog, StartAndAttach/OpenApp, VDI, reconnect, stop, and upgrade proof remain.
