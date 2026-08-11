# WL-FUNC-011 descriptor source generation — 2026-08-11

- Scope: descriptor-backed Files reads remain bound to the admitted source generation through hashing.
- Hostile boundary: replacing the canonical Files record after descriptor hashing cannot escape as a current read.
- Focused gate: `cargo test -p mackesd workers::transfers::v2::tests::descriptor_read_cannot_escape_a_replaced_source_generation -- --exact --nocapture`.
- Farm: `172.20.0.130`, slot 3, admitted with 13,758,360 KiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,874 filtered out.
- Remaining boundary: rotate a live Files source during an active descriptor-backed transfer and prove corrected-forward recovery.
