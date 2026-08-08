# WL-FUNC-021 — Music DRM proof provenance binding (2026-08-07)

## Change

`verify-music-drm-proof.py` now requires the adjacent `.json` producer record
for every PNG. It rejects missing or symlinked metadata, malformed or duplicate
JSON fields, unknown/missing fields, non-`direct-drm-egl-readback` sources,
dimension mismatches, invalid bounded `DrmFourcc` values, and oversized files.
Passing output includes the PNG and metadata SHA-256 hashes.

## Verification

- Farm `.90`, slot `music-drm-proof-sidecar-r1`: Python compilation and the
  expanded self-test passed.
- The self-test covers valid metadata, missing metadata, wrong source,
  dimension mismatch, duplicate fields, truncated/CRC-invalid PNGs, size
  limits, and PNG/metadata symlinks.

This binds a captured frame to its runtime metadata but remains artifact proof,
not live DRM hardware acceptance. Live render, current package, and five-seat
acceptance remain open.
