# WL-CRIT-006 Browser output source-revision admission — r527

Date: 2026-08-13

`packaging/browser-vm/verify-image-manifest.py verify` now requires an explicit
`--source-revision`, suitable for the release-output collector's
`{source_revision}` substitution. Verification rejects malformed and null Git
identities, rejects a requested revision that differs from the immutable
profile, and reconstructs the complete manifest from that profile, image, and
runtime assets. Consequently a pass binds the requested revision to both the
profile and the manifest's `profile.source_commit`; stale manifest identity is
rejected by the existing hostile case.

## Farm gates

- `.50`, slot `crit006-browser-output-revision-test-r527`:
  `python3 packaging/browser-vm/verify-image-manifest.py self-test --repo-root .
  --profile packaging/browser-vm/profile.env` — passed. This includes the new
  malformed-request and requested/profile mismatch refusals plus the existing
  stale-manifest refusal.
- `.170`, slot `crit006-browser-output-revision-compile-r527`:
  `python3 -m py_compile packaging/browser-vm/verify-image-manifest.py` — passed.
- `.196`, slot `crit006-browser-output-revision-tabnanny-r527`:
  `python3 -m tabnanny packaging/browser-vm/verify-image-manifest.py` — passed.
- Local: `git diff --check` — passed.

These gates verify the Browser output-verifier prerequisite only. Production
release artifact generation and the full collector run remain first-release
work; live one-node acceptance remains deferred and non-blocking until after
that release.
