# WL-ARCH-009 CA SQLite writer migration — 2026-08-08

Five certificate-authority mutation families now cross the bounded typed store
writer in split `serve` processes and the same finite dispatcher in standalone
commands: initial CA mint, peer-certificate upsert, revocation, disaster-recovery
restore, and epoch rotation. Rotation is a compare-and-swap against the expected
active epoch and identical post-restart retries are idempotent. Restore and
rotation apply complete CA/peer generations in one SQLite transaction.

The writer frame remains finite at 2 MiB, providing headroom for the separately
bounded one-MiB CA archive. Inputs cap issuer/peer counts, identities,
coordinates/IPs, duplicate generations, active issuers, and malformed epoch
transitions before mutation. A transaction fault injected after the CA prefix
proves rollback before the owner accepts the next healthy request.

The checked conservative direct-SQLite inventory fell from 61 to 48 syntax
sites. Global read-only enforcement is still intentionally deferred until the
remaining classified sites migrate.

## Verification

- BigBoy `.130`, warm slot `arch010-migration-journal-tests-r3`:
  `cargo test --locked -p mackesd --lib store::writer::tests -- --nocapture`:
  6 passed, 0 failed, 4,373 filtered out.
- The same warm slot ran
  `cargo test --locked -p mackesd --lib ca:: -- --nocapture`:
  141 passed, 0 failed, 4,238 filtered out.
- `lint-mackesd-sqlite-authority.sh`: passed with 48 reviewed residual sites.
- Focused remote rustfmt for `store/writer.rs`: passed.
- `git diff --check`: passed.

## Source hashes

```text
263fe10833ad33e1faf32e29f9e1be1b8a129b751810392b56a3f35923757368  crates/mesh/mackesd/src/store/writer.rs
0cab64b9461995de898957dec54d96d93c408d933188fc23c26fa8af4d493152  crates/mesh/mackesd/src/ca/backup.rs
65a30964299a5d1153fa64cb0072770975f9fd869cc6f7b4cbcb223999ac03ea  crates/mesh/mackesd/src/ca/epoch.rs
7dd5a980353b2a9bf0835df6a7d7f1df0f8222f4bc723ec946cc56b88f78fd0d  crates/mesh/mackesd/src/ca/mint.rs
571e46ab6835db425d6bccef468013b9d6166b4d0888e912de9c5fbc1bfac3c2  crates/mesh/mackesd/src/ca/revoke.rs
9564afbb046d252f8ac5b59882ee481b5163a2989fb9f0007c2a01bdd300e95f  crates/mesh/mackesd/src/ca/sign.rs
8b510c7dd8857e53103e3e388444a9b50c6ecfb4f7e0585e3bd5b9e66c5003a6  docs/platform/mackesd-sqlite-direct-write-baseline.tsv
```

## Remaining acceptance gap

This is a material one-writer migration slice, not completion of ARCH-009 S4.
Forty-eight conservative residual syntax sites remain, non-owner connections
are not globally read-only, and six-process built-package crash/recovery proof
is still required. ARCH-009 remains `Remaining`.
