# WL-REL-006 UX-014 Kiron assets — current revision

This evidence records the reproducible UX-014 asset package. It is not live
hardware evidence and does not claim that the full release is admissible.

- Source commit containing the admitted manifest: `60f6d4fa9a7c0ab829710bad54f3dd3e2bd14c50`
- Farm host: `172.20.0.130` (BigBoy)
- Farm slot: `kiron-final-20260816`
- Generator: `install-helpers/produce-kiron-original-assets.py`
- Asset verifier: `install-helpers/verify-kiron-assets.py`
- Manifest SHA-256: `8559bc0031ff2f44b19675910b0252ec71a3628b5bf4f8377c08035b21a82dab`
- Package admission: `packaging/kiron/verify-package.sh --source --expected-source-revision 60f6d4fa9a7c0ab829710bad54f3dd3e2bd14c50` — PASS
- Self-test: `packaging/kiron/verify-package.sh --self-test` — PASS

The package contains 18 authored SVG scenes (grades A–F across live-3d,
pre-rendered, and static modes) and 6 synthesized 48 kHz mono PCM cues. Every
entry is content-addressed and licensed CC0-1.0. The manifest provenance hash
matches the immutable parent source used for generation; the committed package
is admitted by the exact source-bound gate.

This closes the reproducible asset portion of WL-REL-006 S6. RPM production
and live-seat acceptance remain downstream work.
