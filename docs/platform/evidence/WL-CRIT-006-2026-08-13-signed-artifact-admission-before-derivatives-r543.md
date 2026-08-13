# WL-CRIT-006 signed-artifact admission before derivatives — 2026-08-13

## Production result

The restart-safe first-full-release `resume` transaction now constructs the
canonical seven-role collection plan and runs every owning artifact verifier
before derivative image construction begins. A rejected RPM, VM image,
Cuttlefish image, bootc receipt, signature, manifest, or source identity can no
longer trigger derivative build side effects. Collection remains private until
the full phase atomically publishes, so a later derivative failure exposes no
partial release output.

This ordering is independent of fleet size and supports the current one-node
release baseline. It does not perform or claim post-release live acceptance.

## Hostile proof

- Farm host/slot: `172.20.0.170`, slot `2` (`magic-mesh-farm-2`).
- Capacity at admission: 12.66 GiB free was reported as `13,271,708 KiB`, above
  the enforced `8,388,608 KiB` reserve.
- Command: `bash install-helpers/test-run-first-full-release.sh` after an
  explicit farm sync.
- Result: **PASS** — the complete hostile phase-boundary suite passed.
- New hostile assertion: a canonical collector/verifier refusal produced no
  derivative invocation and no caller-visible release output.
- Additional gates on the same farm snapshot:
  `bash -n install-helpers/run-first-full-release.sh install-helpers/test-run-first-full-release.sh`
  and scoped `git diff --check`; both passed.

## Residual acceptance

- Run the first full Fedora 44 release using the operator-governed signing key
  and the exact seven real artifacts.
- Retain its signed farm, package, and artifact-integrity evidence.
- After that release, run the deferred non-blocking single-seat live acceptance
  and corrected-forward recovery proof. Additional nodes remain optional.
