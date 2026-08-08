# WL-FUNC-016 clipboard JSON admission — 2026-08-06

Rich clipboard JSON admission now rejects duplicate object keys at every
nesting level. The signed-envelope hostile fixture specifically proves that a
duplicate `sequence` field cannot be ambiguously admitted.

Verification:

- Farm `.50`, slot `clipboard-duplicate-keys-20260806-r1`: focused hostile
  test passed **1/1**; 64 tests were filtered.
- Farm `.90` sync/compile retry hit `ENOSPC`; no passing result is claimed for
  that lane.
- Source SHA-256:
  `b0cc25c833a3d083a4adc6b31f005a097bfddc33bab06550144e73e10f14ba13`.

Rich MIME negotiation, authenticated mesh/VDI transport, UI permissions, and
live guest proof remain open. Dell was not modified.
