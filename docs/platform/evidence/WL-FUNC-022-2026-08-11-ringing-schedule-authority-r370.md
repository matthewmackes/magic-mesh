# WL-FUNC-022 Ringing schedule authority — 2026-08-11

- Scope: a ringing occurrence retains the schedule payload admitted when that
  occurrence began, including restart recovery.
- Hostile boundary: replacing the schedule cannot lend new sound or target
  authority to an older still-ringing occurrence.
- Focused gate: `cargo test -p mackesd workers::clock::tests::ringing_occurrence_cannot_inherit_replacement_schedule_authority_after_restart -- --exact --nocapture`.
- Farm: `172.20.0.90`, slot 1, admitted with 10.7 GiB free.
- Result: **PASS**, 1 passed, 0 failed, 4,852 filtered out.
- Remaining boundary: installed-seat ringing and multi-peer recovery proof remain.
