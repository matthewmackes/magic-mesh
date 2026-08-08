# WL-FUNC-021 — live Music seat executable provenance guard (2026-08-07)

## Change

`verify-music-live-seat.sh` now verifies the active `mde-musicd.service`
`MainPID`: its `/proc/<pid>/exe` must resolve to `/usr/bin/mde-musicd`, and
`rpm -qf` for that executable must match the source-declared package name,
version, release, and architecture. An installed RPM can no longer make a
stale, alternate, or unowned running process look current.

The provider-loss helper was audited and left unchanged; its loopback-only,
fail-closed reset/recovery witness remains bounded.

## Verification

- Farm `.50`, slot `music-live-helper-provenance-r1`: `bash -n` passed for
  both helpers; both self-tests passed.
- The updated read-only seat-15 run answered ping, `get-state`, and
  `list-albums`, but correctly rejected both the active executable provenance
  and installed `magic-mesh-12.1.6-4` against the current source `12.1.6-5`.

No live state was changed. Current-package, five-seat CPU, provider-loss,
renderer, and two-seat handoff acceptance remain open.
