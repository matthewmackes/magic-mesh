# WL-ARCH-008 — portable Browser bundle integrity (r176)

## Outcome

The Browser profile migration now revalidates every existing imported payload
against its manifest size and SHA-256 before declaring a repeated migration
idempotent. It also refuses missing, unexpected, symlinked, unsafe, duplicated,
or malformed payload identities and verifies a newly staged tree before atomic
publication. A matching `manifest.json` can no longer hide a deleted download,
tampered file, or injected payload.

The hostile fixtures exercise deleted, tampered, unexpected, and symlinked
payloads. The existing deterministic repeat, source-race, duplicate-identity,
secret exclusion, and partial-publication checks remain active.

## Farm verification

Machine 193 (`172.20.0.90`), explicit slot
`arch008-portable-integrity-r145`:

```text
python3 install-helpers/verify-browser-portable-boundary.py --self-test
migrate-browser-profile: self-test passed
browser portable boundary: PASS
verify-browser-portable-boundary.py: self-test passed
```

No physical seat was used.

## Remaining boundary

This proves source-level bundle integrity with disposable fixtures. It does not
perform the still-required live legacy-profile import, guest restore, package
upgrade, or Browser VM quality/performance proof.
