# WL-ARCH-008 portable manifest identity — 2026-08-09

## Outcome

The Browser portable-data boundary now requires source identities and imported
destination identities to be unique. Every imported row must name a destination,
any failed row rejects the bundle, and imported/skipped/failed counts must
exactly match the manifest entries. A duplicate input root previously produced
duplicate rows that remained deterministic and could therefore pass the older
idempotency check while overwriting the same destination.

The hostile fixture runs the real migration helper with a duplicate profile
root and requires an explicit duplicate-source refusal. Existing allowlist,
symlink rejection, secret exclusion, byte-identical repeatability, downloads,
policies, sessions, history, bookmark, and extension checks remain active.

## Farm verification

- Machine 193 (`172.20.0.90`), slot `arch010-r12-lints`:
  `python3 install-helpers/verify-browser-portable-boundary.py --self-test`
  passed, including the migration helper's own self-test and duplicate-root
  hostile fixture.

## Source hashes

- `311bd6d82e87c75afceb5ea4b8f1e5153c01f5623fb84d60b8f4ba3bbdf95839`
  — `install-helpers/verify-browser-portable-boundary.py`

## Remaining boundary

This strengthens the source-level migration boundary but is not live legacy
profile import evidence. Two live consecutive imports, guest restore, secret
scan, package upgrade, and five-seat proof keep WL-ARCH-008 `Remaining`.
