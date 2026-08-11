# WL-ARCH-009 pre-activation process ownership — 2026-08-11

- Scope: each installed `mackesd serve --group` process now acquires its
  kernel-backed group lease through a fallible supervisor constructor before
  it opens the control SQLite writer, publishes startup state, or constructs a
  worker. A duplicate process therefore exits before any group-owned runtime
  side effect.
- Production path: installed grouped systemd service → `mackesd serve --group`
  → process-group lease → writer/startup publication/worker activation.
- Farm: BigBoy `172.20.0.130`, slot `2`.
- Focused gates:
  - `workers::tests::process_group_claim_rejects_duplicate_before_runtime_activation`:
    PASS, 1 passed, 0 failed;
  - production `mackesd` binary check: PASS.
- `git diff --check`: PASS.
- Remaining epic boundary: installed-package and live-fleet duplicate-
  activation proof remains.
