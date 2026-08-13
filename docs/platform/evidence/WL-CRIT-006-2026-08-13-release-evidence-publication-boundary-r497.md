# WL-CRIT-006 release-evidence publication boundary (r497)

Date: 2026-08-13

## Scope and result

`install-helpers/release-evidence.sh write` previously validated the generated
schema-5 envelope before an atomic rename, but unlike `write-binding` it did not
reject a symlinked output parent or output pathname and did not normalize the
published mode. A valid envelope could therefore be redirected through a
caller-controlled path or receive caller-dependent permissions.

The writer now fails closed unless the output parent is a real directory,
records and re-attests that directory's device/inode immediately before
publication, refuses an existing or newly introduced output symlink, removes
all private sidecars on failure, and publishes deterministic mode `0644`.
Deferred live acceptance remains a post-release, non-blocking obligation; this
slice performs no live acceptance and does not weaken any verdict requirement.

## Farm evidence

- `.196`, slot `crit006-evidence-publication-selftest-r497-final`: full
  `bash install-helpers/release-evidence.sh --self-test` passed, including the
  hostile output-symlink preservation fixture and deterministic-mode assertion.
- `.50`, slot `crit006-evidence-publication-syntax-r497-final`: `bash -n`
  passed.
- `.90`, slot `crit006-evidence-publication-matrix-r497-final`:
  `verify-release-gate-matrix.py --self-test` passed one canonical matrix and
  rejected all 19 hostile fixtures.

## Remaining epic acceptance

The first release still requires its build, package, signing, and artifact
integrity gates. Installed single-seat recovery and corrected-forward live proof
remain deferred until after that release and are non-blocking, per the current
WL-CRIT-006 required outcome.
