# WL-FUNC-018 production catalog importer — 2026-08-08

The signed Flatpak catalog is now consumed by a workstation-tier production
worker rather than a test-only harness. The worker loads an exact configured
signer and root-owned, regular, bounded, no-follow trust key; admits only the
signed catalog contract; retains a monotonic catalog/revision/digest watermark
across expiry and restart; and atomically persists its last-good envelope.

Expired catalogs project an empty installed-app snapshot without erasing the
rollback watermark. Identity changes, rollback, same-revision digest conflicts,
tampering, unsafe trust/state files, and malformed payloads fail closed while
retaining the last good catalog. Only installed rows are projected. Statuses do
not echo rejected payloads and identical unavailable/refusal states are
edge-triggered instead of republished every poll.

Production reachability is explicit in `workers/mod.rs`, the workstation Data
worker registry, and `spawn.rs`. The temporary integration-test include harness
was removed after registration.

## Verification

- `.196`, slot `func018-app-catalog-production-s2-r1`: production module tests
  passed 6/6, with 4,428 unrelated tests filtered.
- `.170`, slot `func018-app-catalog-production-bin-s2-r1`: `cargo check --locked
  -p mackesd --bin mackesd` passed, proving the production spawn path compiles.
- The earlier signed-contract proof remains 12/12 focused and 480/480 complete
  on `.196`.

## Remaining acceptance gap

Release packaging must provision the root-owned signer/key/state configuration,
and a live signed import/restart/expiry trace must prove the installed projection
consumer. The reproducible App VM image/profile, typed lifecycle, Front Door UX,
sandbox/package security, and five-seat proof remain, so FUNC-018 stays
`Remaining`.
