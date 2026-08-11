# WL-CRIT-006 CI log inode — 2026-08-11

- Scope: promotion verification hashes and semantically validates one opened `ci-gate.log` inode, then confirms the caller-visible sibling still names that inode.
- Hostile boundary: byte-identical pathname replacement and in-place mutation during verification both fail closed.
- Focused gate: `bash install-helpers/ci-gate.sh --self-test` on a clean farm sync.
- Farm: `172.20.0.196`, slot 1, admitted with 13,618,908 KiB free.
- Result: **PASS**, including the clean green fixture and hostile inode-replacement/mutation fixtures.
- Remaining boundary: publish a real signed release candidate through the live promotion consumer and retain its authenticated gate bundle.
