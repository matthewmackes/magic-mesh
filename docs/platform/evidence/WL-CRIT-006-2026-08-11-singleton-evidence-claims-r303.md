# WL-CRIT-006 singleton evidence claims — 2026-08-11

- Scope: release-evidence `write` admits each singleton claim exactly once,
  including source commit, manifests, topology/VDI evidence, attestations, and
  verdicts.
- Hostile boundary: a duplicate gate-manifest claim is rejected before output
  replacement and leaves an existing evidence bundle unchanged.
- Gates: `bash -n install-helpers/release-evidence.sh` and
  `install-helpers/release-evidence.sh --self-test`.
- Farm: BigBoy (`172.20.0.130`), slot 3.
- Result: **PASS** — syntax exited 0 and the deterministic binding/fail-closed
  validation self-test passed.
- Remaining boundary: one complete current-revision signed release bundle and
  seat/lighthouse/recovery acceptance remain open.
