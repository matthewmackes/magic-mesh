# WL-UX-011 Phone Hub physical-identity equivocation evidence — 2026-08-11

- Scope: Phone Hub projects a physical `device_id` only when all rows claiming
  it agree. Exact duplicates collapse, empty identities are refused, and
  unrelated valid phones remain projected and actionable.
- Hostile boundary: conflicting metadata under one physical identity suppresses
  that identity instead of selecting one declaration by input order.
- Focused gate: `cargo test -p mde-shell-egui phones_hub::tests::fold_devices_suppresses_equivocating_physical_identity -- --exact --nocapture`.
- Farm: BigBoy (`172.20.0.130`), slot 3.
- Result: **PASS**, 1 passed, 0 failed, 1,554 filtered out.
- Remaining boundary: live provider controls and physical-seat proof remain.
