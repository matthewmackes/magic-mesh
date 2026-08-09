# WL-ARCH-008 standalone Browser publication S1 — 2026-08-08

The Browser stack is published at
`https://github.com/matthewmackes/magic-mesh-browser-stack` on `main` revision
`2b36cedb62259d764b37b1b83d1db433fdb5297e`. The fast-forward from
`3028c244` includes the immutable source-history merge and standalone workspace,
CI, provenance, legal, and package contracts.

The final boundary correction removed machine-local `magic-mesh` paths from
fixtures and removed systemd ordering against units supplied only by the source
repository. The verifier now scans all tracked text for local/sibling source
paths while allowing the source remote only in provenance documents; Cargo Git
dependencies back to `magic-mesh` remain forbidden.

## Verification

- `.196`, clean clone plus the candidate patch in directory
  `magic-mesh-browser-farm-browser-standalone-boundary-r3`:
  `install-helpers/verify-standalone-workspace.sh` passed. Dependency/workspace,
  122-path provenance, legal, and 47-row package payload contracts all passed.
- CI exposed early-exit races in the translation, text-to-speech, and
  speech-to-text command wrappers: a child could close stdin before the parent
  wrote its request and be misreported as a generic broken pipe. All three now
  reap and classify the authoritative child status first; successful STT and
  translation output also wins over a racy feed error.
- `.170` passed all 123 `mde-browser-workers` library tests, including the three
  early-exit regressions; `.50` passed warning-denied all-target clippy for the
  crate; `.196` passed crate formatting and the standalone verifier.
- `.170` then passed the complete admitted-workspace test suite and both
  warning-denied CI clippy lanes. The first clippy attempt exposed a full farm
  artifact disk; five stale, rebuildable target trees were removed, recovering
  29 GiB, and the unchanged candidate passed on retry.
- GitHub Actions run `31277690513` passed from a clean checkout of exact revision
  `2b36cedb62259d764b37b1b83d1db433fdb5297e`: admitted workspace tests and
  clippy, standalone boundary/package verification, sandbox tests, CEF tests,
  and the separate Servo all-target check all succeeded.
- `git ls-remote --heads origin main` returned the exact published revision.

## Remaining acceptance gap

The standalone publication S1 acceptance is satisfied. Live legacy-profile
import, Browser VM image,
shell VDI/audio behavior, performance, upgrade cleanup, and five-seat proof
remain, so ARCH-008 stays `Remaining`.
