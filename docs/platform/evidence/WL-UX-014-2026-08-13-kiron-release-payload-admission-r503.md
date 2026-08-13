# WL-UX-014 — Kiron release-payload admission (r503)

The full/workstation Fedora RPM cut now runs
`packaging/kiron/verify-package.sh --source` before compiling. The gate requires
the canonical v2 manifest and payload tree, verifies the manifest and every
asset through `verify-kiron-assets.py`, and confirms both paths are declared in
the base RPM asset list. Server and lighthouse cuts do not carry workstation
visual/audio assets.

No production Kiron assets are present in this revision. That is intentional:
the source admission gate and the ordinary static RPM payload gate both reject
the missing package, so a release cannot silently omit or bypass it. No
placeholder artwork, audio, license, provenance, or acceptance claim was added.

Farm gates (2026-08-13):

- `.50`, slot `ux014-kiron-self`: dedicated verifier self-test passed, covering
  schema hostility, RPM wiring, and missing-production rejection.
- `.90`, slot `ux014-kiron-source`: `verify-package.sh --source` rejected the
  absent canonical `assets/kiron/manifest-v2.json` as required.
- `.170`, slot `ux014-kiron-rpm`: the static base-RPM payload gate rejected both
  the missing manifest and empty `assets/kiron/payload/**/*` source.
- Local tiny checks: Bash syntax and `git diff --check` passed.

Remaining acceptance: author and license the real six A-F live-3D,
pre-rendered, static, and audio assets; publish their immutable identities in
`assets/kiron/manifest-v2.json`; then pass the full RPM build and deferred
post-release installed-seat visual/audio proof.
