# WL-CRIT-006 — Browser profile source freeze (r526)

Date: 2026-08-13

## Result

The Browser image builder now admits its profile before RPM staging, Podman
resolution/build, or output-directory mutation. `verify-profile.sh` accepts an
explicit full release revision and requires:

- the profile's declared source commit to equal that requested revision;
- the revision to resolve to a Git commit;
- the governed profile path to exist at that commit; and
- every profile byte except the self-referential source-commit value to match
  the Git blob at that commit exactly.

Normalizing only the source-commit value avoids an impossible Git hash
fixed-point while retaining exact Git-backed identity for the complete Browser
configuration. A stale declaration, dirty comment/configuration byte,
substituted profile, missing commit, or revision mismatch fails before the
builder mutates image state. The derivative orchestrator already compares that
same declared commit to its requested `--source-revision`, so its requested
release identity and the builder's exact profile admission form one fail-closed
chain without a second profile contract.

## Focused hostile coverage

`packaging/browser-vm/test-profile-source-freeze.sh` proves acceptance of the
exact governed blob with only its release-binding field frozen, and refusal of
a stale declaration, dirty bytes, substituted bytes, and a mismatched requested
revision.

## Gates

- `.50`, slot `crit006-browser-profile-freeze-r526c`: initialized a slot-local
  Git fixture (farm sync intentionally excludes `.git`) and ran
  `packaging/browser-vm/test-profile-source-freeze.sh`; passed.
- `.50`, slot `crit006-browser-profile-shellcheck-r526b`: strict ShellCheck for
  `build-image.sh`, `verify-profile.sh`, and the hostile test; passed with no
  diagnostics.
- Local `bash -n` for all three scripts and scoped `git diff --check`; passed.

Two earlier farm attempts are not acceptance evidence: the wrapper rejected an
unsupported direct-command verb before execution, and the first hostile
fixture tried to append after mode `0400`. The fixture was corrected and the
clean r526c rerun above is the recorded result.

## Remaining WL-CRIT-006 acceptance

- Freeze the canonical profile's source-commit field to the actual first full
  release revision.
- Supply the real signed RPM, base-image, trust, bootc, Android, and identity
  inputs; build and collect the immutable first-release outputs.
- Run the first full release and verify its signatures, payloads, manifests,
  and release integrity.
- Perform the deferred, non-blocking post-release one-node acceptance and
  corrected-forward recovery proof.
