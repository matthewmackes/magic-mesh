# WL-CRIT-007 — distinct overlay claimant authority (2026-08-05)

The shared peer contract now defines a strict, bounded, credential-free overlay
claim containing the exact Nebula node/name/address/public certificate
fingerprint plus domain-separated machine and boot claimant digests. Raw
machine-id, boot-id, certificate bytes, paths, and secrets are absent.

The etcd publisher stores claims beneath a lease-backed namespace keyed by
certificate, machine claimant, and boot claimant. Two machines using one copied
Nebula identity therefore retain distinct simultaneous keys instead of
overwriting one hostname record; the same claimant refresh remains idempotent.
The peer directory row and claimant row share one lease and one etcd transaction.
Exact case-sensitive `peer:{hostname}`, address, schema, digest, and transaction
success checks fail closed before publication.

## Verification

- BigBoy `.130`, slot `wl-crit007-claim-publisher-r1`: focused claimant tests.
- Result: `5 passed; 0 failed; 4460 filtered out`.
- Farm `.170`, slot `wl-crit007-claim-fmt-r1`: focused `rustfmt --check` passed.
- Scoped `git diff --check` passed.

## Remaining acceptance edge

The telemetry heartbeat does not yet supply a validated public certificate
fingerprint or privacy-bounded machine/boot digests, so it still uses the legacy
peer-only publisher; the strict API intentionally cannot fabricate them. The
pre-Nebula authority dependency also remains unresolved. Consequently the
materializer, collision guard, systemd drop-in, and RPM activation still must
not ship to seats.
