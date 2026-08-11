# WL-FUNC-021 Jellyfin in-flight route authority — 2026-08-11

- Scope: the proxy revalidates exact endpoint and credential authority after
  secret lookup and before contacting the upstream.
- Hostile boundary: Bus replacement revokes an in-flight route until an exact
  corrected-forward declaration becomes authoritative.
- Focused gate: `cargo test -p mackesd --features async-services --lib workers::media_jellyfin_proxy::tests::bus_replacement_revokes_inflight_route_until_exact_corrected_forward_authority -- --exact --nocapture`.
- Farm: `172.20.0.196`, slot 1, admitted with 8,789,408 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,847 filtered out.
- Remaining boundary: live provider replacement and installed gateway proof remain.
