# WL-FUNC-017 navigation provider authority inode — 2026-08-11

- Scope: one governed route calculation must remain bound to the exact securely opened provider-authority inode.
- Hostile boundary: even byte-identical atomic authority replacement during provider I/O revokes the in-flight result.
- Focused gate: `cargo test -p mackesd workers::navigation::tests::byte_identical_authority_replacement_cannot_authorize_in_flight_route -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 11,211,132 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,870 filtered out.
- Remaining boundary: live authority rotation during route calculation and corrected-forward success under the replacement authority remain.
