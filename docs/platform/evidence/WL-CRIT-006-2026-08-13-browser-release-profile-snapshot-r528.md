# WL-CRIT-006 Browser release-profile snapshot — r528

Date: 2026-08-13

## Result

The tracked Browser profile is now a source template with the explicit,
non-authoritative `@RELEASE_REVISION@` marker. It no longer claims that its own
bytes contain the Git commit which contains those bytes.

`release-profile.py` accepts one externally supplied full Git revision, reads
the governed template blob from that commit (never from working-tree bytes),
requires exactly one marker, and publishes a bounded `0400` snapshot by an
fsynced exclusive hard-link operation. Existing outputs, null/malformed or
non-commit revisions, missing/changed markers, symlinks, and group/other-
writable output parents are refused.

`build-image.sh` requires the frozen snapshot and exact revision before RPM,
base-image, Podman, or output mutation. `verify-profile.sh` reproduces the
snapshot from Git and compares every byte. Image-manifest creation refuses a
template, and verification cross-binds the requested revision, frozen profile,
manifest, image, and runtime assets.

The image builder also passes that snapshot into its existing static image and
artifact verifier; no post-build verifier falls back to the tracked template.

The canonical derivative orchestrator freezes the profile before RPM admission
or either image builder, then passes the same snapshot and revision to the
Browser builder and final manifest verifier. A freeze failure cannot publish a
collection or invoke RPM verifiers/builders.

## Exact farm gates

- `.50`, `crit006-browser-profile-hostile-r528e`: initialized a slot-local Git
  history (farm sync excludes `.git`), then ran
  `packaging/browser-vm/test-profile-source-freeze.sh`; passed. The suite covers
  exact snapshot acceptance plus stale, dirty, substituted, mismatched,
  existing-output, and writable-parent refusal.
- `.50`, same slot: ran
  `install-helpers/test-build-release-derivative-images.sh`; passed. The hostile
  suite covers argument propagation, failed freeze/build/manifest paths,
  no partial publication, and preservation of signed RPM inputs.
- `.50`, same slot: produced a frozen profile at the slot-local commit, asserted
  mode `0400`, and ran `verify-image-manifest.py self-test`; passed.
- `.50`, `crit006-browser-profile-shellcheck-r528d`: strict ShellCheck and Bash
  syntax checks over both Browser and derivative producer/test shell scripts;
  passed with no diagnostics.
- `.196`, `crit006-browser-profile-python-r528c`: Python bytecode compilation
  and tabnanny for `release-profile.py` and `verify-image-manifest.py`; passed.
- Local scoped `git diff --check`; passed.

An initial producer run exposed and led to correction of a mode-`000` umask;
an initial static route found that `.170` lacks ShellCheck, so the authoritative
strict gate was routed to `.50`. Neither failed attempt is acceptance evidence.

## Remaining release acceptance

- Supply the real release revision and governed inputs, then produce the frozen
  Browser profile and all immutable first-release artifacts.
- Run the first full release and verify its RPM signatures, payloads, image
  manifests, and collected release integrity.
- Perform deferred, non-blocking post-release one-node acceptance and recovery
  proof.
