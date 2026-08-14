# UX-014 original KIRON package — 2026-08-14

The repository now contains a reproducible original asset producer at
`install-helpers/produce-kiron-original-assets.py`. It authors 18 source-owned
SVG fallback assets (A–F × live-3d/pre-rendered/static) and six short original
mono PCM WAV cues under `assets/kiron/payload/`, with CC0 provenance, hashes,
and a source-revision-bound `manifest-v2.json`.

Validation:

```text
python3 install-helpers/produce-kiron-original-assets.py
packaging/kiron/verify-package.sh --source
  OK: six-grade fallback ladder and hashes verified
  PASS: governed workstation RPM asset package admitted
packaging/kiron/verify-package.sh --self-test
  PASS: schema hostility + RPM wiring + missing production rejection
```

This closes the missing authored-package input for S2. Direct renderer,
installed-seat, audio, and live proof remain downstream UX-014 work.
