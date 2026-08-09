# WL-ARCH-009 Nebula SQLite writer cutover — 2026-08-08

The Nebula roster and enrollment paths no longer write their node,
certificate, revocation, or rotation fixture state through direct SQLite
connections. They submit strict typed operations to the process-isolated writer.
The writer bounds every field and collection, rejects duplicate and unknown
fields, validates timestamps, performs rotation with a transactional
compare-and-swap, and makes certificate replay restart-idempotent.

The checked direct-write baseline fell honestly from 48 to 35 residual sites.

## Verification

BigBoy `.130`, slot `arch009-nebula-writer-r3`:

- `cargo test --locked -p mackesd --lib store::writer::tests -- --nocapture`:
  6/6 passed.
- `cargo test --locked -p mackesd --lib nebula_roster::tests -- --nocapture`:
  7/7 passed.
- `cargo test --locked -p mackesd --lib nebula_enroll::tests -- --nocapture`:
  49/49 passed.
- `lint-mackesd-sqlite-authority.sh --self-test` and the actual authority lint
  passed with 35 residual sites.
- Scoped rustfmt and `git diff --check` passed.

## Remaining acceptance gap

Thirty-five allowlisted direct-write sites and the live six-group writer
failure/restart proof remain, so ARCH-009 remains `Remaining`.
